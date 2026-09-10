//! `mastermind verify-spec` — pre-execution gate.
//!
//! **Deterministic, mechanical** version of the planner's pre-handoff
//! prompt-discipline checks. The planner still does semantic judgment; this
//! catches the symbol-missing / file-missing / section-empty / oversized-blast-
//! radius class of bugs at the contract level.
//!
//! Verdict semantics:
//! - `pass`  — no errors, no warnings
//! - `warn`  — warnings only (e.g. large blast radius)
//! - `fail`  — ≥1 error (missing symbol, missing file, empty mandatory section)
//!
//! Exit codes: 0 / 0 / 1. Warnings don't fail the gate by design — a 38-caller
//! blast radius is a flag for the planner to read, not a block.

use crate::spec::{self, ParsedSpec, SymbolClaim, TouchEntry};
use crate::spec_removals;
use crate::spec_symbols::{self, Resolved, Scope, Unresolved};
use crate::store::Store;
use serde::Serialize;
use std::path::Path;
use std::time::Instant;

/// `blast_radius` warning threshold. 30 is empirical — touching a function with
/// >30 callers is rarely "small"; the planner should acknowledge it in Notes.
pub const BLAST_RADIUS_WARN: u32 = 30;

/// Sections the spec template marks `*(MANDATORY ...)*` (minus "for non-trivial
/// work" / "for production" qualifiers we can't evaluate). All must be present
/// AND non-empty for verdict `pass`.
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
pub const VERIFIED_MANDATORY_SECTIONS: &[&str] = &[
    "Goals",
    "Scope",
    "Acceptance Criteria",
    "Tests Plan",
    "Final Verification",
];

/// Mandatory sections to enforce for the spec's declared `mode` (frontmatter).
/// Falls back to `MANDATORY_SECTIONS` when no mode is declared (back-compat with
/// hand-written specs predating the `mode:` field).
pub fn mandatory_sections_for_mode(mode: Option<&str>) -> &'static [&'static str] {
    match mode {
        Some("lite") => LITE_MANDATORY_SECTIONS,
        Some("verified") => VERIFIED_MANDATORY_SECTIONS,
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

/// Tagged finding: `kind` is the machine-readable category, fields are the
/// evidence. JSON-friendly (serde flattens enum + extra fields).
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
    /// A declared path cannot establish a contained, stable regular file.
    DeclaredFileUnavailable {
        file: Option<String>,
        reason: String,
    },
    /// A `## ...` section the template marks MANDATORY is missing or empty.
    EmptyMandatorySection { section: String },
    /// Pre-edit snapshot symbol has many callers — proceed with awareness.
    LargeBlastRadius {
        symbol: String,
        callers: u32,
        threshold: u32,
    },
    /// Pre-edit snapshot caller count doesn't match the live index. Snapshots go
    /// stale; planner should re-grab via `mmcg_callers` before handing off.
    SnapshotCallerCountDrift {
        symbol: String,
        spec_says: u32,
        index_says: u32,
    },
    /// Pre-edit snapshot signature doesn't match the live index. Same staleness
    /// signal — something changed the signature between the planner's
    /// `mmcg_search` and now.
    SnapshotSignatureDrift {
        symbol: String,
        spec_says: String,
        index_says: Option<String>,
    },
    /// The declared snapshot has no unique, usable identity in the index.
    SnapshotUnresolved {
        symbol: String,
        reason: String,
        matches: Option<usize>,
    },
    /// FIND block payload not in the target file — spec is stale or the executor
    /// fails at phase 1. Whitespace-sensitive substring match.
    FindBlockMismatch {
        file: String,
        phase: Option<String>,
        find_text_preview: String,
    },
    /// The literal precondition could not be checked against a stable, bounded input.
    FindBlockUnavailable {
        file: Option<String>,
        phase: Option<String>,
        reason: String,
    },
    /// VERIFY command's first token isn't a binary on `$PATH` — `cargo test`
    /// when `cargo` isn't installed, `pnpm` when the project uses `npm`, etc.
    /// Warning only — could be a project-local script the executor knows about.
    VerifyCommandNotFound { command: String, executable: String },
    /// A `frontmatter.touches[].symbols` symbol not found at the declared file
    /// path. Unlike `MissingSymbol`, this is file-scoped, so it catches monorepo
    /// leaf-name collisions the heuristic misses (`handleWebhook` exists in many
    /// controllers — but not at `src/billing/billing.controller.ts`).
    MissingSymbolAtFile {
        symbol: String,
        file: String,
        language: Option<String>,
    },
    /// A `--strict` requirement unmet (missing frontmatter, unscoped touch, no
    /// verify command, index required…). Only emitted in strict mode.
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

    /// Fold a hard error in and force verdict to Fail. Layers `--strict` /
    /// `--require-index` findings onto a report after `run`.
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
            format!("missing_symbol: {symbol} (claimed in {section}) — no declaration matches its scope")
        }
        Finding::MissingFile { file } => format!("missing_file: `{file}` not on disk"),
        Finding::DeclaredFileUnavailable { file, reason } => {
            format!("declared_file_unavailable: `{}` — regular file could not be established ({reason})", file.as_deref().unwrap_or("<no valid target>"))
        }
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
            format!("snapshot_drift: spec says {symbol} has {spec_says} callers, index says {index_says} — refresh the snapshot caller count")
        }
        Finding::SnapshotSignatureDrift {
            symbol,
            spec_says,
            index_says,
        } => {
            let live = index_says.as_deref().unwrap_or("<no signature stored>");
            format!("snapshot_signature_drift: spec says {symbol} signature is {spec_says}, index says {live} — refresh the declaration snapshot")
        }
        Finding::SnapshotUnresolved {
            symbol,
            reason,
            matches,
        } => {
            let count = matches
                .map(|count| format!(" ({count} matching declarations)"))
                .unwrap_or_default();
            format!("snapshot_unresolved: {symbol}: {reason}{count} — verify the index and declaration scope")
        }
        Finding::FindBlockMismatch {
            file,
            phase,
            find_text_preview,
        } => {
            let phase_label = phase.as_deref().unwrap_or("(no phase label)");
            format!("find_block_mismatch: {phase_label} → `{file}` doesn't contain the FIND text (preview: `{find_text_preview}`) — spec is stale or the file changed")
        }
        Finding::FindBlockUnavailable {
            file,
            phase,
            reason,
        } => {
            let file = file.as_deref().unwrap_or("<no target file>");
            let phase = phase.as_deref().unwrap_or("(no phase label)");
            format!(
                "find_block_unavailable: {phase} → `{file}` — FIND could not be checked ({reason})"
            )
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
            format!("missing_symbol_at_file: {symbol} not found at {file} (language={lang})")
        }
        Finding::StrictViolation { reason } => format!("strict: {reason}"),
    }
}

/// `--strict`-only requirements: a code-touching spec must carry YAML
/// frontmatter scoping what it changes (`touches`/`expected_docs`), scope each
/// touch to a file, and declare ≥1 runnable verify command. Returns violations
/// as `StrictViolation` findings for the caller to fold in.
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
            if spec.declared_verify_commands().is_empty() {
                out.push(Finding::StrictViolation {
                    reason: "no verify command — declare at least one `verify[].cmd` the executor must run".into(),
                });
            }
        }
    }
    out
}

/// Run all Phase A checks against a parsed spec, using `store` as live truth and
/// `repo_root` to resolve file existence. `store` optional — `None` skips the
/// symbol-existence + blast-radius checks (verify-spec outside an indexed project).
pub fn run(spec: &ParsedSpec, store: Option<&Store>, repo_root: &Path) -> Report {
    run_internal(spec, store, repo_root, Phase::Preflight, None)
}

/// Check completed work without reapplying literal pre-edit FIND conditions.
/// The audit appends its shared declared-file findings after checking receipts.
pub(crate) fn run_postflight(
    spec: &ParsedSpec,
    store: Option<&Store>,
    repo_root: &Path,
    removals: Option<&spec_removals::Plan>,
) -> Report {
    run_internal(spec, store, repo_root, Phase::Postflight, removals)
}

enum Phase {
    Preflight,
    Postflight,
}

fn run_internal(
    spec: &ParsedSpec,
    store: Option<&Store>,
    repo_root: &Path,
    phase: Phase,
    removals: Option<&spec_removals::Plan>,
) -> Report {
    let deadline = Instant::now() + crate::diff::git_timeout();
    let mut errors: Vec<Finding> = Vec::new();
    let mut warnings: Vec<Finding> = Vec::new();

    // 1. Mandatory sections non-empty.
    let spec_mode = spec.frontmatter.as_ref().and_then(|f| f.mode.as_deref());
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

    // 3. Pre-edit snapshot symbols: existence, caller-count drift, blast radius.
    //    Two sources, both contribute findings:
    //    a) heuristic `## Pre-edit symbol snapshot` bullets with lexical qualification
    //    b) frontmatter `touches[].symbols` with file+language scoping —
    //       constrains matching snapshots to the declared file and language.
    if let Some(store) = store {
        for claim in &spec.pre_edit_snapshot {
            check_symbol_claim(claim, spec, store, removals, &mut errors, &mut warnings);
        }
        if let Some(fm) = &spec.frontmatter {
            for touch in &fm.touches {
                check_frontmatter_touch(touch, store, removals, &mut errors, &mut warnings);
            }
        }
    }

    // 5. VERIFY commands — first token resolvable on `$PATH`. Soft warn: might be
    //    a project-local script (`./scripts/check.sh`) that looks unresolved but
    //    is fine. Covers heuristic phase-block `**VERIFY**: ...` lines AND
    //    frontmatter `verify[]` `cmd:` entries (label-only entries skipped).
    for cmd in spec.declared_verify_commands() {
        check_verify_command(cmd, &mut warnings);
    }

    // FIND describes pre-edit contents. Successful replacement may remove it.
    if matches!(phase, Phase::Preflight) {
        let interrupted = || store.is_some_and(Store::work_interrupted);
        let control = crate::bounded_fs::ReadControl {
            deadline: Some(deadline),
            interrupted: Some(&interrupted),
        };
        errors.extend(
            crate::declared_files::check(spec, repo_root, control, |_| false)
                .iter()
                .map(declared_file_finding),
        );
        errors.extend(crate::find_checks::check(
            &spec.find_blocks,
            repo_root,
            control,
        ));
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

pub(crate) fn declared_file_finding(issue: &crate::declared_files::Issue) -> Finding {
    if let (Some(file), "target_missing") = (&issue.file, issue.reason) {
        Finding::MissingFile { file: file.clone() }
    } else {
        Finding::DeclaredFileUnavailable {
            file: issue.file.clone(),
            reason: issue.reason.into(),
        }
    }
}

fn check_symbol_claim(
    claim: &SymbolClaim,
    spec: &ParsedSpec,
    store: &Store,
    removals: Option<&spec_removals::Plan>,
    errors: &mut Vec<Finding>,
    warnings: &mut Vec<Finding>,
) {
    if skip_removed_claim(
        &claim.name,
        &spec_symbols::snapshot_scopes(spec, claim),
        claim.signature.as_deref(),
        removals,
        errors,
    ) {
        return;
    }
    match spec_symbols::resolve_snapshot(store, spec, claim) {
        Ok(resolved) => check_resolved_claim(
            &claim.name,
            claim.callers,
            claim.signature.as_deref(),
            resolved,
            store,
            errors,
            warnings,
        ),
        Err(error) if error.reason == "missing" => errors.push(Finding::MissingSymbol {
            symbol: claim.name.clone(),
            section: "Pre-edit symbol snapshot".to_string(),
            raw: claim.raw.clone(),
        }),
        Err(error) => errors.push(unresolved_finding(&claim.name, error)),
    }
}

fn unresolved_finding(name: &str, error: Unresolved) -> Finding {
    Finding::SnapshotUnresolved {
        symbol: name.to_string(),
        reason: error.reason.to_string(),
        matches: error.matches,
    }
}

fn skip_removed_claim(
    name: &str,
    scopes: &[Scope<'_>],
    signature: Option<&str>,
    removals: Option<&spec_removals::Plan>,
    errors: &mut Vec<Finding>,
) -> bool {
    let Some(removals) = removals else {
        return false;
    };
    match removals.accepts_snapshot(name, scopes, signature) {
        Ok(accepted) => accepted,
        Err(error) => {
            errors.push(Finding::SnapshotUnresolved {
                symbol: name.to_string(),
                reason: format!("baseline_{}", error.reason),
                matches: error.matches,
            });
            true
        }
    }
}

fn check_resolved_claim(
    name: &str,
    callers: Option<u32>,
    signature: Option<&str>,
    resolved: Resolved,
    store: &Store,
    errors: &mut Vec<Finding>,
    warnings: &mut Vec<Finding>,
) {
    if let Some(declared) = signature {
        if resolved.symbol.signature.as_deref() != Some(declared) {
            errors.push(Finding::SnapshotSignatureDrift {
                symbol: name.to_string(),
                spec_says: declared.to_string(),
                index_says: resolved.symbol.signature.clone(),
            });
        }
    }
    // Counts retain the graph's name/type-candidate scope. Qualification
    // selects the declaration for signature checks, not definition-bound edges.
    let live_callers =
        match store.callers_of(&resolved.symbol.name, resolved.language.as_deref(), None) {
            Ok(rows) => rows.len() as u32,
            Err(_) => {
                errors.push(Finding::SnapshotUnresolved {
                    symbol: name.to_string(),
                    reason: "caller_query_failed".to_string(),
                    matches: None,
                });
                return;
            }
        };
    if let Some(declared) = callers {
        if declared != live_callers {
            errors.push(Finding::SnapshotCallerCountDrift {
                symbol: name.to_string(),
                spec_says: declared,
                index_says: live_callers,
            });
        }
    }
    if live_callers >= BLAST_RADIUS_WARN {
        warnings.push(Finding::LargeBlastRadius {
            symbol: name.to_string(),
            callers: live_callers,
            threshold: BLAST_RADIUS_WARN,
        });
    }
}

fn check_frontmatter_touch(
    touch: &TouchEntry,
    store: &Store,
    removals: Option<&spec_removals::Plan>,
    errors: &mut Vec<Finding>,
    warnings: &mut Vec<Finding>,
) {
    for sym in &touch.symbols {
        let name = sym.name();
        let file = sym.file().unwrap_or(&touch.file);
        let language = sym.language().or(touch.language.as_deref());
        let scope = Scope {
            name,
            file: Some(file),
            language,
        };
        if skip_removed_claim(name, &[scope], sym.signature(), removals, errors) {
            continue;
        }
        match spec_symbols::resolve(store, name, &[scope]) {
            Ok(resolved) => check_resolved_claim(
                name,
                sym.callers(),
                sym.signature(),
                resolved,
                store,
                errors,
                warnings,
            ),
            Err(error) if error.reason == "missing" => errors.push(Finding::MissingSymbolAtFile {
                symbol: name.to_string(),
                file: file.to_string(),
                language: language.map(str::to_string),
            }),
            Err(error) => errors.push(unresolved_finding(name, error)),
        }
    }
}

/// Check that a VERIFY command's first token resolves on `$PATH`. Skips
/// shell-syntactic intros (`cd …`, `pushd …`) and project-local `./` or `/`
/// paths — both common and not actually missing.
fn check_verify_command(command: &str, warnings: &mut Vec<Finding>) {
    let first = match command.split_whitespace().next() {
        Some(t) => t,
        None => return,
    };
    // Skip project-local paths (`./scripts/foo.sh`) — files, not PATH-resolved
    // binaries. Existence is the executor's problem, not ours.
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
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
        // Windows: try common executable extensions when none is present.
        if cfg!(windows) && !binary.contains('.') {
            for ext in ["exe", "cmd", "bat"] {
                let c = dir.join(format!("{binary}.{ext}"));
                if is_executable_file(&c) {
                    return Some(c);
                }
            }
        }
    }
    None
}

/// A regular file that is also runnable. On Unix a `$PATH` name without the
/// execute bit isn't a resolvable command, so existence is the wrong check; on
/// Windows runnability isn't in the mode, so existence is the best signal.
fn is_executable_file(p: &Path) -> bool {
    if !p.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Body is effectively empty when it's only whitespace, template placeholder
/// bullets (`- <thing>` with angle-bracket hints), or HTML comments.
fn body_is_effectively_empty(body: &str) -> bool {
    let stripped: String = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| !l.starts_with("<!--"))
        // Drop unchanged template placeholders: lines with balanced angle-bracket
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
        // See `doctor.rs::tmp()` — same parallel-collision fix. Atomic counter
        // gives a distinct dir per call; `process::id()` + nanos remain for
        // cross-process uniqueness and debuggability.
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
        // Spec mentions a nonexistent file + valid mandatory sections.
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
        // Monorepo collision: same leaf name `handleWebhook` in two files.
        fs::create_dir_all(root.join("src/billing")).unwrap();
        fs::create_dir_all(root.join("src/legacy")).unwrap();
        // The "right" file does NOT contain handleWebhook (yet) — spec is wrong
        // about where it lives; legacy has it. Heuristic check passes (exists
        // somewhere); scoped check should fail.
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

        // Frontmatter declares handleWebhook in billing.ts. Mandatory sections
        // all present to isolate the frontmatter check.
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
        // Heuristic would PASS (exists somewhere); scoped check must fail.
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
        // Label-only `typecheck` should NOT warn (no cmd); the bogus `cmd:` entry SHOULD.
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
        // Regression: prose like ``do not touch `README.md` `` used to flag
        // README.md as a claimed file (heuristic union path). With frontmatter
        // file scope, heuristic mentioned_files is ignored — frontmatter wins.
        let root = tmp();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/x.rs"), "fn x(){}").unwrap();
        // No README.md on disk — prose mention would flag MissingFile under the
        // old union behavior.
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
        // Heuristic would flag README.md and docs/guide.md missing. Frontmatter
        // authoritative → only src/x.rs in scope (and exists).
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
        // Create the mentioned file so MissingFile doesn't trigger.
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

    #[test]
    fn verified_mode_requires_compact_contract_not_strict_ceremony() {
        let root = tmp();
        let body = "\
---
id: \"1\"
mode: verified
---

## Goals
- Observable outcome
## Scope
- Bounded change
## Acceptance Criteria
- [ ] Behavior is observable
## Tests Plan
- focused test
## Final Verification
- repository gate
";
        let s = spec::parse_str("t.md", body);
        let r = run(&s, None, &root);
        assert!(
            !r.has_failures(),
            "verified compact contract should pass without strict sections: {:?}",
            r.errors
        );
        for strict_only in [
            "Alternatives Considered",
            "Documentation Plan",
            "Observability Plan",
            "Performance Considerations",
        ] {
            assert!(!r.errors.iter().any(|error| matches!(
                error,
                Finding::EmptyMandatorySection { section } if section == strict_only
            )));
        }
        fs::remove_dir_all(&root).ok();
    }
}
