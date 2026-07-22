//! Lowercase hex encoding for digest bytes.
//!
//! `sha2` 0.11 returns `hybrid_array::Array` from `finalize()`/`digest()`, which
//! does not implement `LowerHex` — the `format!("{:x}", …)` this crate used
//! against 0.10's `GenericArray` no longer compiles. Output must stay
//! byte-for-byte identical to that formatting: these digests are persisted in
//! the index (`files.content_sha256`), in snapshot tokens, and in audit-bundle
//! attestations, so a changed encoding would invalidate stored state.

pub fn encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::encode;
    use sha2::{Digest, Sha256};

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
