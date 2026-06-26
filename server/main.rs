#![forbid(unsafe_code)]

mod auth;
mod pg;
mod run;
mod ui;

use axum::response::Response;
use serde::Deserialize;

use std::ffi::CString;
use std::sync::Arc;

use run::{CopyoutStore, Notifier, api_run, api_wake, serve_copyout};

fn main() -> Result<(), axum::BoxError> {
  use axum::http::Uri;
  use log::LevelFilter;
  use std::net::SocketAddr;
  use std::str::FromStr;

  let matches = clap::Command::new("pgbb")
    .version(env!("CARGO_PKG_VERSION"))
    .about(env!("CARGO_PKG_DESCRIPTION"))
    .arg(
      clap::Arg::new("pg_uri")
        .help("Postgres connection URI")
        .default_value("postgres://127.0.0.1:5432/")
        // TODO check scheme == "postgres" | "postgresql"
        // TODO assert empty user:password?
        .value_parser(Uri::from_str),
    )
    .arg(
      clap::Arg::new("http_addr")
        .long("http")
        .help("HOST:PORT to listen")
        .default_value("0.0.0.0:7890")
        .value_parser(SocketAddr::from_str),
    )
    .arg(
      clap::Arg::new("verbosity")
        .long("verbosity")
        .help("Max log level filter")
        .default_value("info")
        .value_parser(LevelFilter::from_str),
    )
    .get_matches();

  let pg_uri = matches.get_one::<Uri>("pg_uri").unwrap().clone();
  let http_addr = matches.get_one::<SocketAddr>("http_addr").unwrap().clone();
  let verbosity = matches.get_one::<LevelFilter>("verbosity").unwrap().clone();

  env_logger::Builder::new()
    .filter(None, verbosity)
    .format_level(false)
    .format_target(false)
    .init();

  let pgctor = {
    use axum::extract::Query;

    let pgctor = pg::Connector::new()
      .with(c"database", c"postgres") // defaults to the user name
      .with(c"client_min_messages", c"NOTICE")
      .with(c"application_name", c"pgbb");

    // https://www.postgresql.org/docs/current/libpq-connect.html#LIBPQ-CONNSTRING-URIS

    // TODO default database from uri.path() ?
    let host = pg_uri.host().unwrap_or("127.0.0.1");
    let port = pg_uri.port_u16().unwrap_or(5432);
    let pgctor = pgctor.with_addr(host, port);
    let qs: Vec<(CString, CString)> = Query::try_from_uri(&pg_uri)?.0;
    let pgctor =
      qs.into_iter().fold(pgctor, |pgctor, (k, v)| pgctor.with(k, v));
    pgctor.with(c"client_encoding", c"UTF8") // force utf8
  };

  let auth = auth::Authenticator::new()?;
  let notifier = Notifier::new();
  let copyouts = CopyoutStore::new();
  let state = Arc::new(AppState { pgctor, auth, notifier, copyouts });

  // TODO request body size limit https://docs.rs/tower-http/latest/tower_http/limit/index.html
  let app = axum::Router::new()
    .route("/api/run", axum::routing::post(api_run))
    .route("/api/wake", axum::routing::post(api_wake))
    .route("/api/tree", axum::routing::post(api_tree))
    .route("/api/defn", axum::routing::post(api_defn))
    .route_layer(axum::middleware::from_fn_with_state(
      state.clone(),
      require_auth,
    ))
    // public routes
    .route("/api/auth", axum::routing::post(api_auth))
    .route("/copyout/{id}", axum::routing::get(serve_copyout))
    .route("/favicon.ico", axum::routing::get(serve_favicon_ico))
    .fallback(ui::serve_ui)
    .layer(axum::middleware::from_fn(log_request))
    .with_state(state);

  let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;

  rt.block_on(async move {
    let listener = tokio::net::TcpListener::bind(http_addr).await?;
    log::info!("serving \"{pg_uri}\" on {http_addr}");
    axum::serve(listener, app)
      // TODO long script can block shutdown (apply timeout?)
      .with_graceful_shutdown(on_sigint_or_sigterm())
      .await
  })?;

  Ok(())
}

async fn on_sigint_or_sigterm() {
  use tokio::signal;

  let ctrl_c = async {
    signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
  };

  #[cfg(unix)]
  let terminate = async {
    signal::unix::signal(signal::unix::SignalKind::terminate())
      .expect("failed to install SIGTERM handler")
      .recv()
      .await;
  };

  #[cfg(not(unix))]
  let terminate = std::future::pending::<()>();

  tokio::select! {
    _ = ctrl_c => log::info!("Ctrl+C received"),
    _ = terminate => log::info!("SIGTERM received"),
  }
}

struct AppState {
  pgctor: pg::Connector,
  auth: auth::Authenticator,
  notifier: Notifier,
  copyouts: CopyoutStore,
}

impl axum::extract::FromRef<Arc<AppState>> for Notifier {
  fn from_ref(state: &Arc<AppState>) -> Self {
    state.notifier.clone()
  }
}

impl axum::extract::FromRef<Arc<AppState>> for CopyoutStore {
  fn from_ref(state: &Arc<AppState>) -> Self {
    state.copyouts.clone()
  }
}

#[derive(Clone)] // extensions require Clone
struct AppError {
  inner: Arc<dyn std::error::Error + Send + Sync + 'static>,
}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for AppError {
  fn from(err: E) -> Self {
    AppError { inner: Arc::new(err) }
  }
}

impl axum::response::IntoResponse for AppError {
  fn into_response(self) -> Response {
    Response::builder()
      .extension(self)
      .status(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
      .header("content-type", "text/plain; charset=utf-8")
      .body("internal server error\n".into())
      .expect("error should not occur when constructing 500 response")
  }
}

#[derive(Deserialize)]
struct UserParam {
  user: CString,
}

async fn require_auth(
  axum::extract::State(env): axum::extract::State<Arc<AppState>>,
  axum::extract::Query(UserParam { user }): axum::extract::Query<UserParam>,
  mut req: axum::extract::Request,
  next: axum::middleware::Next,
) -> Response {
  if let Some(token) = req.headers().get("x-pgbb-auth")
    && let Some(password) = env.auth.verify(user.as_bytes(), token.as_bytes())
  {
    let pgctor = env.pgctor.clone().with_credentials(user, password);
    req.extensions_mut().insert(pgctor);
    next.run(req).await
  } else {
    // TODO rate_limit by ip when unauthorized
    Response::builder()
      .status(axum::http::StatusCode::UNAUTHORIZED)
      .header("content-type", "text/plain; charset=utf-8")
      .body("unauthorized\n".into())
      .expect("error should not occur when constructing 401 response")
  }
}

async fn log_request(
  req: axum::extract::Request,
  next: axum::middleware::Next,
) -> Response {
  let method = req.method().clone();
  let uri = req.uri().clone();
  let resp = next.run(req).await;
  // TODO let remote_addr = req.

  let status = resp.status().as_u16();
  let err = resp.extensions().get::<AppError>();
  let err_msg = std::fmt::from_fn(|f| {
    let Some(err) = err else {
      return Ok(());
    };
    write!(f, " {}", err.inner)
  });

  let level = match status {
    400.. => log::Level::Error,
    _ => log::Level::Info,
  };

  log::log!(level, "{method} {uri} {status}{err_msg}");
  resp
}

#[derive(Deserialize)]
struct AuthParams {
  user: CString,
  password: CString,
}

async fn api_auth(
  axum::extract::State(env): axum::extract::State<Arc<AppState>>,
  // TODO this is the only place where we use Form extractor.
  // Weak reason to use entire axum/form feature.
  // Looked into the axum source code:
  // axum/form costs nothing if axum/query is also enabled
  axum::extract::Form(AuthParams { user, password }): axum::extract::Form<
    AuthParams,
  >,
) -> Result<Response, AppError> {
  // TODO ip rate limit

  // TODO reject weak passwords if ?require_auth=!none
  // https://en.wikipedia.org/wiki/List_of_the_most_common_passwords

  // TODO add error context (use anyhow?)
  let token = env.auth.issue(user.as_bytes(), password.as_bytes())?;

  let pgctor = env.pgctor.clone().with_credentials(user, password);

  match pgctor.connect().await {
    Ok(pgconn) => {
      let _ = pgconn.close().await; // TODO log error or handle somehow?

      // TODO default to ?require_auth=!none
      // https://www.postgresql.org/docs/current/libpq-connect.html#LIBPQ-CONNECT-REQUIRE-AUTH

      let body = format!("{{\"token\":\"{token}\"}}\n");
      Response::builder()
        .header("content-type", "application/json")
        .body(body.into())
        .map_err(|err| err.into())
    }

    Err(err) if err.is_bad_credentials() => Response::builder()
      .extension(AppError::from(err))
      .status(axum::http::StatusCode::UNAUTHORIZED)
      // Client should not rely on response body be formatted as JSON
      // on non-200 response because intermediate proxy can respond in any format,
      // Client should read response as text and show it as error message.
      .header("content-type", "text/plain; charset=utf-8")
      .body("unable to log in with the provided username and password\n".into())
      .map_err(|err| err.into()),

    Err(err) => Response::builder()
      .extension(AppError::from(err))
      .status(axum::http::StatusCode::SERVICE_UNAVAILABLE)
      .header("content-type", "text/plain; charset=utf-8")
      .body("service unavailable\n".into())
      .map_err(|err| err.into()),
  }
}

#[derive(Deserialize)]
struct TreeParams {
  db: Option<CString>,
  ntype: Option<String>,
  noid: Option<String>,
  ntid: Option<String>,
}

async fn api_tree(
  axum::extract::Extension(pgctor): axum::extract::Extension<pg::Connector>,
  axum::extract::Query(TreeParams {
    db,
    ntype,
    noid,
    ntid,
  }): axum::extract::Query<TreeParams>,
) -> Result<Response, AppError> {
  let pgctor = pgctor
    .with(c"statement_timeout", c"10s")
    .with(c"default_transaction_read_only", c"on")
    .with(c"search_path", c"pg_catalog");

  // let pgctor = db.into_iter().fold(pgctor, |pgctor, db| pgctor.with(c"database", db));
  let pgctor = match db {
    Some(db) => pgctor.with(c"database", db),
    None => pgctor,
  };

  // TODO unauthorized
  // TODO service unavailable
  // TODO handle 3D000 invalid_catalog = что отдать? пустой массив
  // так же как и для других типов нод? Или переделать чтобы все ненайденые возвращали 40X?
  // Но как нам понимать из ответа sql что родительсткая нода не найдена?
  let mut pgconn = pgctor.connect().await?;

  const QUERY: pg::NZStr<'_> =
    pg::NZStr::from_bytes(include_bytes!("./api_tree.sql")).unwrap();

  let params = [ntype.as_ref(), noid.as_ref(), ntid.as_ref()]
    .map(|val| val.map(|x| x.as_bytes()));
  pgconn.send_parse(c"", &[pg::TEXT_OID, pg::OID_OID, pg::TEXT_OID], QUERY);
  pgconn.send_bind(c"", &params);
  pgconn.send_execute(); // TODO limit 1
  pgconn.send_sync();

  let mut resp_body = vec![];
  loop {
    let row = match pgconn.recv_message().await? {
      pg::DataRow(row) => row,
      pg::ReadyForQuery { .. } => break,
      _ => continue,
    };
    let scalar = row.into_iter().next().flatten().unwrap_or(b"null");
    resp_body = [scalar, b"\n"].concat();
  }

  let _ = pgconn.close().await;

  Response::builder()
    .header("content-type", "application/json")
    .body(resp_body.into())
    .map_err(|err| err.into())
}

#[derive(Deserialize)]
struct DefnParams {
  db: CString,
  ntype: Option<String>,
  noid: Option<String>,
  ntid: Option<String>,
}

async fn api_defn(
  axum::extract::Extension(pgctor): axum::extract::Extension<pg::Connector>,
  axum::extract::Query(DefnParams {
    db,
    ntype,
    noid,
    ntid,
  }): axum::extract::Query<DefnParams>,
) -> Result<Response, AppError> {
  let pgctor = pgctor
    .with(c"statement_timeout", c"10s")
    .with(c"default_transaction_read_only", c"on")
    .with(c"search_path", c"pg_catalog")
    .with(c"database", db);

  // TODO handle invalid credentials = 401 unauthorized
  // TODO handle 3D000 invalid_catalog = 200 ok , /* no such database */
  let mut pgconn = pgctor.connect().await?;

  const QUERY: pg::NZStr<'_> =
    pg::NZStr::from_bytes(include_bytes!("./api_defn.sql")).unwrap();

  let params = [ntype.as_ref(), noid.as_ref(), ntid.as_ref()]
    .map(|val| val.map(|x| x.as_bytes()));
  pgconn.send_parse(c"", &[pg::TEXT_OID, pg::OID_OID, pg::TEXT_OID], QUERY);
  pgconn.send_bind(c"", &params);
  pgconn.send_execute(); // TODO limit 1
  pgconn.send_sync();

  // TODO handle no DataRow - 200 ok /* no definition found for this tree node */
  let mut resp_body = vec![];
  loop {
    let row = match pgconn.recv_message().await? {
      pg::DataRow(row) => row,
      pg::ReadyForQuery { .. } => break,
      _ => continue,
    };
    let scalar = row.into_iter().next().flatten().unwrap_or(b"null");
    resp_body = scalar.into();
  }

  let _ = pgconn.close().await;

  Response::builder()
    // .header("content-type", "application/sql")
    .header("content-type", "text/plain; charset=utf-8")
    .body(resp_body.into())
    .map_err(|err| err.into())
}

async fn serve_favicon_ico() -> axum::response::Redirect {
  axum::response::Redirect::to("favicon.svg")
}
