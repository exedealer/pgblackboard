use axum::extract::Query;
use axum::response::Response;
use openssl::rand::rand_bytes;
use serde::Deserialize;
use tokio::sync::oneshot;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::AppError;

#[derive(Deserialize)]
pub struct UnsuspendParams {
  token: String,
}

pub async fn api_unsuspend(
  suspend_bridge: axum::extract::State<SuspendBridge>,
  Query(UnsuspendParams { token }): Query<UnsuspendParams>,
) -> Result<Response, AppError> {
  let ok = u128::from_str_radix(&token, 16)
    .ok()
    .and_then(|key| suspend_bridge.take_sender(key))
    .and_then(|tx| tx.send(()).ok())
    .is_some();

  if ok {
    Response::builder()
      .header("content-type", "text/plain; charset=utf-8")
      .body("ok\n".into())
      .map_err(|err| err.into())
  } else {
    Response::builder()
      .status(axum::http::StatusCode::BAD_REQUEST)
      .header("content-type", "text/plain; charset=utf-8")
      .body("fail\n".into())
      .map_err(|err| err.into())
  }
}

#[derive(Clone)]
pub struct SuspendBridge {
  map: Arc<Mutex<HashMap<u128, oneshot::Sender<()>>>>,
}

impl SuspendBridge {
  pub fn new() -> Self {
    Self { map: Arc::new(Mutex::new(HashMap::new())) }
  }

  pub fn get_suspender(&self) -> Suspender<'_> {
    let mut rnd = [0; _];
    rand_bytes(&mut rnd).unwrap(); // TODO no unwrap
    let key = u128::from_be_bytes(rnd);
    let (tx, rx) = oneshot::channel();
    // TODO HashMap::try_insert
    self.map.lock().unwrap().insert(key, tx);
    Suspender { rx, key, container: self }
  }

  fn take_sender(&self, key: u128) -> Option<oneshot::Sender<()>> {
    self.map.lock().unwrap().remove(&key)
  }
}

pub struct Suspender<'a> {
  key: u128,
  container: &'a SuspendBridge,
  rx: oneshot::Receiver<()>,
}

impl Suspender<'_> {
  pub fn token(&self) -> impl std::fmt::Display {
    std::fmt::from_fn(|f| write!(f, "{:032x}", self.key))
  }
  pub async fn unsuspended(mut self) {
    // TODO error is possible when duplicate key generated
    // and tx is replaced by HashMap::insert and dropped
    (&mut self.rx).await.unwrap();
  }
}

impl Drop for Suspender<'_> {
  fn drop(&mut self) {
    self.container.take_sender(self.key);
  }
}

/*
## pagination

the client cannot keep millions of rows,
and the user also needs control over traffic volume.
Therefore, we need a pagination mechanism.

Using backpressure on the client side does not work because fetch
reads the entire response body uncontrollably, ignoring highWatermark
(as of 2026-Apr-30, verified in Firefox and Chrome).

We also need a commit confirmation mechanism,
which requires precise control over the pause point in the request handler code.

Therefore, we implement pagination explicitly on the server. Options:

- in-process lock hashmap (local state, breaks horizontal scaling)
- use a separate PostgreSQL connection and use LISTEN to wait for
  /api/unsuspend to notify /api/run through another pg connection
  (cons — two additional connections,
  increased surface area of the PostgreSQL API usage, and
  reduced compatibility with PostgreSQL-compatible databases)


It may be better to take a completely different approach:
terminate the response at the suspension point instead of pausing it.
This would eliminate the requirement to disable response buffering
in intermediate proxies. With the current protocol, if a proxy buffers
the response, the client cannot react to either `suspended` or `copyout`.
However, this approach introduces another problem: how to detect
that the client has disconnected so the request can be canceled
and the PostgreSQL connection released as early as possible.
It would also require designing a separate cancellation mechanism
for the ABORT button.

*/
