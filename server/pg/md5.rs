use openssl::hash::{Hasher, MessageDigest};
use std::io::Write;

pub fn md5_password(user: &[u8], password: &[u8], salt: &[u8]) -> [u8; 3 + 32] {
  let step1 = concat_md5(password, user);
  let mut step1_hex = [0u8; 32];
  write!(step1_hex.as_mut_slice(), "{step1:032x}").unwrap();
  let step2 = concat_md5(&step1_hex, &salt);
  let mut res = [0u8; _];
  write!(res.as_mut_slice(), "md5{step2:032x}").unwrap();
  res
}

// TODO no unwrap
fn concat_md5(chunk1: &[u8], chunk2: &[u8]) -> u128 {
  let mut hasher = Hasher::new(MessageDigest::md5()).unwrap();
  hasher.update(chunk1).unwrap();
  hasher.update(chunk2).unwrap();
  let digest = hasher.finish().unwrap();
  digest.as_array().map(|&b| u128::from_be_bytes(b)).unwrap()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_1() {
    let result = md5_password(b"user", b"password", &[0, 0, 0, 0]);
    assert_eq!(result, *b"md526954954055dc4921b63361688476acf");
  }

  #[test]
  fn test_zeros() {
    let result = md5_password(b"user", b"password", &[0, 129, 128, 43]);
    assert_eq!(result, *b"md50003108c669dd0773195cf7c18a00700");
  }
}
