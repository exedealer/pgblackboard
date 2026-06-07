mod decode;
mod encode;
mod md5;
mod scram;

use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use std::collections::VecDeque;
use std::ffi::{CStr, CString};

use decode::parse_message;
use encode::*;

pub use decode::BackendMessage;
pub use decode::BackendMessage::*;
pub use decode::DbError;
pub use decode::Field;
pub use decode::Row;
pub use encode::NZStr;
// pub use encode::ContainsZeroError;

// open enum https://github.com/rust-lang/rfcs/pull/3894
pub const TEXT_OID: u32 = 25;
pub const OID_OID: u32 = 26;
pub const JSON_OID: u32 = 114;
// pub const JSONB_OID: u32 = 3802;

// type BoxError = Box<dyn std::error::Error + Sync + Send + 'static>;

#[derive(Clone, Debug)]
pub struct Connector {
  // addr: SocketAddr,
  host: String,
  port: u16,
  password: Vec<u8>,
  // The only case when we need to access `user` directly is for md5 auth.
  // But `user` is the only option which is required by protocol spec, so keep it static
  // https://www.postgresql.org/docs/16/protocol-message-formats.html#PROTOCOL-MESSAGE-FORMATS-STARTUPMESSAGE
  // user: Vec<u8>,

  // TODO prohibit underscored keys
  options: Vec<(CString, CString)>,
}

impl Connector {
  pub fn new() -> Self {
    Self {
      host: "127.0.0.1".into(),
      port: 5432,
      password: Default::default(),
      options: Default::default(),
    }
  }

  // the only case is timezone in api_run (dyn options)
  // pub fn with_default() {}

  pub fn with(
    mut self,
    opt: impl Into<CString>,
    val: impl Into<CString>,
  ) -> Self {
    // TODO rm existing opt. Case insensitive key comparison? (timezone)
    self.options.push((opt.into(), val.into()));
    self
  }

  pub fn with_addr(mut self, host: impl Into<String>, port: u16) -> Self {
    self.host = host.into();
    self.port = port;
    self
  }

  pub fn with_credentials(
    mut self,
    user: impl Into<CString>,
    password: impl Into<Vec<u8>>,
  ) -> Self {
    self.password = password.into();
    self.with(c"user", user)
  }

  pub async fn connect(&self) -> Result<Connection, ConnectError> {
    Connection::connect(self).await
  }

  fn user(&self) -> Option<&[u8]> {
    self.options.iter().find(|(k, _)| k == c"user").map(|(_, v)| v.as_bytes())
  }
}

pub struct Connection {
  stream: TcpStream, // TODO TLS, Unix
  txbuf: VecDeque<u8>,
  rxbuf: Vec<u8>,
  rxbuf_consumed: usize,
  backend_key_data: Option<(i32, Box<[u8]>)>,
}

impl Connection {
  pub async fn connect(connector: &Connector) -> Result<Self, ConnectError> {
    let host = connector.host.as_str();
    let port = connector.port;
    let password = connector.password.as_slice();
    let user = connector.user().unwrap_or_default();

    let mut conn = Self {
      stream: TcpStream::connect((host, port)).await?,
      txbuf: VecDeque::with_capacity(8 * 1024),
      rxbuf: Vec::with_capacity(8 * 1024),
      rxbuf_consumed: 0,
      backend_key_data: None,
    };

    let options = connector
      .options
      .iter()
      .map(|(k, v)| (k.as_c_str().into(), v.as_c_str().into()))
      .collect::<Vec<_>>() // TODO no alloc
      .into_boxed_slice();

    // TODO TcpStream.shutdown on error?
    // TODO accept iterator
    write_startup(&mut conn.txbuf, &options);
    conn.auth(user, password).await?;
    conn.startup().await.map_err(|err| err.into_authorized())?;
    Ok(conn)
  }

  async fn startup(&mut self) -> Result<(), ConnectError> {
    loop {
      match self.recv_message().await? {
        self::ParameterStatus(..) => {}
        self::BackendKeyData { pid, secret_key } => {
          self.backend_key_data = Some((pid, secret_key.into()));
        }
        // TODO ignore Notification,Notice
        self::ReadyForQuery { .. } => return Ok(()),
        _unexp => return Err("unexpected message received".into()),
      }
    }
  }

  async fn auth(
    &mut self,
    user: &[u8],
    password: &[u8],
  ) -> Result<(), ConnectError> {
    match self.recv_message().await? {
      self::AuthenticationOk => {} // trusted
      self::AuthenticationCleartextPassword => {
        self.auth_pwd(password).await?;
      }
      self::AuthenticationMD5Password { salt } => {
        let md5pwd = md5::md5_password(user, password, &salt);
        self.auth_pwd(&md5pwd).await?;
      }
      self::AuthenticationSASL(_mechs) => {
        // if mechs.contains(&&b"SCRAM-SHA-256"[..]) {
        self.auth_sasl(password).await?;
        // }
      }
      self::AuthenticationKerberosV5
      | self::AuthenticationSCMCredential
      | self::AuthenticationGSS
      | self::AuthenticationSSPI => {
        return Err("unsupported authentication method".into());
      }
      _unexp => return Err("unexpected message received".into()),
    }
    Ok(())
  }

  async fn auth_pwd(&mut self, password: &[u8]) -> Result<(), ConnectError> {
    let pwd_nz = password
      .try_into()
      // TODO proper error normalization
      .map_err(|_| "invalid password")?;
    write_password(&mut self.txbuf, pwd_nz);
    let AuthenticationOk = self.recv_message().await? else {
      return Err("AuthenticationOk expected".into());
    };
    Ok(())
  }

  async fn auth_sasl(&mut self, password: &[u8]) -> Result<(), ConnectError> {
    let mut scram_sha256 = scram::ScramSha256::new();
    let data = scram_sha256.start("")?;
    write_sasl_initial_resp(
      &mut self.txbuf,
      c"SCRAM-SHA-256".into(),
      Some(data.as_slice()),
    );
    use AuthenticationSASLContinue as SASLContinue;
    let SASLContinue(server_first) = self.recv_message().await? else {
      return Err("AuthenticationSASLContinue expected".into());
    };
    let data = scram_sha256.update(server_first, password)?;
    write_sasl_resp(&mut self.txbuf, data.as_ref());

    use AuthenticationSASLFinal as SASLFinal;
    let SASLFinal(server_final) = self.recv_message().await? else {
      return Err("AuthenticationSASLFinal expected".into());
    };
    scram_sha256.finish(server_final)?;

    let AuthenticationOk = self.recv_message().await? else {
      return Err("AuthenticationOk expected".into());
    };
    Ok(())
  }

  // TODO stmt zero-copy
  // Bytes.chain + write_buf (utilizes vectored io)
  pub fn send_parse(
    &mut self,
    stmt_name: &CStr,
    param_types: &[u32],
    stmt: NZStr<'_>,
  ) {
    write_parse(&mut self.txbuf, stmt_name.into(), param_types, stmt);
  }

  pub fn send_describe_stmt(&mut self, stmt_name: &CStr) {
    write_describe_stmt(&mut self.txbuf, stmt_name.into());
  }

  pub fn send_bind(&mut self, stmt_name: &CStr, params: &[Option<&[u8]>]) {
    let portal_name = c"".into();
    let out_formats = &[];
    let param_formats = &[];
    write_bind(
      &mut self.txbuf,
      stmt_name.into(),
      portal_name,
      out_formats,
      param_formats,
      params,
    );
  }

  // TODO dry
  pub fn send_bind_bin(&mut self, stmt_name: &CStr, params: &[Option<&[u8]>]) {
    let portal_name = c"".into();
    let out_formats = &[1];
    let param_formats = &[1];
    write_bind(
      &mut self.txbuf,
      stmt_name.into(),
      portal_name,
      out_formats,
      param_formats,
      params,
    );
  }

  pub fn send_execute(&mut self) {
    let portal_name = c"".into();
    let max_rows = 0; // no limit
    write_execute(&mut self.txbuf, portal_name, max_rows);
  }

  pub fn send_close_portal(&mut self) {
    let portal_name = c"".into();
    write_close_portal(&mut self.txbuf, portal_name);
  }

  pub fn send_copy_fail(&mut self) {
    write_copy_fail(&mut self.txbuf, c"COPY FROM STDIN not supported".into());
  }

  pub fn send_sync(&mut self) {
    write_sync(&mut self.txbuf);
  }

  pub fn send_flush(&mut self) {
    write_flush(&mut self.txbuf);
  }

  // pub fn send_query(&mut self, sql: NZStr<'_>) {
  //   write_query(&mut self.txbuf, sql);
  // }

  async fn do_io(&mut self) -> io::Result<()> {
    log::debug!(target: "pg", "io, txbuf {} bytes", self.txbuf.len());
    self.rxbuf.drain(..self.rxbuf_consumed);
    self.rxbuf_consumed = 0;

    // TODO use incomplete message length to reallocate enough memory;

    let (mut rx, mut tx) = self.stream.split();
    let io_n_bytes = tokio::select! {
      res = rx.read_buf(&mut self.rxbuf) => res,
      res = tx.write_buf(&mut self.txbuf),
        if !self.txbuf.is_empty() => res,
    }?;
    if io_n_bytes == 0 {
      return Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "postgres closed connection unexpectedly",
      ));
    }
    // self.inner.flush().await?; tcp flush is noop
    Ok(())
  }

  pub fn is_drained(&self) -> bool {
    let buf = &self.rxbuf[self.rxbuf_consumed..];
    // TODO no decoding
    parse_message(buf).transpose().is_none()
  }

  // TODO err.into_authorized? we should have separate recv_message for pub and internal use anyway
  // because we should not expose auth/startup messages.
  // Seems that we also should not expose ErrorResponse option
  pub async fn recv_message(&mut self) -> io::Result<BackendMessage<'_>> {
    while self.is_drained() {
      self.do_io().await?;
    }

    // нам здесь нужен непрерывный буфер, поэтому vecdeque не пойдет
    let buf = &self.rxbuf[self.rxbuf_consumed..];
    let (nparsed, msg) = parse_message(buf)
      .map_err(|err| io::Error::other(err))?
      .expect("rxbuf should be not drained"); // TODO no unwrap
    // TODO abort if ParameterStatus client_encoding != UTF8
    self.rxbuf_consumed += nparsed;

    log::debug!(target: "pg", "-> {:?}", msg);
    match msg {
      // TODO returning Err conventionally means that
      // next_message is no more callable(?)
      // But next_message can be called if error is not FATAL.
      // In pgbb there is no case when we need to call next_message
      // after error so returning Err for ErrorResponse is more handy for eager return,
      // but this is definitely a layer abstraction leak.
      // ErrorResponse(dberr) => Err(dberr.into_owned().into()),
      ErrorResponse(dberr) => Err(io::Error::other(dberr.into_owned())),
      msg => Ok(msg),
    }
  }

  pub async fn close(mut self) -> io::Result<()> {
    // TODO what if we send Terminate before ReadyForQuery
    // when read buffer is empty but postgres is executing query?
    // SELECT pg_sleep(10);

    // TODO should handle not drained rxbuf?
    if self.txbuf.is_empty() {
      log::debug!(target: "pg", "terminating gracefully");
      write_terminate(&mut self.txbuf);
      self.stream.write_all_buf(&mut self.txbuf).await?;
    }
    self.stream.shutdown().await?;
    log::debug!(target: "pg", "terminated");
    Ok(())
  }

  pub async fn cancel(&self) -> io::Result<()> {
    let Some((pid, seckey)) = self.backend_key_data.as_ref() else {
      return Err(io::Error::other("no backend key"));
    };
    let mut buf = VecDeque::with_capacity(16);
    write_cancel_req(&mut buf, *pid, seckey);

    let addr = self.stream.peer_addr()?;
    let mut conn = TcpStream::connect(addr).await?;
    conn.write_all_buf(&mut buf).await?;
    conn.shutdown().await?;
    log::debug!(target: "pg", "cancel request sent to pid {pid}");
    Ok(())
  }
}

type BoxError = Box<dyn std::error::Error + Sync + Send + 'static>;

#[derive(Debug)]
pub struct ConnectError {
  pub inner: BoxError,
  is_authorized_: bool,
}

impl ConnectError {
  fn new(err: impl Into<BoxError>) -> Self {
    Self { inner: err.into(), is_authorized_: false }
  }

  fn into_authorized(mut self) -> Self {
    self.is_authorized_ = true;
    self
  }

  // нам интересны только ошибки аутентификации вызваные пользователем.
  // Так же могут быть ошибки аутентификации вызваные подменой сервера
  // при взаимной scram-sha-256 аутентификации. Второй тип ошибок надо
  // интерпретировать как internal server error
  /// 28000 role \"xxxx\" does not exist;
  /// 28P01 password authentication failed for user \"xxx\"
  pub fn is_bad_credentials(&self) -> bool {
    // TODO check nul byte rejection errors
    self
      .as_dberror()
      .map(|err| err.code())
      .unwrap_or_default()
      .starts_with(b"28")
  }

  /// Error occured after AuthenticationOk message received
  pub fn is_authorized(&self) -> bool {
    self.is_authorized_
  }

  pub fn as_dberror(&self) -> Option<&DbError<'static>> {
    self.inner.downcast_ref()
  }
}

impl std::error::Error for ConnectError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    Some(self.inner.as_ref())
  }
}

impl std::fmt::Display for ConnectError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", self.inner)
  }
}

impl From<DbError<'static>> for ConnectError {
  fn from(err: DbError<'static>) -> Self {
    Self::new(err)
  }
}

impl From<std::io::Error> for ConnectError {
  fn from(err: std::io::Error) -> Self {
    // TODO no downcast
    match err.downcast::<DbError>() {
      Ok(dberr) => Self::new(dberr),
      Err(other) => Self::new(other),
    }
  }
}

impl From<decode::Error> for ConnectError {
  fn from(err: decode::Error) -> Self {
    Self::new(err)
  }
}

impl From<scram::Error> for ConnectError {
  fn from(err: scram::Error) -> Self {
    Self::new(err)
  }
}

impl From<&str> for ConnectError {
  fn from(msg: &str) -> Self {
    Self::new(msg)
    // TODO origin: "pgbb backend"
  }
}
