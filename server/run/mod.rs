mod copyout;
mod psqlscan;
mod unsuspend;

use axum::BoxError;
use bytes::{Bytes, BytesMut};
use futures_util::stream;
use serde::Deserialize;
use tokio::sync::mpsc;

use std::ffi::{CStr, CString};
use std::fmt::{Display, Write};
use std::iter;

use crate::{AppError, pg};
use psqlscan::statement_boundary;

pub use copyout::{CopyoutBridge, api_copyout};
pub use unsuspend::{SuspendBridge, api_unsuspend};

#[derive(Deserialize, Debug)]
pub struct RunParams {
  pub db: CString,
  pub tz: CString,
}

pub async fn api_run(
  suspend_bridge: axum::extract::State<SuspendBridge>,
  copyout_bridge: axum::extract::State<CopyoutBridge>,
  axum::extract::Extension(pgctor): axum::extract::Extension<pg::Connector>,
  axum::extract::Query(RunParams { db, tz }): axum::extract::Query<RunParams>,
  // TODO how to accept large sql dump?
  // - separate request for duplex streaming
  // - split into chunks (or statements) on client and do request per chunk.
  //    This will help bypass the default nginx request size limit.
  //    However, we may run into the request rate limit.
  script: Bytes,
) -> Result<axum::response::Response, AppError> {
  let (tx, mut rx) = mpsc::channel::<Bytes>(1);

  let buf = BytesMut::with_capacity(16 * 1024);
  let mut msgw = MessageWriter { tx: tx.clone(), buf };

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
      res = run_impl(&mut pgconn, &mut msgw, &suspend_bridge, &copyout_bridge, &script) => {
        if let Err(err) = res {
          log::error!("failed to execute script: {err}"); // TODO log some request context
          msgw.write_error(err.as_ref());
        }
        msgw.end().await;
      }
    }
    let _ = pgconn.close().await;
  });

  // 15sec is common practise for SSE keepalive interval
  // TODO do not hardcode
  let heartbeat_interval = std::time::Duration::from_secs(15);
  let mut idle_timer = Box::pin(tokio::time::sleep(heartbeat_interval));
  let s = stream::poll_fn(move |cx| {
    use std::task::Poll;
    let res = match rx.poll_recv(cx) {
      Poll::Ready(val) => match val {
        Some(fin) if fin.is_empty() => None, // send final chunk `0\r\n\r\n`
        Some(chunk) => Some(Ok(chunk)),
        None => Some(Err("incomplete response")), // drop connection, TODO log error?
      },
      Poll::Pending => match idle_timer.as_mut().poll(cx) {
        Poll::Pending => return Poll::Pending,
        // TODO consider emit just 0x20 space
        // so LineDecoder will decode it as empty array of lines
        Poll::Ready(_) => Some(Ok(Bytes::from_static(b"[\"alive\"]\n"))),
      },
    };
    idle_timer.as_mut().reset(tokio::time::Instant::now() + heartbeat_interval);
    Poll::Ready(res)
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

async fn run_impl(
  pgconn: &mut pg::Connection,
  msgw: &mut MessageWriter,
  suspend_bridge: &SuspendBridge,
  copyout_bridge: &CopyoutBridge,
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
        let head_payload = String::from_utf8_lossy(head_payload);
        msgw.write_head(&head_payload);
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
        // TODO better name for traffic_limit_exceeded, there is a per page limit
        // end_of_page
        // log::debug!("suspended on page boundary");
        let susp = suspend_bridge.get_suspender();
        msgw.write_suspended("traffic_limit_exceeded", susp.token());
        msgw.flush().await;
        susp.unsuspended().await;

        n_bytes_sent = 0;
        n_rows_written = 0;
      } else if pgconn.is_drained() && msgw.len() > 0 {
        // TODO consider FutureExt::now_or_never instead of pgconn.is_drained()
        n_bytes_sent += msgw.len();
        msgw.flush().await;
      }

      let msg_res = pgconn.recv_message().await;
      match msg_res.map_err(expose_dberror)? {
        pg::DataRow(row) => {
          msgw.write_row(row);
          n_rows_written += 1;
        }
        pg::CopyInResponse(..) => {
          pgconn.send_copy_fail();
          // TODO COPY .. FROM STDIN support
          // we should send some ["copyin", ..] message to client,
          // client should do new request with file,
          // we should access request body stream here somehow and pipe it to pg
        }
        pg::CopyOutResponse(_fmt) => {
          // TODO generate filename source_TIMESTAMP.tsv
          let cout = copyout_bridge.open();
          msgw.write_copyout(cout.token());
          n_bytes_sent += msgw.len();
          msgw.flush().await;
          cout.pump(pgconn).await.map_err(expose_dberror)?;
        }
        // pg::BindComplete => {}
        // pg::ParameterStatus { .. } => {}
        // pg::NotificationResponse { .. } => {} // TODO ignore?
        pg::NoticeResponse(notice) => msgw.write_notice(&notice),

        | pg::EmptyQueryResponse { tag } // TODO avoid executing empty queries to avoid synthetic "EMPTY QUERY"?
        // TODO consider using protocol level row limit,
        // so statetement_timeout will not cause abort when execution suspended
        | pg::PortalSuspended { tag } // impossible
        | pg::CommandComplete { tag } => {
          msgw.write_complete(tag.to_bytes());
          pgconn.send_close_portal(); // because of log_duration NoticeResponse, TODO better comment
          pgconn.send_flush();
        }

        pg::CloseComplete => break,

        _ => {}
      }
    } // loop

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
    let susp = suspend_bridge.get_suspender();
    msgw.write_suspended("idle_in_transaction", susp.token());
    msgw.flush().await;
    susp.unsuspended().await;
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
}

impl MessageWriter {
  fn write_start(&mut self, pos_utf16: usize) {
    write!(self.buf, "[\"start\", {{\"position_utf16\": {pos_utf16}}}]\n")
      .unwrap();
  }

  fn write_head(&mut self, payload: &str) {
    // TODO better message name head -> meta, row_description, columns, schema
    write!(self.buf, "[\"head\", {payload}]\n").unwrap();
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
      row.clone().zip(commas).try_for_each(|(val, comma)| match val {
        None => write!(f, "{comma}null"),
        Some(val) => write!(f, "{comma}\"{}\"", json_escape_lossy(val)),
      })
    });
    write!(self.buf, "[\"row\", [{elems}]]\n").unwrap();
  }

  fn write_copyout(&mut self, id: impl Display) {
    // TODO escape id
    write!(self.buf, "[\"copyout\", {{\"id\": \"{id}\"}}]\n").unwrap();
  }

  fn write_complete(&mut self, tag: &[u8]) {
    let payload = json_escape_lossy(tag);
    write!(self.buf, "[\"complete\", \"{payload}\"]\n").unwrap();
  }

  fn write_notice(&mut self, notice: &pg::DbError<'_>) {
    let payload = dberror_json(&notice);
    write!(self.buf, "[\"notice\", {payload}]\n").unwrap();
  }

  fn write_error(&mut self, err: &(dyn std::error::Error + 'static)) {
    if let Some(dberr) = err.downcast_ref() {
      let payload = dberror_json(dberr);
      write!(self.buf, "[\"error\", {payload}]\n").unwrap();
    } else {
      let payload = error_json(err);
      write!(self.buf, "[\"error\", {payload}]\n").unwrap();
    }
  }

  fn write_suspended(&mut self, reason: &str, token: impl Display) {
    let reason = json_escape(reason);
    // TODO escape token
    write!(
      self.buf,
      "[\"suspended\", {{\
      \"reason\": \"{reason}\",\
      \"token\": \"{token}\"}}]\n",
    )
    .unwrap();
  }

  fn len(&self) -> usize {
    self.buf.len()
  }

  async fn flush(&mut self) {
    // use .reserve() so we can safely mutate self.buf after `await`;
    // we don't need cancellation safety right now, but keep it just in case.
    // TODO timeout?. how axum/hyper handles physicaly broken connection?
    let Ok(permit) = self.tx.reserve().await else {
      // let tx.closed() win and pgconn.cancel() called
      let () = std::future::pending().await;
      unreachable!();
    };
    if !self.buf.is_empty() {
      // not final
      let resp_chunk = self.buf.split().freeze();
      permit.send(resp_chunk);
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

fn json_rowdescr(fields: &[pg::Field<'_>]) -> impl Display {
  std::fmt::from_fn(|f| {
    write!(f, "[")?;
    let commas = iter::chain([""], iter::repeat(","));
    for (field, comma) in fields.iter().zip(commas) {
      let pg::Field { table_oid, table_col, type_oid, type_mod, .. } = field;
      let name = json_escape_lossy(field.name.to_bytes());
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

fn error_json(err: impl Display) -> impl Display {
  std::fmt::from_fn(move |f| {
    let msg = err.to_string();
    let msg = json_escape(&msg); // TODO accept impl Display
    write!(f, "{{\"message\": \"{msg}\"}}")
  })
}

fn dberror_json(dberr: &pg::DbError) -> impl Display {
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

fn json_escape_lossy(inp: &[u8]) -> impl Display {
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
fn json_escape(inp: &str) -> impl Display {
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
