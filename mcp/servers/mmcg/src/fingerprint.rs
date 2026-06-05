//! Structural fingerprinting for files in the codegraph.
//!
//! The fingerprint hashes ONLY a file's structural extract — language tag,
//! sorted `(kind, name, signature)` tuples for symbols, sorted
//! `(kind, from_symbol_name, to_name, to_path)` tuples for edges. Line numbers,
//! comments, whitespace, and raw bytes are excluded by design: that's the whole
//! point of cosmetic-vs-structural classification.
//!
//! FNV-1a 64-bit is used (not `std::hash::DefaultHasher`, which is intentionally
//! seeded and would drift across Rust versions / processes). 16-hex-character
//! output. Collision risk at the project scale we care about (<10⁴ files) is
//! negligible.

use crate::store::PendingFile;

/// FNV-1a 64-bit. Deterministic across machines and Rust versions.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Compute the structural fingerprint of a parsed file. Same source → same
/// fingerprint, regardless of machine, Rust version, or parse order.
pub fn compute_structural_fingerprint(pending: &PendingFile) -> String {
    let mut parts: Vec<String> =
        Vec::with_capacity(1 + pending.symbols.len() + pending.edges.len());
    parts.push(format!("L|{}", pending.language));

    // Symbols — kind + name + signature. Sorted for determinism.
    let mut sym_parts: Vec<String> = pending
        .symbols
        .iter()
        .map(|s| {
            let sig = s.signature.as_deref().unwrap_or("");
            format!("S|{}|{}|{}", s.kind, s.name, sig)
        })
        .collect();
    sym_parts.sort();
    parts.extend(sym_parts);

    // Edges — kind + from-symbol-name + to-name + to-path. Sorted for
    // determinism. `from_index` is converted to a name lookup so that
    // re-ordering of the `symbols` vec doesn't perturb the hash.
    let mut edge_parts: Vec<String> = pending
        .edges
        .iter()
        .map(|e| {
            let from_sym = pending
                .symbols
                .get(e.from_index)
                .map(|s| s.name.as_str())
                .unwrap_or("?");
            format!(
                "E|{}|{}|{}|{}",
                e.kind,
                from_sym,
                e.to_name,
                e.to_path.as_deref().unwrap_or("")
            )
        })
        .collect();
    edge_parts.sort();
    parts.extend(edge_parts);

    let combined = parts.join("\n");
    format!("{:016x}", fnv1a64(combined.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{PendingEdge, PendingFile, PendingSymbol};

    fn sym(name: &str, kind: &str, sig: Option<&str>) -> PendingSymbol {
        PendingSymbol {
            name: name.into(),
            kind: kind.into(),
            line_start: 1,
            line_end: 1,
            signature: sig.map(str::to_owned),
            parent_index: None,
            decorators: None,
        }
    }

    fn edge(from_idx: usize, to_name: &str, kind: &str) -> PendingEdge {
        PendingEdge {
            from_index: from_idx,
            to_name: to_name.into(),
            to_path: None,
            to_type: None,
            kind: kind.into(),
            line: 1,
        }
    }

    #[test]
    fn fnv1a64_known_vector() {
        // FNV-1a 64-bit of empty input is the offset basis.
        assert_eq!(fnv1a64(b""), 0xcbf29ce484222325);
        // Well-known test vector: "foo" → 0xdcb27518fed9d577.
        assert_eq!(fnv1a64(b"foo"), 0xdcb27518fed9d577);
    }

    #[test]
    fn fingerprint_is_16_hex_chars() {
        let pending = PendingFile {
            path: "x.rs".into(),
            mtime: 0,
            language: "rust".into(),
            symbols: vec![sym("foo", "fn", Some("fn foo()"))],
            edges: vec![],
        };
        let fp = compute_structural_fingerprint(&pending);
        assert_eq!(fp.len(), 16);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn line_numbers_do_not_affect_fingerprint() {
        // Two PendingFiles with identical structure but different line numbers
        // must produce the same fingerprint — that's the cosmetic-edit case.
        let a = PendingFile {
            path: "x.rs".into(),
            mtime: 0,
            language: "rust".into(),
            symbols: vec![sym("foo", "fn", Some("fn foo()"))],
            edges: vec![edge(0, "bar", "calls")],
        };
        let mut b = PendingFile {
            path: "x.rs".into(),
            mtime: 99, // mtime irrelevant — not in fingerprint
            language: "rust".into(),
            symbols: vec![PendingSymbol {
                line_start: 100, // line shifted
                line_end: 200,
                ..sym("foo", "fn", Some("fn foo()"))
            }],
            edges: vec![PendingEdge {
                line: 150, // line shifted
                ..edge(0, "bar", "calls")
            }],
        };
        // Touch b to silence the unused_mut warning.
        b.symbols[0].name = "foo".into();
        assert_eq!(
            compute_structural_fingerprint(&a),
            compute_structural_fingerprint(&b),
        );
    }

    #[test]
    fn signature_change_perturbs_fingerprint() {
        let a = PendingFile {
            path: "x.rs".into(),
            mtime: 0,
            language: "rust".into(),
            symbols: vec![sym("foo", "fn", Some("fn foo()"))],
            edges: vec![],
        };
        let b = PendingFile {
            path: "x.rs".into(),
            mtime: 0,
            language: "rust".into(),
            symbols: vec![sym("foo", "fn", Some("fn foo(x: i32) -> i32"))],
            edges: vec![],
        };
        assert_ne!(
            compute_structural_fingerprint(&a),
            compute_structural_fingerprint(&b),
        );
    }

    #[test]
    fn adding_a_call_perturbs_fingerprint() {
        let a = PendingFile {
            path: "x.rs".into(),
            mtime: 0,
            language: "rust".into(),
            symbols: vec![sym("foo", "fn", Some("fn foo()"))],
            edges: vec![],
        };
        let b = PendingFile {
            path: "x.rs".into(),
            mtime: 0,
            language: "rust".into(),
            symbols: vec![sym("foo", "fn", Some("fn foo()"))],
            edges: vec![edge(0, "bar", "calls")],
        };
        assert_ne!(
            compute_structural_fingerprint(&a),
            compute_structural_fingerprint(&b),
        );
    }

    #[test]
    fn symbol_reorder_does_not_perturb_fingerprint() {
        // Parsing order shouldn't matter — sorted tuples mean any permutation
        // of the same set hashes the same.
        let a = PendingFile {
            path: "x.rs".into(),
            mtime: 0,
            language: "rust".into(),
            symbols: vec![
                sym("foo", "fn", Some("fn foo()")),
                sym("bar", "fn", Some("fn bar()")),
            ],
            edges: vec![],
        };
        let b = PendingFile {
            path: "x.rs".into(),
            mtime: 0,
            language: "rust".into(),
            symbols: vec![
                sym("bar", "fn", Some("fn bar()")),
                sym("foo", "fn", Some("fn foo()")),
            ],
            edges: vec![],
        };
        assert_eq!(
            compute_structural_fingerprint(&a),
            compute_structural_fingerprint(&b),
        );
    }
}
