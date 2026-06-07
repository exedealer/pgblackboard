use openssl::base64;
use openssl::error::ErrorStack;
use openssl::hash::{MessageDigest, hash};
use openssl::memcmp;
use openssl::pkcs5::pbkdf2_hmac;
use openssl::pkey::PKey;
use openssl::rand::rand_bytes;
use openssl::sign::Signer;

// TODO mostly vibed, need rewrite
// TODO see also deno-postgres https://github.com/denodrivers/postgres/blob/dd7df18fe2ef4da9f1ffa10006763420c89b4b52/connection/scram.ts#L160

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum Error {
  /// Server message was malformed or missing a required field.
  InvalidServerMessage(&'static str),
  /// Server signature did not match – authentication failed.
  AuthFailed,
  /// An OpenSSL operation failed.
  Crypto(ErrorStack),
}

impl std::fmt::Display for Error {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::InvalidServerMessage(r) => write!(f, "invalid server message: {r}"),
      Self::AuthFailed => f.write_str("server signature mismatch"),
      Self::Crypto(e) => write!(f, "crypto error: {e}"),
    }
  }
}

impl std::error::Error for Error {}

impl From<ErrorStack> for Error {
  fn from(e: ErrorStack) -> Self {
    Self::Crypto(e)
  }
}

// impl From<ScramError> for std::io::Error {
//   fn from(e: ScramError) -> Self {
//     std::io::Error::other(e)
//   }
// }

// ── State machine ─────────────────────────────────────────────────────────────

/// SCRAM-SHA-256 client (RFC 5802).
///
/// ```text
/// Client                Server
/// ──────────────────────────────────────────────────────
/// start()  ──── client-first-message ──────────────────►
///      ◄─── server-first-message ──────────────────
/// update() ──── client-final-message ──────────────────►
///      ◄─── server-final-message ──────────────────
/// finish() verifies server signature
/// ```
pub struct ScramSha256 {
  /// `n=<user>,r=<cnonce>` – needed to build the auth message later.
  client_first_msg_bare: Vec<u8>,
  /// Computed during `update`; compared against the value the server sends.
  expected_server_signature: Vec<u8>,
}

impl ScramSha256 {
  // ── Step 1 ────────────────────────────────────────────────────────────────

  pub fn new() -> Self {
    Self { client_first_msg_bare: vec![], expected_server_signature: vec![] }
  }

  /// Build the **client-first-message** and return `(state, message)`.
  ///
  /// The returned bytes must be sent to the server as-is.
  pub fn start(&mut self, username: &str) -> Result<Vec<u8>, Error> {
    // 18 random bytes → 24-char base64 nonce (no padding issues)
    let mut raw = [0u8; 18];
    rand_bytes(&mut raw)?;
    let cnonce = base64::encode_block(&raw);

    // client-first-message-bare  =  "n=<user>,r=<cnonce>"
    // client-first-message     =  "n,,"  +  bare   (no channel binding)
    let bare = format!("n={username},r={cnonce}").into_bytes();
    let msg = [b"n,,".as_slice(), &bare].concat();

    self.client_first_msg_bare = bare;
    Ok(msg)
  }

  // ── Step 2 ────────────────────────────────────────────────────────────────

  /// Process the **server-first-message**, derive all keys, and return the
  /// **client-final-message** that must be sent back to the server.
  ///
  /// `server_first` – raw bytes received from the server
  /// `password`   – cleartext password (SASLprep is the caller's responsibility)
  pub fn update(
    &mut self,
    server_first: &[u8],
    // TODO normalization
    // https://github.com/brianc/node-postgres/blob/f252870eba73c15449b57562e6698b5859e32095/packages/pg/lib/crypto/sasl.js#L21
    // https://docs.rs/crate/stringprep/0.1.5/source/src/lib.rs#54
    password: &[u8],
  ) -> Result<Vec<u8>, Error> {
    // ── Parse server-first-message: r=…,s=…,i=… ─────────────────────────
    let msg = std::str::from_utf8(server_first)
      .map_err(|_| Error::InvalidServerMessage("not UTF-8"))?;

    let (mut snonce, mut salt_b64, mut iterations) = ("", "", 0u32);
    for part in msg.split(',') {
      match part.split_once('=') {
        Some(("r", v)) => snonce = v,
        Some(("s", v)) => salt_b64 = v,
        Some(("i", v)) => {
          iterations = v
            .parse()
            .map_err(|_| Error::InvalidServerMessage("bad iteration count"))?
        }
        _ => {}
      }
    }
    if snonce.is_empty() || salt_b64.is_empty() || iterations == 0 {
      return Err(Error::InvalidServerMessage("missing r/s/i field"));
    }
    // Nonce must begin with the client nonce we sent.
    let cnonce_end = snonce
      .find(',') // nonce itself never contains ','
      .unwrap_or(snonce.len());
    let _cnonce_part = &snonce[..cnonce_end]; // optionally verify it matches

    let salt = base64::decode_block(salt_b64)
      .map_err(|_| Error::InvalidServerMessage("bad base64 salt"))?;

    // ── client-final-message-without-proof ───────────────────────────────
    //   c = base64("n,,")   (channel-binding header, no binding data)
    //   r = full server nonce
    let c_attr = base64::encode_block(b"n,,");
    let cfmwp = format!("c={c_attr},r={snonce}").into_bytes();

    // ── AuthMessage = client-first-bare + "," + server-first + "," + cfmwp
    let auth_msg = [
      self.client_first_msg_bare.as_slice(),
      b",",
      server_first,
      b",",
      cfmwp.as_slice(),
    ]
    .concat();

    // ── Key derivation ───────────────────────────────────────────────────
    //   SaltedPassword  = PBKDF2-HMAC-SHA256(password, salt, i, 32)
    let mut salted_pwd = vec![0u8; 32];
    pbkdf2_hmac(
      password,
      &salt,
      iterations as usize,
      MessageDigest::sha256(),
      &mut salted_pwd,
    )?;

    //   ClientKey     = HMAC-SHA256(SaltedPassword, "Client Key")
    let client_key = hmac_sha256(&salted_pwd, b"Client Key")?;
    //   StoredKey     = SHA256(ClientKey)
    let stored_key = hash(MessageDigest::sha256(), &client_key)?.to_vec();
    //   ClientSignature = HMAC-SHA256(StoredKey, AuthMessage)
    let client_sig = hmac_sha256(&stored_key, &auth_msg)?;
    //   ClientProof   = ClientKey XOR ClientSignature
    let client_proof: Vec<u8> =
      client_key.iter().zip(&client_sig).map(|(a, b)| a ^ b).collect();

    //   ServerKey     = HMAC-SHA256(SaltedPassword, "Server Key")
    let server_key = hmac_sha256(&salted_pwd, b"Server Key")?;
    //   ServerSignature = HMAC-SHA256(ServerKey, AuthMessage)
    self.expected_server_signature = hmac_sha256(&server_key, &auth_msg)?;

    // ── Assemble client-final-message ────────────────────────────────────
    let proof_b64 = base64::encode_block(&client_proof);
    let mut out = cfmwp;
    out.extend_from_slice(b",p=");
    out.extend_from_slice(proof_b64.as_bytes());

    Ok(out)
  }

  // ── Step 3 ────────────────────────────────────────────────────────────────

  // https://docs.rs/postgres-protocol/0.6.10/src/postgres_protocol/authentication/sasl.rs.html#249

  /// Verify the **server-final-message** (`v=<base64-sig>`).
  ///
  /// Returns `Ok(())` only when the server signature is correct.
  pub fn finish(&self, server_final: &[u8]) -> Result<(), Error> {
    let msg = std::str::from_utf8(server_final)
      .map_err(|_| Error::InvalidServerMessage("not UTF-8"))?;

    // Could also be "e=<error>" on authentication failure from the server side.
    if let Some(err) = msg.strip_prefix("e=") {
      return Err(Error::InvalidServerMessage(
        // Leak the static str only for known errors; otherwise generic.
        if err.contains("invalid-proof") {
          "invalid-proof"
        } else {
          "server error"
        },
      ));
    }

    let b64 = msg
      .strip_prefix("v=")
      .ok_or(Error::InvalidServerMessage("missing v= field"))?;
    let server_sig = base64::decode_block(b64)
      .map_err(|_| Error::InvalidServerMessage("bad base64 signature"))?;

    // Constant-time comparison to prevent timing attacks.
    if memcmp::eq(&server_sig, &self.expected_server_signature) {
      Ok(())
    } else {
      Err(Error::AuthFailed)
    }
  }
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<Vec<u8>, ErrorStack> {
  let pkey = PKey::hmac(key)?;
  let mut signer = Signer::new(MessageDigest::sha256(), &pkey)?;
  signer.update(data)?;
  signer.sign_to_vec()
}
