use std::ffi::CStr;
use std::fmt::Debug;

pub type Result<T> = std::result::Result<T, Error>;

use Error::*;

#[derive(Debug)]
pub enum BackendMessage<'a> {

  // TODO do not expose startup messages
  NegotiateProtocolVersion,
  AuthenticationOk,
  AuthenticationKerberosV5,
  AuthenticationCleartextPassword,
  AuthenticationMD5Password { salt: [u8; 4] },
  AuthenticationSCMCredential,
  AuthenticationGSS,
  AuthenticationGSSContinue(&'a [u8]),
  AuthenticationSSPI,
  AuthenticationSASL(Box<[&'a CStr]>),
  AuthenticationSASLContinue(&'a [u8]),
  AuthenticationSASLFinal(&'a [u8]),
  BackendKeyData { pid: i32, secret_key: &'a [u8] },
  // --------

  ParameterStatus(&'a CStr, &'a CStr),
  ReadyForQuery { txn_status: u8 }, // I|T|E

  DataRow(Row<'a>),
  ParseComplete,
  BindComplete,
  CloseComplete,
  ParameterDescription(Box<[Oid]>),
  RowDescription(Box<[Field<'a>]>),
  NoData,
  CommandComplete { tag: &'a CStr },
  EmptyQueryResponse { tag: &'static CStr },
  PortalSuspended { tag: &'static CStr },
  ErrorResponse(DbError<'a>),
  NoticeResponse(DbError<'a>),

  CopyInResponse(CopyFmt),
  CopyOutResponse(CopyFmt),
  CopyBothResponse(CopyFmt),
  CopyData(&'a [u8]),
  CopyDone,

  NotificationResponse {
    pid: i32,
    channel: &'a CStr,
    payload: &'a CStr,
  },
}

#[derive(Debug)]
pub enum Error {
  MessageTooBig,
  InvalidMessageLength,
  InvalidDatumLength,
  UnexpectedEndOfMessage,
  UnexpectedTrailingData,
  UnknownMessage,
}

impl std::fmt::Display for Error {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{:?}", self)
  }
}

impl std::error::Error for Error {}


/// Numeric object identifer.
/// http://www.postgresql.org/docs/9.4/static/datatype-oid.html
pub type Oid = u32;


// #[derive(Debug)]
// #[derive(PartialEq)]
// pub enum TransactionStatus {
//   Idle,
//   InTransaction,
//   InFailedTransaction,
// }
//





// TODO big toasted values stream or truncate
pub fn parse_message(buf: &[u8]) -> Result<Option<(usize, BackendMessage<'_>)>> {
  if buf.len() < 5 { return Ok(None); }
  let msg_len = buf[1..].first_chunk().map(|x| i32::from_be_bytes(*x)).unwrap();
  if msg_len < 4 { return Err(InvalidMessageLength); }
  if msg_len > 0x2000000 { return Err(MessageTooBig); } // TODO limit buffer on caller side
  let msg_len = msg_len as _;
  if buf.len() <= msg_len { return Ok(None); }
  let ident = buf[0];
  let body = &buf[5..msg_len + 1];
  let res = decode_message(ident, body)?;
  // buf.split_off(..msg_len + 1);
  Ok(Some((msg_len + 1, res)))
}

// errors
// unexpected end of message
// unexpected trailing data
// unknown postgres message
pub fn decode_message(ident: u8, ref mut body: &[u8]) -> Result<BackendMessage<'_>> {
  use BackendMessage::*;

  // println!("{}. {:?}", ident.escape_ascii(), body);

  // https://github.com/postgres/postgres/blob/REL_18_STABLE/src/include/libpq/protocol.h#L36
  let ret = match ident {
    b'D' => DataRow(read_row(body)?),
    b'd' => CopyData(read_all(body)),

    b'R' => match read_i32(body)? {
      0 => AuthenticationOk,
      2 => AuthenticationKerberosV5,
      3 => AuthenticationCleartextPassword,
      5 => AuthenticationMD5Password { salt: read_array(body)? },
      6 => AuthenticationSCMCredential,
      7 => AuthenticationGSS,
      8 => AuthenticationGSSContinue(read_all(body)),
      9 => AuthenticationSSPI,
      10 => AuthenticationSASL(read_sasl_mechanisms(body)?),
      11 => AuthenticationSASLContinue(read_all(body)),
      12 => AuthenticationSASLFinal(read_all(body)),
      _tag => return Err(UnknownMessage),
    }

    b'E' => ErrorResponse(read_error(body)?),
    b'N' => NoticeResponse(read_error(body)?),
    b'S' => ParameterStatus(read_str(body)?, read_str(body)?),

    b'1' => ParseComplete,
    b'2' => BindComplete,
    b't' => ParameterDescription(read_many(body, read_u32)?),
    b'T' => RowDescription(read_many(body, read_field)?),
    b'n' => NoData,
    b'C' => CommandComplete { tag: read_str(body)? },
    b'I' => EmptyQueryResponse { tag: c"EMPTY QUERY" },
    b's' => PortalSuspended { tag: c"PORTAL SUSPENDED" },
    b'3' => CloseComplete,

    b'K' => BackendKeyData {
      pid: read_i32(body)?,
      secret_key: read_all(body),
    },
    b'Z' => ReadyForQuery {
      txn_status: read_u8(body)?,
    },
    b'A' => NotificationResponse {
      pid: read_i32(body)?,
      channel: read_str(body)?,
      payload: read_str(body)?,
    },

    b'G' => CopyInResponse(read_copy_fmt(body)?),
    b'H' => CopyOutResponse(read_copy_fmt(body)?),
    b'W' => CopyBothResponse(read_copy_fmt(body)?),
    b'c' => CopyDone,

    _ => return Err(UnknownMessage),
  };

  if !body.is_empty() {
    // println!("{}. {:?}", ident.escape_ascii(), body);
    return Err(UnexpectedTrailingData)
  }

  Ok(ret)
}

fn read_sasl_mechanisms<'a>(buf: &mut &'a [u8]) -> Result<Box<[&'a CStr]>> {
  let mut ret = Vec::with_capacity(2);
  while let s = read_str(buf)? && !s.is_empty() {
    ret.push(s);
  }
  Ok(ret.into())
}



#[derive(Debug)]
pub struct CopyFmt {
  /// 0 indicates the overall COPY format is textual (rows separated
  /// by newlines, columns separated by separator characters, etc).
  /// 1 indicates the overall copy format is binary (similar to DataRow
  /// format). See COPY for more information.
  pub format: u8,

  /// The format codes to be used for each column.
  /// Each must presently be zero (text) or one (binary).
  /// All must be zero if the overall copy format is textual.
  pub column_formats: Box<[i16]>,
}

fn read_copy_fmt(buf: &mut &[u8]) -> Result<CopyFmt> {
  let format = read_u8(buf)?;
  let column_formats = read_many(buf, read_i16)?;
  Ok(CopyFmt { format, column_formats })
}


#[derive(Clone)]
pub struct Row<'a> {
  ncols: usize,
  buf: &'a [u8],
}

impl<'a> Iterator for Row<'a> {
  type Item = Option<&'a [u8]>;

  fn next(&mut self) -> Option<Self::Item> {
    if self.ncols == 0 { return None; }
    self.ncols -= 1;
    read_datum(&mut self.buf)
      .expect("row values should be prevalidated")
      .into()
  }
}

impl<'a> std::fmt::Debug for Row<'a> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    fn escape_sql(val: Option<&[u8]>) -> impl Debug {
      std::fmt::from_fn(move |f| {
        let Some(val) = val else {
          return write!(f, "NULL");
        };
        write!(f, "'")?;
        for b in val.iter().take(100) {
          match b {
            b'"' => write!(f, "\""),
            _ => write!(f, "{}", b.escape_ascii()),
          }?;
        }
        // TODO strip middle
        if val.len() > 100 {
          write!(f, "\u{2026}")?; // triple dot
        }
        write!(f, "'")
      })
    }

    f.debug_list()
      .entries(self.clone().map(|val| escape_sql(val)))
      .finish()
  }
}

fn read_row<'a>(buf: &mut &'a [u8]) -> Result<Row<'a>> {
  let ncols = read_u16(buf)?.into(); // TODO i16? error if negative
  let res = Row { ncols, buf: *buf };
  std::iter::repeat_with(|| read_datum(buf).map(drop))
    .take(ncols).try_for_each(|res| res)?;
  Ok(res)
}

fn read_datum<'a>(buf: &mut &'a [u8]) -> Result<Option<&'a [u8]>> {
  match read_i32(buf)? {
    -1 => Ok(None),
    ilen if let Ok(len) = ilen.try_into() => Ok(Some(
      buf.split_off(..len).ok_or(UnexpectedEndOfMessage)?
    )),
    _ => Err(InvalidDatumLength),
  }

  // match read_i32(buf)? {
  //   len @ 0.. => Ok(Some(read_exact(buf, len as _)?)),
  //   -1 => Ok(None),
  //   _ => Err(InvalidDatumLength),
  // }
}


#[derive(Debug)]
pub struct Field<'a> {
  pub name: &'a CStr,

  /// If the field can be identified as a column of a specific table,
  /// the object ID of the table; otherwise zero.
  pub table_oid: Oid, // Option<NonZeroU32>

  /// If the field can be identified as a column of a specific table,
  /// the attribute number of the column; otherwise zero.
  pub table_col: i16, // Option<NonZeroI16>

  /// The object ID of the field's data type.
  pub type_oid: Oid,

  /// The type modifier (see pg_attribute.atttypmod).
  /// The meaning of the modifier is type-specific.
  pub type_mod: i32,

  /// The data type size (see pg_type.typlen).
  /// Note that negative values denote variable-width types.
  pub type_size: i16,

  /// The format code being used for the field.
  /// Currently will be zero (text) or one (binary).
  /// In a RowDescription returned from the statement variant of Describe,
  /// the format code is not yet known and will always be zero.
  pub format: i16,
}

fn read_field<'a>(buf: &mut &'a [u8]) -> Result<Field<'a>> {
  Ok(Field {
    name: read_str(buf)?,
    table_oid: read_u32(buf)?,
    table_col: read_i16(buf)?,
    type_oid: read_u32(buf)?,
    type_size: read_i16(buf)?,
    type_mod: read_i32(buf)?,
    format: read_i16(buf)?,
  })
}

// pub type ErrorData<'a> = Vec<(u8, &'a CStr)>;

use std::borrow::Cow;

#[derive(Debug)]
pub struct DbError<'a> {
  pub fields: Vec<(DbErrorField, Cow<'a, CStr>)>,
}

impl DbError<'_> {
  pub fn into_owned(self) -> DbError<'static> {
    let fields = self.fields.into_iter()
      .map(|(k, v)| (k, Cow::Owned(v.into_owned())))
      .collect();
    DbError { fields }
  }

  pub fn code(&self) -> &[u8] {
    self.get(b'C').map(|v| v.to_bytes()).unwrap_or(b"")
  }

  /// 1-based char offset
  // pub fn position(&self) -> Option<usize> { // Option<NonZeroUsize> ?
  //   let val = self.get(b'P')?;
  //   let val = val.to_str().ok()?;
  //   let val = val.parse().ok()?;
  //   (val > 0).then_some(val)
  // }

  fn get(&self, key: u8) -> Option<&CStr> {
    self.fields.iter().find(|(k, _)| k.0 == key).map(|(_, v)| v.as_ref())
  }
}

impl std::error::Error for DbError<'_> {}

impl std::fmt::Display for DbError<'_> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let mut sev = c"";
    let mut code = c"";
    let mut msg = c"";
    for (key, val) in &self.fields {
      let slot = match key.0 {
        b'C' => &mut code,
        b'V' => &mut sev,
        b'M' => &mut msg,
        _ => continue,
      };
      *slot = &val;
    }
    let sev = sev.to_bytes().escape_ascii();
    let code = code.to_bytes().escape_ascii();
    write!(f, "{sev}#{code} {msg:?}")
  }
}

/// https://www.postgresql.org/docs/18/protocol-error-fields.html
#[derive(Clone, Copy)]
pub struct DbErrorField(u8);

impl DbErrorField {
  // pub const SEVERITY: Self = b'S';
//   pub const SEVERITY_EN: Self = Self(b'V');
  // const CODE: Self = Self(b'C');
//   pub const MESSAGE: Self = Self(b'M');
//   pub const DETAIL: Self = Self(b'D');
//   pub const HINT: Self = Self(b'H');
//   pub const POSITION: Self = Self(b'P');
//   pub const INTERNAL_POSITION: Self = Self(b'p');
//   pub const INTERNAL_QUERY: Self = Self(b'q');
//   pub const CONTEXT: Self = Self(b'W');
//   pub const FILE: Self = Self(b'F');
//   pub const LINE: Self = Self(b'L');
//   pub const ROUTINE: Self = Self(b'R');
//   pub const SCHEMA_NAME: Self = Self(b's');
//   pub const TABLE_NAME: Self = Self(b't');
//   pub const COLUMN_NAME: Self = Self(b'c');
//   pub const DATATYPE_NAME: Self = Self(b'd');
//   pub const CONSTRAINT_NAME: Self = Self(b'n');
}

impl std::fmt::Debug for DbErrorField {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", self.0.escape_ascii())
  }
}

impl std::fmt::Display for DbErrorField {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self.0 {
      b'S' => write!(f, "severity"),
      b'V' => write!(f, "severity_en"),
      b'C' => write!(f, "code"),
      b'M' => write!(f, "message"),
      b'D' => write!(f, "detail"),
      b'H' => write!(f, "hint"),
      b'P' => write!(f, "position"),
      b'p' => write!(f, "internal_position"),
      b'q' => write!(f, "internal_query"),
      b'W' => write!(f, "context"), // "where"?
      b'F' => write!(f, "file"),
      b'L' => write!(f, "line"),
      b'R' => write!(f, "routine"),
      b's' => write!(f, "schema_name"),
      b't' => write!(f, "table_name"),
      b'c' => write!(f, "column_name"),
      b'd' => write!(f, "datatype_name"),
      b'n' => write!(f, "constraint_name"),
      unknown => write!(f, "field_{unknown:02x}"),
    }
  }
}

fn read_error<'a>(buf: &mut &'a [u8]) -> Result<DbError<'a>> {
  let mut fields = Vec::with_capacity(18);
  // TODO iter::from_fn().try_collect
  while let key @ 1.. = read_u8(buf)? {
    let val = read_str(buf)?;
    fields.push((DbErrorField(key), val.into()));
  }
  Ok(DbError { fields })
}

// TODO all this lists are related to columns list
// RowDescription
// CopyInResponse column_formats
// DataRow can be read with read_many but avoids allocation
// ParameterDescription ... not related to columns, but stil about tuple
fn read_many<'a, T>(
  buf: &mut &'a [u8],
  mut read_item: impl FnMut(&mut &'a [u8]) -> Result<T>,
) -> Result<Box<[T]>> {
  let len = read_u16(buf)?; // TODO i16, check negative?
  let len = len.into();
  // TODO try_collect https://doc.rust-lang.org/nightly/std/iter/trait.Iterator.html#method.try_collect
  // std::iter::from_fn(|| read_item(buf)).take(len).try_collect()
  let mut items = Vec::with_capacity(len);
  for _ in 0..len {
    let el = read_item(buf)?;
    items.push(el);
  }
  Ok(items.into())
}

fn read_str<'a>(buf: &mut &'a [u8]) -> Result<&'a CStr> {
  let val = std::ffi::CStr::from_bytes_until_nul(*buf)
    .map_err(|_| UnexpectedEndOfMessage)?;
  // TODO count_bytes will be O(n)
  let _ = buf.split_off(..val.count_bytes() + 1);
  Ok(val)
}

fn read_u32(buf: &mut &[u8]) -> Result<u32> {
  read_array(buf).map(u32::from_be_bytes)
}

fn read_i32(buf: &mut &[u8]) -> Result<i32> {
  read_array(buf).map(i32::from_be_bytes)
}

fn read_u16(buf: &mut &[u8]) -> Result<u16> {
  read_array(buf).map(u16::from_be_bytes)
}

fn read_i16(buf: &mut &[u8]) -> Result<i16> {
  read_array(buf).map(i16::from_be_bytes)
}

fn read_u8(buf: &mut &[u8]) -> Result<u8> {
  read_array(buf).map(|[x]| x)
}

fn read_array<const N: usize>(buf: &mut &[u8]) -> Result<[u8; N]> {
  let (&chunk, tail) = buf.split_first_chunk().ok_or(UnexpectedEndOfMessage)?;
  *buf = tail;
  Ok(chunk)
}

// fn read_exact<'a>(buf: &mut &'a [u8], len: usize) -> Result<&'a [u8]> {
//   buf.split_off(..len).ok_or(UnexpectedEndOfMessage)
// }

fn read_all<'a>(buf: &mut &'a [u8]) -> &'a [u8] {
  buf.split_off(..buf.len()).unwrap()
}
