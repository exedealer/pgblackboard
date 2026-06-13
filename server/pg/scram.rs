use openssl::base64;
use openssl::hash::MessageDigest;
use openssl::memcmp;
use openssl::pkcs5::pbkdf2_hmac;
use openssl::rand::rand_bytes;
use openssl::sha::sha256;
use std::fmt::Display;

use std::io::{self, Write as _};

pub struct ScramSha256 {
  c_nonce: [u8; 24],
  server_sig: [u8; 32],
}

impl ScramSha256 {
  pub fn new(_user: &[u8]) -> io::Result<Self> {
    let mut c_nonce = [0u8; _];
    rand_bytes(&mut c_nonce).map_err(io::Error::other)?;
    // c-nonce should be printable %x21-2B / %x2D-7E
    // so we can drop 2 bits and move byte to allowed 2D-7E range
    let c_nonce = c_nonce.map(|b| (b & 0b111111) + 0x2D);
    Ok(Self { c_nonce, server_sig: [0; _] })
  }

  fn client_first_message_bare(&self) -> impl Display + 'static {
    let c_nonce = self.c_nonce;
    std::fmt::from_fn(move |f| {
      let c_nonce = str::from_utf8(&c_nonce).expect("ascii only");
      write!(f, "n=,r={c_nonce}")
    })
  }

  pub fn client_first_message(&self) -> Vec<u8> {
    format!("n,,{}", self.client_first_message_bare()).into()
  }

  pub fn update(
    &mut self,
    server_first_msg: &[u8],
    password: &[u8],
  ) -> io::Result<Vec<u8>> {
    let server_first_msg = str::from_utf8(server_first_msg)
      .map_err(|_| invalid_inp("non-utf8 server-first-message"))?;

    let mut attrs = server_first_msg.split(',');
    // https://datatracker.ietf.org/doc/html/rfc5802#section-7
    let is_printable = |ch| matches!(ch, '\x21'..='\x2B' | '\x2D'..='\x7E');

    let nonce = attrs
      .next()
      .and_then(|a| a.strip_prefix("r="))
      // The client MUST verify that the initial part of the nonce used
      // in subsequent messages is the same as the nonce it initially specified.
      .filter(|val| val.as_bytes().starts_with(&self.c_nonce))
      .filter(|val| val.chars().all(is_printable))
      .ok_or_else(|| invalid_inp("bad nonce"))?;

    let salt = attrs
      .next()
      .and_then(|a| a.strip_prefix("s="))
      .and_then(|val| base64::decode_block(val).ok())
      .ok_or_else(|| invalid_inp("bad salt"))?;

    let iters = attrs
      .next()
      .and_then(|a| a.strip_prefix("i="))
      .and_then(|val| val.parse().ok())
      // spec requires at least 4096 iterations
      // but postgres allows 1 .. 0x7fffffff for `scram_iterations`.
      // https://www.postgresql.org/docs/18/runtime-config-connection.html#GUC-SCRAM-ITERATIONS
      // 100k for upper bound is taken from node-postgres
      // and this page https://github.com/advisories/GHSA-98QH-XJC8-98PQ
      // Seems that psql does not limit iterations but allows to interrupt
      // https://github.com/postgres/postgres/blob/27cf3b5aff4fdb53d00f44760ecdce06ddb02925/src/common/scram-common.c#L84
      .filter(|&val| matches!(val, 1..100_000))
      .ok_or_else(|| invalid_inp("bad iteration count"))?;

    if let Some(_) = attrs.next() {
      // TODO what are extensions? ignore extensions?
      return Err(invalid_inp("unexpected attribute in server-first-message"));
    };

    let c_attr = "biws"; // base64(b"n,,"); (channel-binding header, no binding data)
    let client_final_msg_no_proof = format_args!("c={c_attr},r={nonce}");

    // https://datatracker.ietf.org/doc/html/rfc5802#section-3

    let client_first_message_bare = self.client_first_message_bare();
    let auth_msg = format_args!(
      "{client_first_message_bare},\
      {server_first_msg},\
      {client_final_msg_no_proof}"
    );
    let password = saslprep_lax(password);
    let salted_password = pbkdf2_hmac_sha256(&salt, iters, &password);
    let client_key = hmac_sha256(&salted_password, "Client Key");
    let stored_key = sha256(&client_key);
    let client_sig = hmac_sha256(&stored_key, auth_msg);
    let client_proof = xor_bytes(client_key, client_sig);
    let server_key = hmac_sha256(&salted_password, "Server Key");
    self.server_sig = hmac_sha256(&server_key, auth_msg);

    let client_proof = base64::encode_block(&client_proof); // TODO no alloc
    Ok(format!("{client_final_msg_no_proof},p={client_proof}").into())
  }

  // TODO take ownership? they care about state checking for some reason
  pub fn finish(&self, server_final_msg: &[u8]) -> io::Result<()> {
    // https://docs.rs/postgres-protocol/0.6.10/src/postgres_protocol/authentication/sasl.rs.html#249
    let first_attr = str::from_utf8(server_final_msg)
      .map_err(|_| invalid_inp("non-utf8 server-final-message"))?
      .split(',')
      .next()
      .expect("str::split should produce non-empty iterator");

    if let Some(e) = first_attr.strip_prefix("e=") {
      return Err(invalid_inp(format!("server responded with error: {e}")));
    }

    let server_sig = first_attr
      .strip_prefix("v=")
      .and_then(|val| base64::decode_block(val).ok())
      .ok_or_else(|| invalid_inp("bad server signature"))?;

    if !(server_sig.len() == self.server_sig.len() // because memcmp::eq panics
      && memcmp::eq(&server_sig, &self.server_sig))
    // constant time check
    {
      return Err(invalid_inp("server signature does not match"));
    }
    Ok(())
  }
}

fn pbkdf2_hmac_sha256(salt: &[u8], iters: usize, password: &[u8]) -> [u8; 32] {
  let mut out = [0u8; _];
  let alg = MessageDigest::sha256();
  // TODO is safe to unwrap?
  pbkdf2_hmac(password, salt, iters, alg, &mut out).unwrap();
  out
}

fn hmac_sha256(key: &[u8], data: impl Display) -> [u8; 32] {
  // TODO is it safe to unwrap?
  let alg = MessageDigest::sha256();
  let pkey = openssl::pkey::PKey::hmac(key).unwrap();
  let mut signer = openssl::sign::Signer::new(alg, &pkey).unwrap();
  write!(signer, "{data}").unwrap();
  let mut out = [0; _];
  signer.sign(&mut out).unwrap();
  out
}

fn xor_bytes<const N: usize>(mut a: [u8; N], b: [u8; N]) -> [u8; N] {
  a.iter_mut().zip(b).for_each(|(a, b)| *a ^= b);
  a
}

// TODO invent more accurate error reporting
fn invalid_inp(
  err: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidInput, err)
}

fn saslprep_lax(password: &[u8]) -> std::borrow::Cow<'_, [u8]> {
  // https://github.com/brianc/node-postgres/blob/f252870eba73c15449b57562e6698b5859e32095/packages/pg/lib/crypto/sasl.js#L21

  // postgres-protocol crate uses strict saslprep but allows non-utf8
  // https://docs.rs/postgres-protocol/latest/src/postgres_protocol/authentication/sasl.rs.html#26
  // https://docs.rs/crate/stringprep/0.1.5/source/src/lib.rs#54

  // fast path for ascii text, ascii controls allowed
  if password.is_ascii() {
    return password.into();
  }

  let Ok(password) = str::from_utf8(password) else {
    return password.into(); // allow non-utf8
  };

  let map_char = |ch| match ch as u32 {
    // https://www.rfc-editor.org/info/rfc3454/#appendix-C.1.2
    0x00A0 | 0x1680 | 0x2000..=0x200B | 0x202F | 0x205F | 0x3000 => Some(' '),

    // https://www.rfc-editor.org/info/rfc3454/#appendix-B.1
    0x00AD | 0x034F | 0x1806 | 0x180B..=0x180D | 0x200C | 0x200D => None,
    0x2060 | 0xFE00..=0xFE0F | 0xFEFF => None,

    _ => Some(ch),
  };

  use unicode_normalization::UnicodeNormalization;
  let s: String = password.chars().filter_map(map_char).nfkc().collect();
  s.into_bytes().into()
}

// https://github.com/denodrivers/postgres/blob/dd7df18fe2ef4da9f1ffa10006763420c89b4b52/connection/scram.ts#L160
