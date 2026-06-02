// pub fn write_bad(out: &mut impl TxBuf) {
//   out.put_u8(b'S');
//   out.put_i32(-1);
// }

use std::ffi::{ CStr, FromBytesWithNulError };

// https://www.postgresql.org/docs/18/protocol-message-formats.html#PROTOCOL-MESSAGE-FORMATS-STARTUPMESSAGE
pub fn write_startup(
  out: &mut impl TxBuf,
  options: &[(NZStr<'_>, NZStr<'_>)],
) {
  out.put_frame(|out| {
    out.put_i16(3); // major version
    out.put_i16(0); // minor version
    for &(k, v) in options {
      out.put_str(k);
      out.put_str(v);
    }
    out.put_u8(0);
  });
}

pub fn write_password(out: &mut impl TxBuf, password: NZStr<'_>) {
  out.put_u8(b'p');
  out.put_frame(|out| {
    out.put_str(password);
  });
}

pub fn write_sasl_initial_resp(
  out: &mut impl TxBuf,
  mechanism: NZStr<'_>,
  maybe_data: Option<&[u8]>,
) {
  out.put_u8(b'p');
  out.put_frame(|out| {
    out.put_str(mechanism);
    if let Some(data) = maybe_data {
      out.put_i32(data.len() as _);
      out.put(data);
    } else {
      out.put_i32(-1);
    }
  });
}

pub fn write_sasl_resp(out: &mut impl TxBuf, data: &[u8]) {
  out.put_u8(b'p');
  out.put_frame(|out| {
    out.put(data);
  });
}

// pub fn write_query(out: &mut impl TxBuf, query: NZStr<'_>) {
//   out.put_u8(b'Q');
//   out.put_frame(|out| {
//     out.put_str(query);
//   });
// }

// https://www.postgresql.org/docs/18/protocol-message-formats.html#PROTOCOL-MESSAGE-FORMATS-PARSE
pub fn write_parse(
  out: &mut impl TxBuf,
  stmt_name: NZStr<'_>,
  param_types: &[u32],
  stmt: NZStr<'_>,
) {
  out.put_u8(b'P');
  out.put_frame(|out| {
    out.put_str(stmt_name);
    out.put_str(stmt);
    out.put_i16(param_types.len() as _); // TODO u16?
    // param_types.iter().for_each(|&oid| out.put_u32(oid));
    for &oid in param_types {
      out.put_u32(oid);
    }
  });
}

// https://www.postgresql.org/docs/18/protocol-message-formats.html#PROTOCOL-MESSAGE-FORMATS-DESCRIBE
pub fn write_describe_stmt(out: &mut impl TxBuf, stmt_name: NZStr<'_>) {
  out.put_u8(b'D');
  out.put_frame(|out| {
    out.put_u8(b'S');
    out.put_str(stmt_name);
  });
}

// https://www.postgresql.org/docs/18/protocol-message-formats.html#PROTOCOL-MESSAGE-FORMATS-BIND
pub fn write_bind(
  out: &mut impl TxBuf,
  stmt_name: NZStr<'_>,
  portal_name: NZStr<'_>,
  out_formats: &[i16],
  param_formats: &[i16],
  param_values: &[Option<&[u8]>],
) {
  out.put_u8(b'B');
  out.put_frame(|out| {
    out.put_str(portal_name);
    out.put_str(stmt_name);
    out.put_i16(param_formats.len() as _);
    for &fmt in param_formats {
      out.put_i16(fmt);
    }
    out.put_i16(param_values.len() as _);
    for &maybe_val in param_values {
      if let Some(val) = maybe_val {
        out.put_i32(val.len() as _);
        out.put(val);
      } else {
        out.put_i32(-1);
      }
    }
    out.put_i16(out_formats.len() as _);
    for &fmt in out_formats {
      out.put_i16(fmt);
    }
  });
}

// https://www.postgresql.org/docs/18/protocol-message-formats.html#PROTOCOL-MESSAGE-FORMATS-EXECUTE
pub fn write_execute(
  out: &mut impl TxBuf,
  portal_name: NZStr<'_>,
  max_rows: i32,
) {
  out.put_u8(b'E');
  out.put_frame(|out| {
    out.put_str(portal_name);
    out.put_i32(max_rows); // TODO u32?
  });
}

// https://www.postgresql.org/docs/16/protocol-message-formats.html#PROTOCOL-MESSAGE-FORMATS-CLOSE
pub fn write_close_portal(out: &mut impl TxBuf, portal_name: NZStr<'_>) {
  out.put_u8(b'C');
  out.put_frame(|out| {
    out.put_u8(b'P');
    out.put_str(portal_name);
  });
}

// https://www.postgresql.org/docs/18/protocol-message-formats.html#PROTOCOL-MESSAGE-FORMATS-COPYFAIL
pub fn write_copy_fail(out: &mut impl TxBuf, reason: NZStr<'_>) {
  out.put_u8(b'f');
  out.put_frame(|out| {
    out.put_str(reason);
  });
}

// https://www.postgresql.org/docs/18/protocol-message-formats.html#PROTOCOL-MESSAGE-FORMATS-FLUSH
pub fn write_flush(out: &mut impl TxBuf) {
  out.put(b"H\0\0\0\x04");
}

// https://www.postgresql.org/docs/18/protocol-message-formats.html#PROTOCOL-MESSAGE-FORMATS-SYNC
pub fn write_sync(out: &mut impl TxBuf) {
  out.put(b"S\0\0\0\x04");
}

// https://www.postgresql.org/docs/18/protocol-message-formats.html#PROTOCOL-MESSAGE-FORMATS-TERMINATE
pub fn write_terminate(out: &mut impl TxBuf) {
  out.put(b"X\0\0\0\x04");
}

// https://www.postgresql.org/docs/18/protocol-message-formats.html#PROTOCOL-MESSAGE-FORMATS-CANCELREQUEST
pub fn write_cancel_req(out: &mut impl TxBuf, pid: i32, secret: &[u8]) {
  out.put_frame(|out| {
    out.put_i16(1234);
    out.put_i16(5678);
    out.put_i32(pid);
    out.put(secret);
  });
}

// #[derive(PartialEq, Clone, Copy, Hash, Eq)]
#[derive(Clone, Copy)]
pub struct NZStr<'a>(&'a [u8]);

impl<'a> NZStr<'a> {
  // TODO return Result when const Result.ok() become stable
  pub const fn from_bytes(bytes: &'a [u8]) -> Option<Self> {
    // using memchr, but with no exlicit crate dependency
    match CStr::from_bytes_with_nul(bytes) {
      Err(FromBytesWithNulError::NotNulTerminated) => Some(Self(bytes)),
      _ => None,
    }
  }
}

impl<'a> std::ops::Deref for NZStr<'a> {
  type Target = [u8];
  fn deref(&self) -> &Self::Target { self.0 }
}

impl<'a> TryFrom<&'a [u8]> for NZStr<'a> {
  type Error = ContainsZeroError;
  fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
    Self::from_bytes(value).ok_or(ContainsZeroError)
  }
}

impl<'a> From<&'a std::ffi::CStr> for NZStr<'a> {
  fn from(value: &'a std::ffi::CStr) -> Self {
    NZStr(value.to_bytes())
  }
}

#[derive(Debug)]
pub struct ContainsZeroError;
impl std::error::Error for ContainsZeroError {}
impl std::fmt::Display for ContainsZeroError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "string contains zero byte")
  }
}


pub trait TxBuf {
  fn put(&mut self, val: &[u8]) -> usize;
  fn patch(&mut self, pos: usize, val: &[u8]);

  fn put_i32(&mut self, val: i32) { self.put(&val.to_be_bytes()); }
  // TODO put_oid?
  fn put_u32(&mut self, val: u32) { self.put(&val.to_be_bytes()); }
  fn put_i16(&mut self, val: i16) { self.put(&val.to_be_bytes()); }
  // fn put_u16(&mut self, val: u16) { self.put(&val.to_be_bytes()); }
  fn put_u8(&mut self, val: u8) { self.put(&[val]);}

  fn put_str(&mut self, val: NZStr<'_>) {
    self.put(&val);
    self.put_u8(0);
  }

  fn put_frame(&mut self, put_body: impl FnOnce(&mut Self)) {
    // postgres reads message length as signed int32
    // https://github.com/postgres/postgres/blob/REL_18_2/src/backend/libpq/pqcomm.c#L1206
    // and restricts length value to 4 <= len < MaxAllocSize
    // MaxAllocSize=0x3fffffff
    // https://github.com/postgres/postgres/blob/REL_18_2/src/include/libpq/libpq.h#L31
    let mut len: i32 = 0;
    let base = self.put(&len.to_be_bytes());
    put_body(self);
    len = self.put(b"")
      .checked_sub(base)
      .expect("outgoing postgres message length should not be negative")
      .try_into()
      .expect("outgoing postgres message length should fit into i32");
    self.patch(base, &len.to_be_bytes());
  }
}

impl TxBuf for std::collections::VecDeque<u8> {
  fn put(&mut self, val: &[u8]) -> usize {
    let pos = self.len();
    self.extend(val);
    pos
  }

  fn patch(&mut self, pos: usize, val: &[u8]) {
    self.iter_mut()
      .skip(pos) // TODO ensure O(1)
      .zip(val).for_each(|(d, s)| *d = *s);
  }
}
