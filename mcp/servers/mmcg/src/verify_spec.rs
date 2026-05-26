//! `mmcg verify-spec` — pre-execution gate.
//!
//! Replaces the prompt-discipline checks the planner is supposed to do before
//! handing off to the executor with **deterministic, mechanical** checks. The
//! LLM auditor still handles semantic judgment; this catches the symbols-don't-
//! exist / files-don't-exist / required-section-empty / oversized-blast-radius
//! class of bugs at the contract level.
//!
//! Verdict semantics:
//! - `pass`  — no errors, no warnings
//! - `warn`  — warnings only (e.g. large blast radius)
//! - `fail`  — at least one error (missing symbol, missing file, empty
//!   mandatory section)
//!
//! Exit code maps: 0 / 0 / 1. Warnings do NOT fail the gate by design — a
//! 38-caller blast radius is a flag for the planner to read, not a block.

use crate::spec::{self, ParsedSpec, SymbolClaim};
use crate::store::Store;
use serde::Serialize;
use std::path::Path;

/// Threshold above which `blast_radius` becomes a warning. 30 is empirical —
/// touching a function with >30 callers is rarely "small" and the planner
/// should at least acknowledge the impact in Notes.
pub const BLAST_RADIUS_WARN: u32 = 30;

/// Sections the spec template marks `*(MANDATORY ...)*` (modulo "for
/// non-trivial work" / "for code that runs in production" qualifiers we can't
/// evaluate). All must be present AND non-empty for verdict to be `pass`.
pub const MANDATORY_SECTIONS: &[&str] = &[
    "Goals",
    "Alternatives Considered",
    "Tests Plan",
    "Documentation Plan",
    "Observability Plan",
    "Performance Considerations",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Pass,
    Warn,
    Fail,
}

/// Tagged finding. `kind` is the machine-readable category, fields are the
/// specific evidence. JSON-friendly (serde flattens enum + extra fields).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Finding {
    /// Spec names a symbol the index doesn't know about.
    MissingSymbol {
        symbol: String,
        section: String,
        raw: String,
    },
    /// Spec references a file path that doesn't exist on disk.
    MissingFile { file: String },
    /// A `## ...` section the template marks MANDATORY is missing or empty.
    EmptyMandatorySection { section: String },
    /// Pre-edit snapshot symbol has many callers — proceed with awareness.
    LargeBlastRadius {
        symbol: String,
        callers: u32,
        threshold: u32,
    },
    /// Pre-edit snapshot caller count doesn't match the live index. Pre-edit
    /// snapshots go stale; planner should re-grab via `mmcg_callers` before
    /// handing off.
    SnapshotCallerCountDrift {
        symbol: String,
        spec_says: u32,
        index_says: u32,
    },
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub spec: String,
    pub verdict: Verdict,
    pub errors: Vec<Finding>,
    pub warnings: Vec<Finding>,
}

impl Report {
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        let marker = match self.verdict {
            Verdict::Pass => "✅",
            Verdict::Warn => "⚠️",
            Verdict::Fail => "❌",
        };
        out.push_str(&format!(
            "{marker} {:?} — {}\n  errors: {}, warnings: {}\n\n",
            self.verdict,
            self.spec,
            self.errors.len(),
            self.warnings.len(),
        ));
        for e in &self.errors {
            out.push_str(&format!("  ❌ {}\n", render_finding(e)));
        }
        for w in &self.warnings {
            out.push_str(&format!("  ⚠️  {}\n", render_finding(w)));
        }
        out
    }

    pub fn has_failures(&self) -> bool {
        !self.errors.is_empty()
    }
}

fn render_finding(f: &Finding) -> String {
    match f {
        Finding::MissingSymbol {
            symbol, section, ..
        } => {
            format!("missing_symbol: `{symbol}` (claimed in {section}) — `mmcg_search {symbol}` returns nothing")
        }
        Finding::MissingFile { file } => format!("missing_file: `{file}` not on disk"),
        Finding::EmptyMandatorySection { section } => {
            format!("empty_mandatory_section: `{section}` is missing or empty")
        }
        Finding::LargeBlastRadius {
            symbol,
            callers,
            threshold,
        } => {
            format!(
                "large_blast_radius: `{symbol}` has {callers} callers (warn threshold {threshold})"
            )
        }
        Finding::SnapshotCallerCountDrift {
            symbol,
            spec_says,
            index_says,
        } => {
            format!("snapshot_drift: spec says `{symbol}` has {spec_says} callers, index says {index_says} — re-run `mmcg_callers {symbol}`")
        }
    }
}

/// Run all Phase A checks against a parsed spec, using `store` as the live
/// truth source and `repo_root` to resolve file existence. `store` is
/// optional — when `None` the symbol-existence + blast-radius checks are
/// skipped (use case: running verify-spec outside an indexed project).
pub fn run(spec: &ParsedSpec, store: Option<&Store>, repo_root: &Path) -> Report {
    let mut errors: Vec<Finding> = Vec::new();
    let mut warnings: Vec<Finding> = Vec::new();

    // 1. Mandatory sections non-empty.
    for section in MANDATORY_SECTIONS {
        match spec::section_body(spec, section) {
            None => errors.push(Finding::EmptyMandatorySection {
                section: section.to_string(),
            }),
            Some(body) => {
                if body_is_effectively_empty(body) {
                    errors.push(Finding::EmptyMandatorySection {
                        section: section.to_string(),
                    });
                }
            }
        }
    }

    // 2. Mentioned files exist on disk.
    for rel in &spec.mentioned_files {
        let abs = repo_root.join(rel);
        if !abs.exists() {
            errors.push(Finding::MissingFile { file: rel.clone() });
        }
    }

    // 3. Pre-edit snapshot symbols: existence, caller-count drift, blast radius.
    if let Some(store) = store {
        for claim in &spec.pre_edit_snapshot {
            check_symbol_claim(claim, store, &mut errors, &mut warnings);
        }
    }

    let verdict = if !errors.is_empty() {
        Verdict::Fail
    } else if !warnings.is_empty() {
        Verdict::Warn
    } else {
        Verdict::Pass
    };
    Report {
        spec: spec.path.clone(),
        verdict,
        errors,
        warnings,
    }
}

fn check_symbol_claim(
    claim: &SymbolClaim,
    store: &Store,
    errors: &mut Vec<Finding>,
    warnings: &mut Vec<Finding>,
) {
    let hits = match store.search_symbols(&claim.name, None, None) {
        Ok(rows) => rows,
        Err(_) => return,
    };
    if hits.is_empty() {
        errors.push(Finding::MissingSymbol {
            symbol: claim.name.clone(),
            section: "Pre-edit symbol snapshot".to_string(),
            raw: claim.raw.clone(),
        });
        return;
    }
    // Caller-count check — compare spec's stated count against the live index.
    let live = match store.callers_of(&claim.name, None, None) {
        Ok(callers) => callers.len() as u32,
        Err(_) => return,
    };
    if let Some(spec_count) = claim.callers {
        if spec_count != live {
            errors.push(Finding::SnapshotCallerCountDrift {
                symbol: claim.name.clone(),
                spec_says: spec_count,
                index_says: live,
            });
        }
    }
    if live >= BLAST_RADIUS_WARN {
        warnings.push(Finding::LargeBlastRadius {
            symbol: claim.name.clone(),
            callers: live,
            threshold: BLAST_RADIUS_WARN,
        });
    }
}

/// Section body is effectively empty when it's whitespace, just the template
/// placeholder bullets (`- <thing>` with angle-bracket placeholders), or
/// commentary HTML comments only.
fn body_is_effectively_empty(body: &str) -> bool {
    let stripped: String = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| !l.starts_with("<!--"))
        // Drop unchanged template placeholders: lines that contain angle-bracket
        // hints like `- <Alt 1 short name>` or `<symbol>`.
        .filter(|l| {
            let open = l.matches('<').count();
            let close = l.matches('>').count();
            !(open > 0 && open == close)
        })
        .collect::<Vec<_>>()
        .join("\n");
    stripped.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec;
    use std::fs;
    use std::path::PathBuf;

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "mmcg-verify-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn fails_when_mandatory_sections_empty() {
        let root = tmp();
        let body = "# T\n## Goals\nx\n## Tests Plan\n\n## Documentation Plan\n";
        let s = spec::parse_str("t.md", body);
        let r = run(&s, None, &root);
        assert_eq!(r.verdict, Verdict::Fail);
        // Empty section + missing ones.
        assert!(r.errors.iter().any(
            |e| matches!(e, Finding::EmptyMandatorySection { section } if section == "Tests Plan")
        ));
        assert!(r.errors.iter().any(|e| matches!(e, Finding::EmptyMandatorySection { section } if section == "Alternatives Considered")));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn fails_when_mentioned_file_missing() {
        let root = tmp();
        // Spec mentions a file that doesn't exist + valid mandatory sections.
        let body = "\
## Goals
Edit `src/missing.rs`
## Alternatives Considered
- a — rejected
## Tests Plan
- test_x
## Documentation Plan
- update README.md
## Observability Plan
- n/a
## Performance Considerations
- O(1)
";
        let s = spec::parse_str("t.md", body);
        let r = run(&s, None, &root);
        assert!(r
            .errors
            .iter()
            .any(|e| matches!(e, Finding::MissingFile { file } if file == "src/missing.rs")));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn template_placeholder_bullets_count_as_empty() {
        let root = tmp();
        // Section body is only template placeholders — should count as empty.
        let body = "\
## Goals
1. Real goal
## Alternatives Considered
- <Alt 1 short name> — rejected because <concrete reason>
- <Alt 2 short name> — rejected because <reason>
## Tests Plan
- test_x
## Documentation Plan
- update README
## Observability Plan
- n/a
## Performance Considerations
- O(1)
";
        let s = spec::parse_str("t.md", body);
        let r = run(&s, None, &root);
        assert!(r
            .errors
            .iter()
            .any(|e| matches!(e, Finding::EmptyMandatorySection { section } if section == "Alternatives Considered")));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn passes_clean_spec_without_store() {
        let root = tmp();
        // Create the file the spec mentions so MissingFile doesn't trigger.
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/foo.rs"), "fn x() {}").unwrap();
        let body = "\
## Goals
1. Add accessor in `src/foo.rs`
## Alternatives Considered
- A — rejected: reason
## Tests Plan
- test_x
## Documentation Plan
- update README
## Observability Plan
- n/a
## Performance Considerations
- O(1)
";
        let s = spec::parse_str("t.md", body);
        let r = run(&s, None, &root);
        assert_eq!(r.verdict, Verdict::Pass);
        fs::remove_dir_all(&root).ok();
    }
}
