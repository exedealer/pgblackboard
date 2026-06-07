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
  pub fn issue(
    &self,
    user: &[u8],
    password: &[u8],
  ) -> Result<String, ErrorStack> {
    let mut iv = [0u8; 12];
    rand_bytes(&mut iv)?;

    let mut tag = [0u8; 16];
    let ciphertext = encrypt_aead(
      Cipher::aes_256_gcm(),
      &self.secret,
      Some(&iv),
      user, // AAD
      password,
      &mut tag,
    )?;

    let token = [iv.as_slice(), tag.as_slice(), &ciphertext].concat();
    Ok(base64::encode_block(&token))
  }

  pub fn verify(&self, user: &[u8], token: &[u8]) -> Option<Vec<u8>> {
    let token_str = std::str::from_utf8(token).ok()?;
    let bytes = base64::decode_block(token_str).ok()?;

    let mut ciphertext = bytes.as_slice();
    let iv = ciphertext.split_off(..12)?;
    let tag = ciphertext.split_off(..16)?;

    let password = decrypt_aead(
      Cipher::aes_256_gcm(),
      &self.secret,
      Some(iv),
      user, // AAD
      ciphertext,
      tag,
    )
    .ok()?;

    Some(password)
  }
}
