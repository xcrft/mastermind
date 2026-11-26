//! Lowercase hex encoding for digest bytes.
//!
//! `sha2` 0.11 returns `hybrid_array::Array` from `finalize()`/`digest()`, which
//! does not implement `LowerHex` — the `format!("{:x}", …)` this crate used
//! against 0.10's `GenericArray` no longer compiles. Output must stay
//! byte-for-byte identical to that formatting: these digests are persisted in
//! the index (`files.content_sha256`), in snapshot tokens, and in audit-bundle
//! attestations, so a changed encoding would invalidate stored state.

use sha2::{Digest, Sha256};
use std::io::Write;

pub fn encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

/// The lowercase hex SHA-256 of `bytes`, the digest every stored binding uses.
pub fn sha256(bytes: impl AsRef<[u8]>) -> String {
    encode(&Sha256::digest(bytes))
}

/// Whether `value` is exactly `length` lowercase hex digits, the only form a
/// stored digest, receipt or invocation identifier may take.
pub fn is_lower(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Feeds every byte `inner` accepts to `hasher`. `sha2` 0.11 hashers no longer
/// implement `io::Write`, and the bounded copy helpers write into one.
pub struct Sha256Writer<'a, W> {
    pub hasher: &'a mut Sha256,
    pub inner: W,
}

impl<W: Write> Write for Sha256Writer<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(bytes)?;
        self.hasher.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercase_check_rejects_uppercase_other_lengths_and_non_hex() {
        assert!(is_lower("0123456789abcdef", 16));
        assert!(!is_lower("0123456789ABCDEF", 16));
        assert!(!is_lower("0123456789abcdef", 15));
        assert!(!is_lower("0123456789abcdeg", 16));
    }

    #[test]
    fn encodes_known_sha256_vector() {
        // NIST vector for "abc" — pins lowercase, zero-padded, most-significant
        // nibble first, matching what `{:x}` produced under sha2 0.10.
        assert_eq!(
            encode(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn zero_pads_every_byte_and_handles_empty() {
        assert_eq!(encode(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
        assert_eq!(encode(&[]), "");
    }
}
