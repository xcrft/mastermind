//! `mastermind verify-spec` — pre-execution gate.
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

use crate::spec::{self, ParsedSpec, SymbolClaim, TouchEntry};
use crate::store::Store;
use serde::Serialize;
use std::collections::HashSet;
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

/// Sections required only for lite mode (minimal spec).
pub const LITE_MANDATORY_SECTIONS: &[&str] = &["Goals"];

/// Return the set of mandatory sections to enforce given the spec's declared
/// mode (from frontmatter). Falls back to `MANDATORY_SECTIONS` when no mode
/// is declared (backward compat with hand-written specs that predate the
/// `mode:` field).
pub fn mandatory_sections_for_mode(mode: Option<&str>) -> &'static [&'static str] {
    match mode {
        Some("lite") => LITE_MANDATORY_SECTIONS,
        _ => MANDATORY_SECTIONS,
    }
}

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
    /// Pre-edit snapshot signature doesn't match the live index. Same staleness
    /// signal — somebody (or another commit) changed the signature between
    /// when the planner ran `mmcg_search` and now.
    SnapshotSignatureDrift {
        symbol: String,
        spec_says: String,
        index_says: Option<String>,
    },
    /// FIND block payload not present in the target file — spec is stale or
    /// the executor would fail at phase 1. Whitespace-sensitive substring match.
    FindBlockMismatch {
        file: String,
        phase: Option<String>,
        find_text_preview: String,
    },
    /// VERIFY command's first token isn't a binary on `$PATH` — `cargo test`
    /// when `cargo` isn't installed, `pnpm` when project uses `npm`, etc.
    /// Warning only — could be a project-local script the executor knows about.
    VerifyCommandNotFound { command: String, executable: String },
    /// A symbol named in `frontmatter.touches[].symbols` was not found at the
    /// declared file path. Distinct from `MissingSymbol`: this one is
    /// file-scoped, so it catches monorepo leaf-name collisions that the
    /// heuristic check would miss (`handleWebhook` exists in many controllers
    /// — but not at `src/billing/billing.controller.ts`).
    MissingSymbolAtFile {
        symbol: String,
        file: String,
        language: Option<String>,
    },
    /// A `--strict` requirement was not met (missing frontmatter, unscoped
    /// touch, no verify command, index required…). Only emitted in strict mode.
    StrictViolation { reason: String },
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

    /// Fold a hard error in and force the verdict to Fail. Used to layer
    /// `--strict` / `--require-index` findings onto a report after `run`.
    pub fn push_error(&mut self, finding: Finding) {
        self.errors.push(finding);
        self.verdict = Verdict::Fail;
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
        Finding::SnapshotSignatureDrift {
            symbol,
            spec_says,
            index_says,
        } => {
            let live = index_says.as_deref().unwrap_or("<no signature stored>");
            format!("snapshot_signature_drift: spec says `{symbol}` signature is `{spec_says}`, index says `{live}` — re-run `mmcg_search {symbol}`")
        }
        Finding::FindBlockMismatch {
            file,
            phase,
            find_text_preview,
        } => {
            let phase_label = phase.as_deref().unwrap_or("(no phase label)");
            format!("find_block_mismatch: {phase_label} → `{file}` doesn't contain the FIND text (preview: `{find_text_preview}`) — spec is stale or the file changed")
        }
        Finding::VerifyCommandNotFound {
            command,
            executable,
        } => {
            format!("verify_command_not_found: `{command}` — executable `{executable}` not on PATH")
        }
        Finding::MissingSymbolAtFile {
            symbol,
            file,
            language,
        } => {
            let lang = language.as_deref().unwrap_or("<any>");
            format!("missing_symbol_at_file: `{symbol}` not found at `{file}` (language={lang}) — file/language scoping from frontmatter.touches catches collisions the heuristic would miss")
        }
        Finding::StrictViolation { reason } => format!("strict: {reason}"),
    }
}

/// Extra requirements enforced only under `--strict`: a code-touching spec must
/// carry YAML frontmatter that scopes what it changes (`touches`/`expected_docs`),
/// scope each touch to a file, and declare at least one runnable verify command.
/// Returns the violations as `StrictViolation` findings for the caller to fold in.
pub fn strict_check(spec: &ParsedSpec) -> Vec<Finding> {
    let mut out = Vec::new();
    match &spec.frontmatter {
        None => out.push(Finding::StrictViolation {
            reason: "no YAML frontmatter — a strict spec needs a `---` block with touches / verify / breaking_changes".into(),
        }),
        Some(fm) => {
            if !fm.has_file_scope() {
                out.push(Finding::StrictViolation {
                    reason: "frontmatter declares no `touches` or `expected_docs` — scope what the spec changes".into(),
                });
            }
            for t in &fm.touches {
                if t.file.trim().is_empty() {
                    out.push(Finding::StrictViolation {
                        reason: "a `touches` entry has an empty `file`".into(),
                    });
                } else if t.symbols.is_empty() {
                    out.push(Finding::StrictViolation {
                        reason: format!(
                            "touch `{}` names no `symbols` — list the symbols it changes",
                            t.file
                        ),
                    });
                }
            }
            if fm.verify.is_empty() && spec.verify_commands.is_empty() {
                out.push(Finding::StrictViolation {
                    reason: "no verify command — declare at least one `verify[].cmd` the executor must run".into(),
                });
            }
        }
    }
    out
}

/// Run all Phase A checks against a parsed spec, using `store` as the live
/// truth source and `repo_root` to resolve file existence. `store` is
/// optional — when `None` the symbol-existence + blast-radius checks are
/// skipped (use case: running verify-spec outside an indexed project).
pub fn run(spec: &ParsedSpec, store: Option<&Store>, repo_root: &Path) -> Report {
    let mut errors: Vec<Finding> = Vec::new();
    let mut warnings: Vec<Finding> = Vec::new();

    // 1. Mandatory sections non-empty.
    let spec_mode = spec
        .frontmatter
        .as_ref()
        .and_then(|f| f.mode.as_deref());
    for section in mandatory_sections_for_mode(spec_mode) {
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
    //    Frontmatter-authoritative when present: if frontmatter declares
    //    `touches[]` or `expected_docs[]`, use ONLY that list. Heuristic
    //    backticked-path-token extraction is too noisy for gates (it picks
    //    up prose mentions like ``do not touch `README.md` `` as claims).
    //    When frontmatter is absent (or has no file-scope fields), fall back
    //    to the heuristic mentioned_files for backward compat.
    let files_to_check: Vec<String> = match spec.frontmatter.as_ref() {
        Some(fm) if fm.has_file_scope() => {
            let mut seen: HashSet<String> = HashSet::new();
            let mut out: Vec<String> = Vec::new();
            for touch in &fm.touches {
                if seen.insert(touch.file.clone()) {
                    out.push(touch.file.clone());
                }
            }
            for doc in &fm.expected_docs {
                if seen.insert(doc.clone()) {
                    out.push(doc.clone());
                }
            }
            out
        }
        _ => spec.mentioned_files.clone(),
    };
    for rel in &files_to_check {
        let abs = repo_root.join(rel);
        if !abs.exists() {
            errors.push(Finding::MissingFile { file: rel.clone() });
        }
    }

    // 3. Pre-edit snapshot symbols: existence, caller-count drift, blast radius.
    //    Two sources, both contribute findings:
    //    a) heuristic `## Pre-edit symbol snapshot` bullets (name-only search)
    //    b) frontmatter `touches[].symbols` with file+language scoping —
    //       catches monorepo collisions the heuristic misses.
    if let Some(store) = store {
        for claim in &spec.pre_edit_snapshot {
            check_symbol_claim(claim, store, &mut errors, &mut warnings);
        }
        if let Some(fm) = &spec.frontmatter {
            for touch in &fm.touches {
                check_frontmatter_touch(touch, store, &mut errors, &mut warnings);
            }
        }
    }

    // 4. FIND blocks — for every block with a target file, the FIND text must
    //    be a literal substring of the current file contents. Stale FIND ⇒
    //    executor fails at phase 1, so this is `error`, not warning.
    for block in &spec.find_blocks {
        check_find_block(block, repo_root, &mut errors);
    }

    // 5. VERIFY commands — first token resolvable on `$PATH`. Soft warn:
    //    might be a project-local script (`./scripts/check.sh`) which would
    //    look like an unresolved binary but is fine. Includes both heuristic
    //    phase-block `**VERIFY**: ...` lines AND frontmatter `verify[]` entries
    //    with `cmd:` form (label-only entries are informational, skipped).
    let mut all_commands: Vec<String> = spec.verify_commands.clone();
    if let Some(fm) = &spec.frontmatter {
        for entry in &fm.verify {
            if let Some(cmd) = entry.command() {
                all_commands.push(cmd.to_string());
            }
        }
    }
    // De-dup while preserving order.
    let mut seen_cmds: HashSet<String> = HashSet::new();
    for cmd in &all_commands {
        if seen_cmds.insert(cmd.clone()) {
            check_verify_command(cmd, &mut warnings);
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
    let live_callers = match store.callers_of(&claim.name, None, None) {
        Ok(callers) => callers.len() as u32,
        Err(_) => return,
    };
    if let Some(spec_count) = claim.callers {
        if spec_count != live_callers {
            errors.push(Finding::SnapshotCallerCountDrift {
                symbol: claim.name.clone(),
                spec_says: spec_count,
                index_says: live_callers,
            });
        }
    }
    // Signature check — if the bullet recorded a signature, the live signature
    // of any matching symbol must equal it. Multiple matches (e.g. C# partial
    // class collisions, monorepo same-name in two languages) → pass if ANY
    // matches; the planner has the language filter to disambiguate elsewhere.
    if let Some(spec_sig) = &claim.signature {
        let live_sigs: Vec<Option<String>> = hits.iter().map(|s| s.signature.clone()).collect();
        let any_match = live_sigs
            .iter()
            .any(|s| s.as_deref() == Some(spec_sig.as_str()));
        if !any_match {
            errors.push(Finding::SnapshotSignatureDrift {
                symbol: claim.name.clone(),
                spec_says: spec_sig.clone(),
                index_says: live_sigs.into_iter().flatten().next(),
            });
        }
    }
    if live_callers >= BLAST_RADIUS_WARN {
        warnings.push(Finding::LargeBlastRadius {
            symbol: claim.name.clone(),
            callers: live_callers,
            threshold: BLAST_RADIUS_WARN,
        });
    }
}

/// Validate one `frontmatter.touches[]` entry — the symbols listed must exist
/// at the declared file path (and language, if given). Catches the leaf-name
/// collision class of false positive that the heuristic `pre_edit_snapshot`
/// path can't see: `handleWebhook` exists in many controllers, but the spec
/// says THIS one is at `src/billing/billing.controller.ts`.
fn check_frontmatter_touch(
    touch: &TouchEntry,
    store: &Store,
    errors: &mut Vec<Finding>,
    warnings: &mut Vec<Finding>,
) {
    for sym in &touch.symbols {
        let name = sym.name();
        // Inherit file/language from the touch entry unless the Detailed
        // variant overrides them.
        let file = sym.file().unwrap_or(touch.file.as_str());
        let language = sym.language().or(touch.language.as_deref());

        let hits = match store.search_symbols(name, None, language) {
            Ok(rows) => rows,
            Err(_) => continue, // store error — verify_spec already surfaces store failures elsewhere
        };
        let scoped: Vec<_> = hits.into_iter().filter(|s| s.file_path == file).collect();
        if scoped.is_empty() {
            errors.push(Finding::MissingSymbolAtFile {
                symbol: name.to_string(),
                file: file.to_string(),
                language: language.map(str::to_string),
            });
            continue;
        }
        // Caller-count drift, same any-file scoping rule as the heuristic
        // path. (Caller counts are inherently cross-file; we filter the symbol
        // hit by file for existence, but don't try to attribute callers to a
        // specific definition.)
        if let Some(declared) = sym.callers() {
            let live = match store.callers_of(name, language, None) {
                Ok(callers) => callers.len() as u32,
                Err(_) => continue,
            };
            if declared != live {
                errors.push(Finding::SnapshotCallerCountDrift {
                    symbol: name.to_string(),
                    spec_says: declared,
                    index_says: live,
                });
            }
            if live >= BLAST_RADIUS_WARN {
                warnings.push(Finding::LargeBlastRadius {
                    symbol: name.to_string(),
                    callers: live,
                    threshold: BLAST_RADIUS_WARN,
                });
            }
        }
        // Signature drift — must match at least one scoped hit.
        if let Some(declared_sig) = sym.signature() {
            let live_sigs: Vec<Option<String>> =
                scoped.iter().map(|s| s.signature.clone()).collect();
            let any_match = live_sigs.iter().any(|s| s.as_deref() == Some(declared_sig));
            if !any_match {
                errors.push(Finding::SnapshotSignatureDrift {
                    symbol: name.to_string(),
                    spec_says: declared_sig.to_string(),
                    index_says: live_sigs.into_iter().flatten().next(),
                });
            }
        }
    }
}

/// FIND-block validation: the planner's `FIND:` payload must appear as a
/// substring of the target file. Whitespace-sensitive — matching the executor's
/// reality (it does literal replace).
fn check_find_block(block: &crate::spec::FindBlock, repo_root: &Path, errors: &mut Vec<Finding>) {
    let Some(file) = &block.file else {
        // No `**File:**` marker — can't validate. Silent skip; verify-spec's
        // mandatory-file-mentions check (#2) catches the typical case.
        return;
    };
    let abs = repo_root.join(file);
    let Ok(body) = std::fs::read_to_string(&abs) else {
        // Missing file is already flagged by check #2; don't double-report.
        return;
    };
    if !body.contains(&block.find_text) {
        let preview: String = block
            .find_text
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(80)
            .collect();
        errors.push(Finding::FindBlockMismatch {
            file: file.clone(),
            phase: block.phase.clone(),
            find_text_preview: preview,
        });
    }
}

/// Check that the first token of a VERIFY command resolves on `$PATH`. Skips
/// shell-syntactic intros (`cd …`, `pushd …`) and project-local paths starting
/// with `./` or `/` — both are common and not actually missing.
fn check_verify_command(command: &str, warnings: &mut Vec<Finding>) {
    let first = match command.split_whitespace().next() {
        Some(t) => t,
        None => return,
    };
    // Skip project-local paths (`./scripts/foo.sh`) — they're files, not
    // PATH-resolved binaries. Existence is the executor's problem, not ours.
    if first.starts_with("./") || first.starts_with('/') {
        return;
    }
    // Skip shell builtins that wrap a real command.
    if matches!(first, "cd" | "pushd" | "popd" | "exec" | "env" | "time") {
        return;
    }
    if which_on_path(first).is_some() {
        return;
    }
    warnings.push(Finding::VerifyCommandNotFound {
        command: command.to_string(),
        executable: first.to_string(),
    });
}

/// Bare-bones `which(1)` — walks `$PATH`, checks each entry for the binary.
/// Returns the resolved absolute path or None.
fn which_on_path(binary: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
        // Windows: try common executable extensions if no extension present.
        if cfg!(windows) && !binary.contains('.') {
            for ext in ["exe", "cmd", "bat"] {
                let c = dir.join(format!("{binary}.{ext}"));
                if c.is_file() {
                    return Some(c);
                }
            }
        }
    }
    None
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
        use std::sync::atomic::{AtomicU64, Ordering};
        // See `doctor.rs::tmp()` — same parallel-collision fix. The atomic
        // counter guarantees a distinct directory per call; `process::id()` +
        // nanos remain for cross-process uniqueness and debuggability.
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "mmcg-verify-{}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
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
    fn flags_stale_find_block_against_file() {
        let root = tmp();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/foo.rs"), "fn renamed() {}\n").unwrap();
        let body = "\
## Goals
1. Touch `src/foo.rs`
## Alternatives Considered
- A
## Tests Plan
- t
## Documentation Plan
- d
## Observability Plan
- n/a
## Performance Considerations
- O(1)
## Phase 1: change
### 1.1 edit
**File:** `src/foo.rs`
FIND:
```rust
fn old_name() {}
```
";
        let s = spec::parse_str("t.md", body);
        let r = run(&s, None, &root);
        assert!(r.errors.iter().any(|e| matches!(
            e,
            Finding::FindBlockMismatch { file, .. } if file == "src/foo.rs"
        )));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn verify_command_check_handles_local_paths_and_missing_bins() {
        let root = tmp();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/foo.rs"), "// stub").unwrap();
        let body = "\
## Goals
1. Edit `src/foo.rs`
## Alternatives Considered
- A
## Tests Plan
- t
## Documentation Plan
- d
## Observability Plan
- n/a
## Performance Considerations
- O(1)
## Phase 1: x
**File:** `src/foo.rs`
FIND:
```
// stub
```
VERIFY: `./scripts/local-script.sh`
VERIFY: `definitely-not-on-path-12345`
";
        let s = spec::parse_str("t.md", body);
        let r = run(&s, None, &root);
        // ./scripts/... is project-local — no warning expected
        let local_warned = r.warnings.iter().any(|w| {
            matches!(
                w,
                Finding::VerifyCommandNotFound { executable, .. } if executable.starts_with("./")
            )
        });
        assert!(!local_warned, "./scripts/ paths should not be PATH-checked");
        // The missing-binary one should warn
        assert!(r.warnings.iter().any(|w| matches!(
            w,
            Finding::VerifyCommandNotFound { executable, .. } if executable == "definitely-not-on-path-12345"
        )));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn frontmatter_touch_catches_file_scoped_missing_symbol() {
        use crate::indexer::Indexer;
        let root = tmp();
        // Two files with the SAME leaf name `handleWebhook` — monorepo collision.
        fs::create_dir_all(root.join("src/billing")).unwrap();
        fs::create_dir_all(root.join("src/legacy")).unwrap();
        // The "right" file does NOT contain handleWebhook (yet) — spec is wrong
        // about where it lives. Legacy does. Heuristic check would pass
        // (handleWebhook exists somewhere); scoped check should fail.
        fs::write(
            root.join("src/billing/billing.ts"),
            "export function unrelated() {}\n",
        )
        .unwrap();
        fs::write(
            root.join("src/legacy/legacy.ts"),
            "export function handleWebhook(req, res) {}\n",
        )
        .unwrap();

        let db = root.join("idx.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&root).index_all(&mut store, false).unwrap();
        drop(store);

        // Frontmatter declares handleWebhook lives in billing.ts.
        // Mandatory sections all present so we isolate the frontmatter check.
        let body = "---
id: \"1\"
touches:
  - file: src/billing/billing.ts
    language: typescript
    symbols:
      - name: handleWebhook
---

## Goals
- Add `handleWebhook` to `src/billing/billing.ts`
## Alternatives Considered
- a — rejected
## Tests Plan
- t
## Documentation Plan
- d
## Observability Plan
- n/a
## Performance Considerations
- O(1)
";
        let s = spec::parse_str("t.md", body);
        let store = Store::open(&db).unwrap();
        let r = run(&s, Some(&store), &root);
        // Heuristic would PASS (handleWebhook exists somewhere). Frontmatter
        // scoped check must fail.
        assert!(
            r.errors.iter().any(|e| matches!(
                e,
                Finding::MissingSymbolAtFile { symbol, file, .. }
                    if symbol == "handleWebhook" && file == "src/billing/billing.ts"
            )),
            "expected MissingSymbolAtFile finding; got {:?}",
            r.errors
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn frontmatter_verify_cmd_is_path_checked() {
        let root = tmp();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/x.rs"), "fn x(){}").unwrap();
        let body = "---
id: \"1\"
verify:
  - typecheck
  - cmd: \"definitely-not-on-path-87654\"
---

## Goals
- Edit `src/x.rs`
## Alternatives Considered
- A
## Tests Plan
- t
## Documentation Plan
- d
## Observability Plan
- n/a
## Performance Considerations
- O(1)
";
        let s = spec::parse_str("t.md", body);
        let r = run(&s, None, &root);
        // Label-only `typecheck` should NOT warn (no cmd). The `cmd:` entry
        // for the bogus binary SHOULD warn.
        assert!(r.warnings.iter().any(|w| matches!(
            w,
            Finding::VerifyCommandNotFound { executable, .. } if executable == "definitely-not-on-path-87654"
        )));
        assert!(!r.warnings.iter().any(|w| matches!(
            w,
            Finding::VerifyCommandNotFound { executable, .. } if executable == "typecheck"
        )));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn frontmatter_authoritative_ignores_prose_path_mentions() {
        // Regression: prose mentions like ``do not touch `README.md` ``
        // used to flag README.md as a claimed file (heuristic union path).
        // When frontmatter declares file scope, the heuristic mentioned_files
        // is ignored — frontmatter is authoritative.
        let root = tmp();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/x.rs"), "fn x(){}").unwrap();
        // No README.md on disk — the prose mention would flag MissingFile
        // under the old union behavior.
        let body = "---
id: \"1\"
touches:
  - file: src/x.rs
---

## Goals
- Edit `src/x.rs`. Do not touch `README.md` or `docs/guide.md`.
## Alternatives Considered
- A
## Tests Plan
- t
## Documentation Plan
- d
## Observability Plan
- n/a
## Performance Considerations
- O(1)
";
        let s = spec::parse_str("t.md", body);
        let r = run(&s, None, &root);
        // Heuristic would have flagged README.md and docs/guide.md as missing.
        // Frontmatter authoritative → only src/x.rs is in scope (and exists).
        assert!(
            !r.errors
                .iter()
                .any(|e| matches!(e, Finding::MissingFile { file } if file == "README.md")),
            "prose mention of README.md should not be treated as a claimed file"
        );
        assert!(
            !r.errors
                .iter()
                .any(|e| matches!(e, Finding::MissingFile { file } if file == "docs/guide.md")),
            "prose mention of docs/guide.md should not be treated as a claimed file"
        );
        assert_eq!(r.verdict, Verdict::Pass);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn frontmatter_expected_docs_are_existence_checked() {
        let root = tmp();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/x.rs"), "fn x(){}").unwrap();
        // README.md exists, docs/missing.md doesn't.
        fs::write(root.join("README.md"), "# x").unwrap();
        let body = "---
id: \"1\"
expected_docs:
  - README.md
  - docs/missing.md
---

## Goals
- Edit `src/x.rs`
## Alternatives Considered
- A
## Tests Plan
- t
## Documentation Plan
- d
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
            .any(|e| matches!(e, Finding::MissingFile { file } if file == "docs/missing.md")));
        assert!(!r
            .errors
            .iter()
            .any(|e| matches!(e, Finding::MissingFile { file } if file == "README.md")));
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

    #[test]
    fn strict_check_flags_spec_without_frontmatter() {
        let s = spec::parse_str("t.md", "## Goals\n- x\n## Tests Plan\n- t\n");
        let findings = strict_check(&s);
        assert!(
            findings.iter().any(|f| matches!(
                f,
                Finding::StrictViolation { reason } if reason.contains("frontmatter")
            )),
            "expected a frontmatter strict violation, got {findings:?}"
        );
    }

    #[test]
    fn strict_check_passes_a_well_scoped_frontmatter_spec() {
        let body = "\
---
touches:
  - file: src/foo.rs
    language: rust
    symbols:
      - name: foo
verify:
  - cmd: cargo test
---
## Goals
- x
";
        let s = spec::parse_str("t.md", body);
        assert!(
            strict_check(&s).is_empty(),
            "well-scoped frontmatter spec should pass strict, got {:?}",
            strict_check(&s)
        );
    }

    #[test]
    fn lite_mode_goals_section_passes() {
        let root = tmp();
        let body = "\
---
id: \"1\"
mode: lite
---

## Goals

Do the thing

## Scope

- **File:** `src/foo.rs`
";
        let s = spec::parse_str("t.md", body);
        let r = run(&s, None, &root);
        assert!(
            !r.errors.iter().any(|e| matches!(
                e,
                Finding::EmptyMandatorySection { section } if section == "Goals"
            )),
            "lite spec with ## Goals populated should NOT flag Goals as missing; got {:?}",
            r.errors
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn lite_mode_does_not_require_standard_sections() {
        let root = tmp();
        let body = "\
---
id: \"1\"
mode: lite
---

## Goals

Do the thing
";
        let s = spec::parse_str("t.md", body);
        let r = run(&s, None, &root);
        let standard_only = [
            "Alternatives Considered",
            "Tests Plan",
            "Documentation Plan",
            "Observability Plan",
            "Performance Considerations",
        ];
        for section in &standard_only {
            assert!(
                !r.errors.iter().any(|e| matches!(
                    e,
                    Finding::EmptyMandatorySection { section: s } if s == section
                )),
                "lite mode should not require `{section}`; got {:?}",
                r.errors
            );
        }
        fs::remove_dir_all(&root).ok();
    }
}
