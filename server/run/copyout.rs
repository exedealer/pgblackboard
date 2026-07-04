use bytes::{Bytes, BytesMut};
use futures_util::stream;
use openssl::rand::rand_bytes;
use tokio::sync::mpsc;

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use crate::{AppError, pg};

pub async fn api_copyout(
  axum::extract::Path(token): axum::extract::Path<String>,
  copyout_bridge: axum::extract::State<CopyoutBridge>,
) -> Result<axum::response::Response, AppError> {
  // TODO HEAD request should not take_rx
  let rx = u128::from_str_radix(&token, 16)
    .ok()
    .and_then(|key| copyout_bridge.take_rx(key));

  let Some(mut rx) = rx else {
    return axum::response::Response::builder()
      .status(axum::http::StatusCode::NOT_FOUND)
      .header("content-type", "text/plain; charset=utf-8")
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

async fn pump(
  pgconn: &mut pg::Connection,
  tx: &mpsc::Sender<Bytes>,
) -> io::Result<()> {
  let mut buf = BytesMut::with_capacity(8 * 1024);

  loop {
    if pgconn.is_drained() && !buf.is_empty() {
      let chunk = buf.split().freeze();
      if let Err(_) = tx.send(chunk).await {
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
    let _ = tx.send(leftover).await;
  }

  let fin = Bytes::new();
  let _ = tx.send(fin).await;

  Ok(())
}

// TODO рассмотреть возможность общего моста для copyout/copyin
#[derive(Clone)]
pub struct CopyoutBridge {
  map: Arc<Mutex<HashMap<u128, mpsc::Receiver<Bytes>>>>,
}

impl CopyoutBridge {
  pub fn new() -> Self {
    Self { map: Arc::new(Mutex::new(HashMap::new())) }
  }

  fn take_rx(&self, key: u128) -> Option<mpsc::Receiver<Bytes>> {
    self.map.lock().unwrap().remove(&key)
  }

  pub fn open(&self) -> Copyout<'_> {
    let mut rnd = [0; _];
    rand_bytes(&mut rnd).unwrap(); // TODO no unwrap
    let key = u128::from_be_bytes(rnd);
    let (tx, rx) = mpsc::channel(1);
    self.map.lock().unwrap().insert(key, rx);
    Copyout { key, tx, container: self }
  }
}

pub struct Copyout<'a> {
  key: u128,
  tx: mpsc::Sender<Bytes>,
  container: &'a CopyoutBridge,
}

impl Copyout<'_> {
  pub fn token(&self) -> impl std::fmt::Display {
    std::fmt::from_fn(|f| write!(f, "{:032x}", self.key))
  }

  pub async fn pump(self, pgconn: &mut pg::Connection) -> io::Result<()> {
    tokio::select! {
      res = pump(pgconn, &self.tx) => res,
      _ = self.tx.closed() => {
        let _ = pgconn.cancel().await;
        Err(io::Error::other("client gone during download"))
      }
    }
  }
}

impl Drop for Copyout<'_> {
  fn drop(&mut self) {
    // clean up if the receiver was not taken
    self.container.take_rx(self.key);
  }
}
