use openssl::base64;
use openssl::error::ErrorStack;
use openssl::rand::rand_bytes;
use openssl::symm::{Cipher, decrypt_aead, encrypt_aead};

pub struct Authenticator {
  secret: [u8; 32],
}

impl Authenticator {
  pub fn new() -> Result<Self, ErrorStack> {
    let mut secret = [0u8; 32];
    // TODO accept secret key from env var
    #[cfg(not(debug_assertions))]
    rand_bytes(&mut secret)?;
    Ok(Self { secret })
  }

  // TODO add expiration?
  pub fn issue(&self, user: &[u8], pwd: &[u8]) -> Result<String, ErrorStack> {
    let mut iv = [0u8; 12];
    rand_bytes(&mut iv)?;

    let mut tag = [0u8; 16];
    let alg = Cipher::aes_256_gcm();
    let enc = encrypt_aead(alg, &self.secret, Some(&iv), user, pwd, &mut tag)?;

    let token = [iv.as_slice(), tag.as_slice(), &enc].concat();
    Ok(base64::encode_block(&token))
  }

  pub fn verify(&self, user: &[u8], token: &[u8]) -> Option<Vec<u8>> {
    let token_str = std::str::from_utf8(token).ok()?;
    let bytes = base64::decode_block(token_str).ok()?;

    let mut enc = bytes.as_slice();
    let iv = enc.split_off(..12)?;
    let tag = enc.split_off(..16)?;
    let alg = Cipher::aes_256_gcm();
    let pwd = decrypt_aead(alg, &self.secret, Some(iv), user, enc, tag).ok()?;
    Some(pwd)
  }
}
