mod psqlscan;

use axum::BoxError;
use bytes::{Bytes, BytesMut};
use futures_util::stream;
use openssl::rand::rand_bytes;
use serde::Deserialize;
use tokio::sync::mpsc;

use std::ffi::{CStr, CString};
use std::fmt::Write;
use std::iter;

use crate::{AppError, pg};
use psqlscan::statement_boundary;

#[derive(Deserialize)]
pub struct WakeParams {
  id: String,
}

// TODO rename api_resume, api_ack, api_scroll
pub async fn api_wake(
  axum::extract::State(notifier): axum::extract::State<Notifier>,
  axum::extract::Query(WakeParams { id }): axum::extract::Query<WakeParams>,
) -> Result<axum::response::Response, AppError> {
  if let Ok(id) = u128::from_str_radix(&id, 16) {
    notifier.notify(id);
  }
  axum::response::Response::builder()
    .header("content-type", "application/json")
    .body("{\"ok\":true}\n".into())
    .map_err(|err| err.into())
}

#[derive(Deserialize, Debug)]
pub struct RunParams {
  pub db: CString,
  pub tz: CString,
}

pub async fn api_run(
  axum::extract::State(notifier): axum::extract::State<Notifier>,
  axum::extract::Extension(pgctor): axum::extract::Extension<pg::Connector>,
  axum::extract::Query(RunParams { db, tz }): axum::extract::Query<RunParams>,
  // TODO how to accept large sql dump?
  // - separate request for duplex streaming
  // - split into chunks (or statements) on client and do request per chunk.
  //    This will help bypass the default nginx request size limit.
  //    However, we may run into the request rate limit.
  script: Bytes,
) -> Result<axum::response::Response, AppError> {
  // TODO session bound, so user can wake only own tasks, session_id + pid
  // TODO rename job_id, task_id, run_id, script_id (should not use wake to not confuse with future wakers)
  let wake_id = {
    let mut rnd = [0; _];
    rand_bytes(&mut rnd)?;
    u128::from_be_bytes(rnd)
  };

  let (tx, mut rx) = mpsc::channel::<Bytes>(1);

  let mut msgw = MessageWriter {
    wake_id,
    notifier,
    tx: tx.clone(),
    buf: BytesMut::with_capacity(16 * 1024),
  };

  let pgctor = pgctor
    // .with(c"statement_timeout", c"1h")
    .with(c"timezone", tz) // TODO should not fail if invalid timezone
    .with(c"database", db);

  let mut pgconn = match pgctor.connect().await {
    Ok(pgconn) => pgconn,
    Err(err) if err.is_authorized() => {
      msgw.write_error(err.inner.as_ref());
      let body = msgw.buf.freeze();
      return axum::response::Response::builder()
        .header("content-type", "text/x-ndjson; charset=utf-8")
        .body(body.into())
        .map_err(|err| err.into());
    }
    // 28000 role \"xxxx\" does not exist
    // 28P01 password authentication failed for user \"xxx\"
    // TODO invalid password (utf8, nul byte) has no code (backend originated)
    Err(err) if err.is_bad_credentials() => {
      return axum::response::Response::builder()
        .extension(AppError::from(err)) // log error
        .status(axum::http::StatusCode::UNAUTHORIZED)
        .header("content-type", "text/plain; charset=utf-8")
        .body("authentication token is no longer valid\n".into())
        .map_err(|err| err.into());
    }
    Err(err) => {
      return axum::response::Response::builder()
        .extension(AppError::from(err)) // log error
        .status(axum::http::StatusCode::SERVICE_UNAVAILABLE)
        .header("content-type", "text/plain; charset=utf-8")
        .body("service unavailable\n".into())
        .map_err(|err| err.into());
    }
  };

  // TODO detachable background task.
  tokio::spawn(async move {
    tokio::select! {
      _ = tx.closed() => {
        log::debug!("client gone, sending CancelRequest to postgres");
        let _ = pgconn.cancel().await; // TODO timeout
      }
      res = api_run_inner(&mut pgconn, &mut msgw, &script) => {
        if let Err(err) = res {
          log::error!("failed to execute script: {err}"); // TODO log some request context
          msgw.write_error(err.as_ref());
        }
        msgw.end().await;
      }
    }
    let _ = pgconn.close().await;
  });

  let s = stream::poll_fn(move |cx| {
    rx.poll_recv(cx).map(|chunk| match chunk {
      Some(fin) if fin.is_empty() => None, // send final chunk `0\r\n\r\n`
      Some(data) => Some(Ok(data)),
      // TODO log error?
      None => Some(Err("incompete response")), // drop connection
    })
  });
  // TODO consider https://www.rfc-editor.org/rfc/rfc7464
  axum::response::Response::builder()
    // https://ndjson.com/faq/ recommends `application/x-ndjson`
    // .header("content-type", "application/x-ndjson; charset=utf-8")
    // but `application/x-ndjson` is displayed as base64 in firefox devtools, not debuggable
    .header("content-type", "text/x-ndjson; charset=utf-8")
    .header("cache-control", "no-transform") // disable gzip buffering
    .header("x-accel-buffering", "no") // disable nginx response buffering
    .body(axum::body::Body::from_stream(s))
    .map_err(|err| err.into())
}

async fn api_run_inner(
  pgconn: &mut pg::Connection,
  msgw: &mut MessageWriter,
  script: &[u8],
) -> Result<(), BoxError> {
  // TODO parse \connect "database" here
  // But how to do run selection? whitespace padding? (actual current solution btw, because of statement position)
  // let db_json = json_escape_lossy(b"example");
  // write!(resp_buf, "[\"db\", \"{db_json}\"]\n").unwrap();

  let mut enrich_rowdescr_stmt_prepared = false;
  let mut rowdescr = String::with_capacity(8 * 1024);

  let mut n_bytes_sent = 0;
  let mut n_rows_written = 0;
  let mut no_stmt_emitted = true;
  let mut stmt_pos_utf16;

  let mut script = {
    // TODO trim comments
    let trimmed = script.trim_ascii_start();
    // ascii only, single utf16 code units
    stmt_pos_utf16 = script.len() - trimmed.len();
    trimmed
  };

  let statements = std::iter::from_fn(|| {
    let pos = statement_boundary(script);
    let stmt;
    (stmt, script) = script.split_at(pos);
    Some(stmt).filter(|s| !s.is_empty())
  });

  for stmt in statements {
    log::debug!("executing statement \"{}\"", stmt.escape_ascii());

    let stmt = pg::NZStr::try_from(stmt)
      // TODO return position_utf16 for nul char?
      .map_err(|_| "script must not contain NUL characters")?;

    msgw.write_start(stmt_pos_utf16);

    rowdescr.clear();
    // TODO parse, bind, describe_portal
    // so parametrized queries will cause error eager, before enrich_rowdescr
    // also avoid ParameterDescription message.
    // The stopper is that portals does not outlive Sync
    //   с другой стороны, сейчас мы можем увидеть шапку таблицы
    //  прежде чем получим ошибку о неуказаных параметрах.
    //  Это может быть полезно или наоборот вредно?
    pgconn.send_parse(c"", &[], stmt);
    pgconn.send_describe_stmt(c"");
    // TODO we can use RowDescription/NoData as terminator, `Describe` should not generate notices
    pgconn.send_close_portal(); // because of log_duration NoticeResponse, TODO better comment
    pgconn.send_flush();
    let rowdescr = loop {
      match pgconn.recv_message().await {
        Ok(pg::NoData) => {}
        Ok(pg::RowDescription(fields)) => {
          // TODO assert field.format == 0 (text) or allow binary datum
          // TODO assert single RowDescription ?
          // TODO: PostgreSQL can return non-UTF-8 field names,
          // so it's better to write field names as-is.
          // Postgres should be able to decode the names because postgres produced them itself.
          // Wrapping non-UTF-8 strings in JSON should be safe.
          // When client_encoding != UTF-8, PostgreSQL returns/accepts non-UTF-8 JSON.
          write!(rowdescr, "{}", json_rowdescr(&fields)).unwrap();
        }
        Ok(pg::NoticeResponse(notice)) => msgw.write_notice(&notice),
        // Ok(pg::ParameterDescription(..)) => {
        //   // script should have no parameters:
        //   // error will be genereated by following Bind
        // }
        Ok(pg::CloseComplete) => break rowdescr.as_bytes(),
        Ok(_) => {}
        Err(err) => return Err(expose_dberror(err)),
      }
    };

    if !rowdescr.is_empty() {
      // TODO fix user script interference `DEALLOCATE ALL, DISCARD ALL`,
      // We shoud reparse statement every time but its noisy and more expensive.
      // Protecting from DEALOCATE/PREPARE pgbb_enrich_rowdescr makes no sence,
      // because there are also many other ways to break the things intentionally.
      const ENRICH_ROWDESCR_STNAME: &CStr = c"pgbb_enrich_rowdescr";
      if !enrich_rowdescr_stmt_prepared {
        const ENRICH_ROWDESCR_SQL: pg::NZStr<'_> =
          pg::NZStr::from_bytes(include_bytes!("./enrich_rowdescr.sql"))
            .unwrap();
        pgconn.send_parse(
          ENRICH_ROWDESCR_STNAME,
          &[pg::JSON_OID],
          ENRICH_ROWDESCR_SQL,
        );
        enrich_rowdescr_stmt_prepared = true;
      }

      pgconn.send_bind_bin(ENRICH_ROWDESCR_STNAME, &[Some(rowdescr)]);
      pgconn.send_execute(); // TODO limit 1
      if no_stmt_emitted {
        // The first statement may be non-transactional
        // and may rely on being executed at the beginning of a implicit transaction,
        // therefore we should complete the transaction
        // that we implicitly opened by executing pgbb_enrich_rowdescr.
        // Although I currently don't know which queries that
        // returns RowDescription are non-transactional.
        // https://github.com/search?q=repo%3Apostgres%2Fpostgres+PreventInTransactionBlock&type=code
        pgconn.send_sync();
      } else {
        pgconn.send_close_portal();
        pgconn.send_flush();
      }

      loop {
        let row = match pgconn.recv_message().await {
          Ok(pg::DataRow(row)) => row, // TODO assert only one DataRow?
          Ok(pg::CloseComplete | pg::ReadyForQuery { .. }) => break,
          Ok(_) => continue,
          Err(err) => {
            return Err(
              format!("failed to enrich row description: {err}").into(),
            );
          }
        };
        // TODO do not flatten - error hiding
        let head_payload = row.into_iter().next().flatten().unwrap_or(b"null");
        msgw.write_head(head_payload);
      }
    }

    pgconn.send_bind(c"", &[]); // user statement
    pgconn.send_execute();
    pgconn.send_flush();
    no_stmt_emitted = false;

    loop {
      // TODO fix hardcoded limits,  max_page_bytes, max_page_rows, GUC? querystring?
      if n_bytes_sent + msgw.len() >= 5 * 1024 * 1024 // 5Mib
        || n_rows_written >= 1000_u32
      {
        log::debug!("suspended on page boundary");
        // TODO better name for traffic_limit_exceeded, there is a per page limit
        msgw.flush(Some("traffic_limit_exceeded")).await;
        n_bytes_sent = 0;
        n_rows_written = 0;
      }

      // TODO consider FutureExt::now_or_never instead of pgconn.is_drained()
      if pgconn.is_drained() && msgw.len() > 0 {
        n_bytes_sent += msgw.len();
        msgw.flush(None).await;
      }

      let heartbeat_interval = std::time::Duration::from_secs(10); // TODO fix hardcode
      let msg_fut = pgconn.recv_message();
      let msg_fut = tokio::time::timeout(heartbeat_interval, msg_fut);
      let Ok(msg_res) = msg_fut.await else {
        // TODO should not count "alive" messages to n_bytes_sent?
        msgw.write_alive_if_drained();
        continue;
      };

      match msg_res.map_err(expose_dberror)? {
        pg::DataRow(row) => {
          msgw.write_row(row);
          n_rows_written += 1;
        }
        pg::CopyData(_data) => {
          // prefer not rely on CopyData contains exact one row
          // TODO should open separate channel,
          // should not stream copy output through message stream
          // because fetch gives no control over backpressure in js space
          // TODO we can wrap mutliple copy outputs to tar/zip file
          // in case of multiple COPY queries.
          // but symmetry between COPY TO / COPY FROM will be broken
        }
        pg::CopyInResponse(..) => {
          pgconn.send_copy_fail();
          // TODO COPY .. FROM STDIN support
          // we should send some ["copyin", ..] message to client,
          // client should do new request with file,
          // we should access request body stream here somehow and pipe it to pg
        }
        pg::CopyOutResponse(..) => {} // TODO error if binary
        pg::CopyDone => {}

        // pg::BindComplete => {}
        // pg::ParameterStatus { .. } => {}
        // pg::NotificationResponse { .. } => {} // TODO ignore?
        pg::NoticeResponse(notice) => msgw.write_notice(&notice),

        | pg::EmptyQueryResponse { tag } // TODO avoid executing empty queries to avoid synthetic "EMPTY QUERY"?
        | pg::PortalSuspended { tag } // impossible
        | pg::CommandComplete { tag } => {
          msgw.write_complete(tag.to_bytes());
          pgconn.send_close_portal(); // because of log_duration NoticeResponse, TODO better comment
          pgconn.send_flush();
        }

        pg::CloseComplete => break,

        _ => {}
      }
    }

    stmt_pos_utf16 += str::from_utf8(&stmt)
      // TODO its not ok to report error after succesfull executing invalid query,
      //  but we dont need utf8 decoding for execution.
      //  We should return `null` position after invalid utf8
      .map_err(|_| "script must be valid UTF-8")?
      .encode_utf16()
      .count();
  }

  let check_for_changes_query = c"\
    select null \
    where pg_catalog.txid_current_if_assigned() is not null"
    .into();
  let mut has_changes = false;
  pgconn.send_parse(c"", &[], check_for_changes_query);
  pgconn.send_bind(c"", &[]);
  pgconn.send_execute();
  pgconn.send_close_portal();
  pgconn.send_flush();
  let has_changes = loop {
    match pgconn.recv_message().await {
      Ok(pg::DataRow { .. }) => has_changes = true,
      Ok(pg::CloseComplete) => break has_changes,
      Ok(_) => {}
      Err(err) => {
        return Err(
          format!("failed to check for uncommited changes: {err}").into(),
        );
      }
    }
  };

  // TODO If the transaction is explicit, there's no point in asking the user
  // for Sync confirmation, since Sync does not commit explicit transactions.
  // Need to somehow determine whether the transaction is explicit or not.
  if has_changes {
    log::debug!("suspended idle_in_transaction");
    msgw.flush(Some("idle_in_transaction")).await;
  }

  pgconn.send_sync();
  loop {
    match pgconn.recv_message().await {
      // deferred constraints/triggers can raise notices and errors on commit
      Ok(pg::NoticeResponse(notice)) => msgw.write_notice(&notice),
      Ok(pg::ReadyForQuery { .. }) => break,
      Ok(_) => {} // TODO ParameterStatus, NotificationResponse, what else?
      Err(err) => return Err(expose_dberror(err)),
    }
  }

  Ok(())
}

fn expose_dberror(err: tokio::io::Error) -> BoxError {
  match err.downcast::<pg::DbError>() {
    Ok(dberr) => dberr.into(),
    Err(err) => err.into(),
  }
}

// RunContext?
struct MessageWriter {
  buf: BytesMut,
  tx: mpsc::Sender<Bytes>,
  wake_id: u128,
  notifier: Notifier,
}

impl MessageWriter {
  fn write_start(&mut self, pos_utf16: usize) {
    write!(self.buf, "[\"start\", {{\"position_utf16\": {pos_utf16}}}]\n")
      .unwrap();
  }

  fn write_head(&mut self, payload: &[u8]) {
    // TODO better message name head -> meta, row_description, columns, schema
    self.buf.extend_from_slice(b"[\"head\", ");
    self.buf.extend_from_slice(payload);
    self.buf.extend_from_slice(b"]\n");
  }

  fn write_row(&mut self, row: pg::Row<'_>) {
    // TODO may be we should be able to provide binary values?
    // how to mark datum as binary?
    //  - array wrapper
    //  - row prefix
    //  - zero char suffix (fragile)

    // ["row", ["this is valid utf8 text", "this too", ["hexencoded"], "last text cell"]]
    let elems = std::fmt::from_fn(|f| {
      let commas = iter::chain([""], iter::repeat(", "));
      for (val, comma) in row.clone().zip(commas) {
        write!(f, "{comma}")?;
        match val {
          None => write!(f, "null"),
          Some(val) => write!(f, "\"{}\"", json_escape_lossy(val)),
        }?;
      }
      Ok(())
    });
    write!(self.buf, "[\"row\", [{elems}]]\n").unwrap();
  }

  fn write_complete(&mut self, tag: &[u8]) {
    let payload = json_escape_lossy(tag);
    write!(self.buf, "[\"complete\", \"{payload}\"]\n").unwrap();
  }

  fn write_notice(&mut self, notice: &pg::DbError<'_>) {
    let payload = dberror_json(&notice);
    write!(self.buf, "[\"notice\", {payload}]\n").unwrap();
  }

  fn write_alive_if_drained(&mut self) {
    if self.buf.is_empty() {
      // TODO may be we can just emit 0x20 space?
      // so LineDecoder will decode it as empty array of lines
      // "[]" | "null" | "\x20"
      write!(self.buf, "[\"alive\"]\n").unwrap();
    }
  }

  fn write_error(&mut self, err: &(dyn std::error::Error + 'static)) {
    // TODO position_utf16
    // use std::error::Error as _;
    if let Some(dberr) = err.downcast_ref() {
      let payload = dberror_json(dberr);
      write!(self.buf, "[\"error\", {payload}]\n").unwrap();
    } else {
      let payload = error_json(err);
      write!(self.buf, "[\"error\", {payload}]\n").unwrap();
    }
  }

  fn len(&self) -> usize {
    self.buf.len()
  }

  async fn flush(&mut self, suspend_reason: Option<&str>) {
    // use .reserve() so we can safely mutate self.buf after `await`;
    // we don't need cancellation safety right now, but keep it just in case.
    // TODO timeout?. how axum/hyper handles physicaly broken connection?
    let Ok(permit) = self.tx.reserve().await else {
      // let tx.closed() win and pgconn.cancel() called
      let () = std::future::pending().await;
      unreachable!();
    };
    if let Some(suspend_reason) = suspend_reason {
      let suspend_reason = json_escape(suspend_reason);
      let wake_id = self.wake_id;
      write!(
        self.buf,
        "[\"suspended\", {{\
        \"reason\": \"{suspend_reason}\",\
        \"wake_id\": \"{wake_id:032x}\"}}]\n"
      )
      .unwrap();
    }
    if !self.buf.is_empty() {
      // not final
      let resp_chunk = self.buf.split().freeze();
      permit.send(resp_chunk);
    }
    if suspend_reason.is_some() {
      // TODO we should subscribe to notifier before emit "suspended",
      self.notifier.notified(self.wake_id).await;
    }
  }

  async fn end(self) {
    let leftover = self.buf.freeze();
    if !leftover.is_empty() {
      let _ = self.tx.send(leftover).await;
    }
    // send an acknowledgment that the response has completed
    // so the client doesn't observe a successful response completion
    // if the task panics
    let fin = Bytes::new();
    let _ = self.tx.send(fin).await;
  }
}

fn json_rowdescr(fields: &[pg::Field<'_>]) -> impl std::fmt::Display {
  std::fmt::from_fn(|f| {
    write!(f, "[")?;
    let commas = iter::chain([""], iter::repeat(","));
    for (field, comma) in fields.iter().zip(commas) {
      let pg::Field { name, table_oid, table_col, type_oid, type_mod, .. } =
        field;
      let name = json_escape_lossy(name.to_bytes());
      write!(
        f,
        "{comma}{{\
        \"name\":\"{name}\",\
        \"table_oid\":{table_oid},\
        \"table_col\":{table_col},\
        \"type_oid\":{type_oid},\
        \"type_mod\":{type_mod}}}"
      )?;
    }
    write!(f, "]")
  })
}

fn error_json(err: impl std::fmt::Display) -> impl std::fmt::Display {
  std::fmt::from_fn(move |f| {
    let msg = err.to_string();
    let msg = json_escape(&msg); // TODO accept impl Display
    write!(f, "{{\"message\": \"{msg}\"}}")
  })
}

fn dberror_json(dberr: &pg::DbError) -> impl std::fmt::Display {
  std::fmt::from_fn(move |f| {
    write!(f, "{{")?;
    let commas = iter::chain([""], iter::repeat(", "));
    for ((key, val), comma) in dberr.fields.iter().zip(commas) {
      let val = json_escape_lossy(val.to_bytes());
      // TODO escape key, json_escape(impl Display)
      write!(f, "{comma}\"{key}\": \"{val}\"")?;
    }
    write!(f, "}}")
  })
}

fn json_escape_lossy(inp: &[u8]) -> impl std::fmt::Display {
  std::fmt::from_fn(|f| {
    for chunk in inp.utf8_chunks() {
      let valid = json_escape(chunk.valid());
      write!(f, "{valid}")?;
      if !chunk.invalid().is_empty() {
        // write!(f, "{}", chunk.invalid().escape_ascii())?;
        f.write_char(char::REPLACEMENT_CHARACTER)?;
      }
    }
    Ok(())
  })
}

// TODO accept impl Display
fn json_escape(inp: &str) -> impl std::fmt::Display {
  // fn need_escape(ch: char) -> bool {
  //   ch.is_ascii_control() || ch == '"' || ch == '\\'
  // }
  std::fmt::from_fn(|f| {
    for ch in inp.chars() {
      match ch {
        '"' | '\\' | '\n' | '\r' | '\t' => write!(f, "{}", ch.escape_default()),
        ch if ch.is_ascii_control() => write!(f, "\\u{:04x}", u32::from(ch)),
        safe => f.write_char(safe), // TODO slow?
      }?;
    }

    // for chunk in inp.split_inclusive(need_escape) {
    //   // chunk cannot be empty
    //   if need_escape(chunk.chars().last().unwrap()) {
    //     let (safe, tail) = chunk.split_at(chunk.len() - 1);
    //     // let tail = tail.as_bytes().escape_ascii();
    //     let tail = tail.escape_default();
    //     write!(f, "{safe}{tail}")?;
    //   } else {
    //     write!(f, "{chunk}")?;
    //   }
    // }
    Ok(())
  })
}

#[derive(Clone)]
pub struct Notifier {
  tx: tokio::sync::broadcast::Sender<u128>,
}

impl Notifier {
  pub fn new() -> Self {
    // TODO why not 1? is 16 enough?
    let tx = tokio::sync::broadcast::Sender::new(16);
    Self { tx }
  }

  fn notify(&self, id: u128) {
    let _ = self.tx.send(id);
  }

  async fn notified(&self, id: u128) {
    use tokio::sync::broadcast::error::RecvError;
    let mut rx = self.tx.subscribe();
    loop {
      match rx.recv().await {
        Ok(notified_id) if notified_id == id => break,
        Ok(_) => {}
        // message missed, user need to press MORE again
        Err(RecvError::Lagged(_)) => {}
        Err(RecvError::Closed) => {
          todo!("handle notifier closed");
        }
      }
    }
  }
}

/*
## paging

the client cannot keep millions of rows,
and the user also needs control over traffic volume.
Therefore, we need a paging mechanism.

Using backpressure on the client side does not work because fetch
reads the entire response body uncontrollably, ignoring highWatermark
(as of 2026-Apr-30, verified in Firefox and Chrome).

We also need a commit confirmation mechanism,
which requires precise control over the pause point in the request handler code.

Therefore, we implement paging explicitly on the server. Options:

- in-process lock hashmap (local state, breaks horizontal scaling)
- use a separate PostgreSQL connection and use LISTEN to wait for
  /api/wake to notify /api/run through another pg connection
  (cons — two additional connections,
  increased surface area of the PostgreSQL API usage, and
  reduced compatibility with PostgreSQL-compatible databases)
*/

/*

curl -v --raw '192.168.110.58:7890/api/auth' -d 'user=postgres&password='

{"ok":true,"token":"N3NShQFVqcWoFEDNC6pvOCNmj0DBD/QMRvtgfg=="}

curl -v --raw -H 'x-pgbb-auth: N3NShQFVqcWoFEDNC6pvOCNmj0DBD/QMRvtgfg==' '192.168.110.58:7890/api/run?user=postgres&db=postgres&tz=asia/almaty' -d 'select 1'

 */
