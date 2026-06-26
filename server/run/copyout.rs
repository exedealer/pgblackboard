use bytes::{Bytes, BytesMut};
use futures_util::stream;
use openssl::rand::rand_bytes;
use tokio::sync::mpsc;

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use crate::{AppError, pg};

pub async fn serve_copyout(
  axum::extract::Path(id): axum::extract::Path<String>,
  copyouts: axum::extract::State<CopyoutStore>,
) -> Result<axum::response::Response, AppError> {
  // TODO HEAD request should not take_rx
  let Some(mut rx) = copyouts.take_rx(&id) else {
    return axum::response::Response::builder()
      .status(axum::http::StatusCode::NOT_FOUND)
      .body("not found\n".into())
      .map_err(|err| err.into());
  };

  let s = stream::poll_fn(move |cx| {
    rx.poll_recv(cx).map(|chunk| match chunk {
      Some(fin) if fin.is_empty() => None, // send final chunk `0\r\n\r\n`
      Some(data) => Some(Ok(data)),
      // TODO log error?
      None => Some(Err("incompete response")), // drop connection
    })
  });
  axum::response::Response::builder()
    // TODO content-disposition: attachment; filename=
    .header("content-type", "application/octet-stream")
    .header("x-accel-buffering", "no") // disable nginx response buffering
    .body(axum::body::Body::from_stream(s))
    .map_err(|err| err.into())
}

#[derive(Clone)]
pub struct CopyoutStore {
  rxs: Arc<Mutex<HashMap<u128, mpsc::Receiver<Bytes>>>>,
}

impl CopyoutStore {
  pub fn new() -> Self {
    Self { rxs: Arc::new(Mutex::new(HashMap::new())) }
  }

  fn take_rx(&self, id: &str) -> Option<mpsc::Receiver<Bytes>> {
    u128::from_str_radix(id, 16)
      .ok()
      .and_then(|id| self.rxs.lock().unwrap().remove(&id))
  }

  pub fn add_new(&self) -> Copyout<'_> {
    let id_int = {
      let mut rnd = [0; _];
      rand_bytes(&mut rnd).unwrap(); // TODO no unwrap
      u128::from_be_bytes(rnd)
    };
    let (tx, rx) = mpsc::channel(1);
    self.rxs.lock().unwrap().insert(id_int, rx);
    Copyout { id_int, tx, store: &self }
  }
}

pub struct Copyout<'a> {
  id_int: u128,
  tx: mpsc::Sender<Bytes>,
  store: &'a CopyoutStore,
}

impl Drop for Copyout<'_> {
  fn drop(&mut self) {
    // clean up if the receiver was not taken
    self.store.rxs.lock().unwrap().remove(&self.id_int);
  }
}

impl Copyout<'_> {
  pub fn id(&self) -> impl std::fmt::Display {
    std::fmt::from_fn(|f| write!(f, "{:032x}", self.id_int))
  }

  pub async fn pump(self, pgconn: &mut pg::Connection) -> io::Result<()> {
    tokio::select! {
      res = self.pump_inner(pgconn) => res,
      _ = self.tx.closed() => {
        let _ = pgconn.cancel().await;
        Err(io::Error::other("client gone during download"))
      }
    }
  }

  async fn pump_inner(&self, pgconn: &mut pg::Connection) -> io::Result<()> {
    let mut buf = BytesMut::with_capacity(8 * 1024);

    loop {
      if pgconn.is_drained() && !buf.is_empty() {
        let chunk = buf.split().freeze();
        if let Err(_) = self.tx.send(chunk).await {
          let () = std::future::pending().await; // let tx.closed() win
          unreachable!();
        }
      }

      match pgconn.recv_message().await? {
        pg::CopyData(data) => buf.extend_from_slice(data),
        pg::CopyDone => break,
        pg::NoticeResponse(notice) => {
          log::debug!("notice during COPY OUT: {notice}");
          // TODO report notice.
          // Downloading can be suspended if we report notice to msgw
        }
        pg::ParameterStatus(..) => {}
        _ => {} // TODO return error on unexpected message
      }
    }

    if !buf.is_empty() {
      let leftover = buf.freeze();
      let _ = self.tx.send(leftover).await;
    }

    let fin = Bytes::new();
    let _ = self.tx.send(fin).await;

    Ok(())
  }
}
