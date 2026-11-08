//! Retained successful preconditions allow explicit reapproval after implementation.

use crate::bounded_fs::{self, BoundedReadError, ReadControl};
use crate::run_task::RunState;
use crate::spec::{self, ParsedSpec, SymbolSpec, TouchEntry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const RECEIPT_LIMIT: u64 = 64 * 1024;
const SPEC_LIMIT: u64 = crate::audit_bundle::BUNDLE_INPUT_MAX as u64;
const CHAIN_LIMIT: usize = 128;
const TOTAL_LIMIT: u64 = 32 * 1024 * 1024;

/// Version 2 approvals were checked by the spec parser that records lines it
/// cannot read. A version 1 approval stays valid for its own audit, but cannot
/// carry preconditions into a reapproval, because the older parser could have
/// dropped some of them and the edited files can no longer prove them.
const SCHEMA_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    schema_version: u32,
    repository_identity: String,
    spec_path: String,
    spec_sha256: String,
    baseline_oid: String,
    initial_worktree_revision: Option<String>,
    iteration: u32,
    preflight_started_at: u64,
    indexed_preconditions_covered: bool,
    basis: String,
    parent: Option<String>,
}

pub(crate) struct Validation {
    pub current: ParsedSpec,
    pub carried: usize,
    basis: &'static str,
    parent: Option<String>,
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn directory(root: &Path, spec: &Path) -> PathBuf {
    crate::run_task::state_file_path(root, spec).with_file_name("preflight-approvals")
}

fn initial_revision(state: &RunState) -> Result<Option<String>, String> {
    state
        .initial_worktree
        .as_ref()
        .map(|initial| initial.revision())
        .transpose()
}

fn read(root: &Path, path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    bounded_fs::read_regular_file(root, path, limit, limit, ReadControl::default())
        .map(|file| file.bytes)
        .map_err(|error| match error {
            BoundedReadError::TooLarge { .. } => "preflight_approval_limit".into(),
            _ => "preflight_approval_unavailable".into(),
        })
}

/// The approved spec, whether its index checks ran, and whether the current
/// spec parser checked it.
fn load(
    root: &Path,
    spec: &Path,
    state: &RunState,
) -> Result<Option<(ParsedSpec, bool, bool)>, String> {
    let Some(pin) = state.preflight_approval_sha256.as_deref() else {
        return Ok(None);
    };
    let mut next = Some(pin.to_string());
    let mut active = None;
    let mut child_iteration = None;
    let mut remaining = TOTAL_LIMIT;
    let initial = initial_revision(state)?;
    for _ in 0..CHAIN_LIMIT {
        let Some(pin) = next.take() else {
            return Ok(active);
        };
        if !valid_hash(&pin) {
            return Err("preflight_approval_pin_invalid".into());
        }
        let bytes = read(
            root,
            &directory(root, spec).join(format!("{pin}.json")),
            RECEIPT_LIMIT.min(remaining),
        )?;
        remaining -= bytes.len() as u64;
        if crate::run_task::hash_text(
            std::str::from_utf8(&bytes).map_err(|_| "preflight_approval_invalid")?,
        ) != pin
        {
            return Err("preflight_approval_changed".into());
        }
        let record: Approval =
            serde_json::from_slice(&bytes).map_err(|_| "preflight_approval_invalid")?;
        if !(1..=SCHEMA_VERSION).contains(&record.schema_version)
            || state.repository_identity.as_deref() != Some(record.repository_identity.as_str())
            || record.spec_path != state.spec_path
            || !valid_hash(&record.spec_sha256)
            || record.baseline_oid != state.baseline_ref
            || !crate::diff::is_full_git_oid(&record.baseline_oid)
            || record.initial_worktree_revision != initial
            || record.iteration == 0
            || record.iteration > state.iteration
            || child_iteration.is_some_and(|child| record.iteration >= child)
            || !matches!(
                record.basis.as_str(),
                "current_preflight" | "reapproval" | "legacy_prior_approval"
            )
            || (record.basis == "reapproval") != record.parent.is_some()
        {
            return Err("preflight_approval_binding_mismatch".into());
        }
        if active.is_none()
            && (record.spec_sha256 != state.spec_hash
                || record.preflight_started_at != state.started_at)
        {
            return Err("preflight_approval_binding_mismatch".into());
        }
        let body = read(
            root,
            &directory(root, spec).join(format!("{}.md", record.spec_sha256)),
            SPEC_LIMIT.min(remaining),
        )?;
        remaining -= body.len() as u64;
        let body = String::from_utf8(body).map_err(|_| "preflight_approval_spec_invalid")?;
        if crate::run_task::hash_text(&body) != record.spec_sha256 {
            return Err("preflight_approval_spec_changed".into());
        }
        let parsed = spec::parse_str(&record.spec_path, &body);
        // The older parser read a misplaced block as a spec without
        // frontmatter, so its approval of that body still stands.
        let legacy = record.schema_version < SCHEMA_VERSION
            && parsed.frontmatter_error.as_deref() == Some("frontmatter_not_at_start");
        if parsed.frontmatter_error.is_some() && !legacy {
            return Err("preflight_approval_spec_invalid".into());
        }
        if active.is_none() {
            active = Some((
                parsed,
                record.indexed_preconditions_covered,
                record.schema_version == SCHEMA_VERSION,
            ));
        }
        child_iteration = Some(record.iteration);
        next = record.parent;
    }
    if next.is_none() {
        Ok(active)
    } else {
        Err("preflight_approval_chain_limit".into())
    }
}

pub(crate) fn validate(root: &Path, spec: &Path, state: &RunState) -> Result<(), String> {
    load(root, spec, state).map(|_| ())
}

fn encoded(value: impl Serialize) -> String {
    serde_json::to_string(&value).expect("serializable historical precondition")
}

fn snapshot_key(spec: &ParsedSpec, claim: &spec::SymbolClaim) -> String {
    let mut scopes = crate::spec_symbols::snapshot_scopes(spec, claim)
        .into_iter()
        .map(|scope| (scope.name, scope.file, scope.language))
        .collect::<Vec<_>>();
    scopes.sort_unstable();
    encoded((
        &claim.name,
        claim.signature.as_deref(),
        claim.callers,
        scopes,
    ))
}

fn touch_key(touch: &TouchEntry, symbol: &SymbolSpec) -> String {
    encoded((
        symbol.name(),
        symbol.file().unwrap_or(&touch.file),
        symbol.language().or(touch.language.as_deref()),
        symbol.signature(),
        symbol.callers(),
    ))
}

/// Only an exact previously approved condition is historical. Revised conditions
/// retain normal live validation, including effective file/language scope.
pub(crate) fn prepare(
    root: &Path,
    spec_path: &Path,
    current: &ParsedSpec,
    body: &str,
    previous: Option<&RunState>,
    explicit_pre_only: bool,
) -> Result<Validation, String> {
    let mut validation = Validation {
        current: current.clone(),
        carried: 0,
        basis: "current_preflight",
        parent: None,
    };
    let Some(previous) = previous else {
        return Ok(validation);
    };
    let historical = match load(root, spec_path, previous)? {
        Some(_) if crate::run_task::spec_hash_matches(&previous.spec_hash, body) => {
            // An unchanged contract requests fresh validation, not amendment recovery.
            return Ok(validation);
        }
        Some((_, _, false)) => {
            return Err("preflight_approval_schema_outdated: start a new task".into());
        }
        Some(approved) => {
            validation.parent = previous.preflight_approval_sha256.clone();
            validation.basis = "reapproval";
            Some(approved)
        }
        None if explicit_pre_only
            && previous.repository_identity.is_some()
            && previous.iteration > 0
            && previous.started_at > 0
            && crate::diff::is_full_git_oid(&previous.baseline_ref)
            && crate::run_task::spec_hash_matches(&previous.spec_hash, body) =>
        {
            // A failed retry may have advanced the counter. This preserves
            // prior contract approval, not an invented original attempt.
            validation.basis = "legacy_prior_approval";
            Some((current.clone(), !previous.allow_no_index, true))
        }
        None => None,
    };
    let Some((old, index_checked, _)) = historical else {
        return Ok(validation);
    };
    let find_keys = old
        .find_blocks
        .iter()
        .map(|block| encoded((&block.file, &block.find_text)))
        .collect::<BTreeSet<_>>();
    validation
        .current
        .find_blocks
        .retain(|block| !find_keys.contains(&encoded((&block.file, &block.find_text))));
    validation.carried = current.find_blocks.len() - validation.current.find_blocks.len();
    if index_checked {
        let snapshot_keys = old
            .pre_edit_snapshot
            .iter()
            .map(|claim| snapshot_key(&old, claim))
            .collect::<BTreeSet<_>>();
        validation
            .current
            .pre_edit_snapshot
            .retain(|claim| !snapshot_keys.contains(&snapshot_key(current, claim)));
        validation.carried +=
            current.pre_edit_snapshot.len() - validation.current.pre_edit_snapshot.len();
        let touch_keys = old
            .frontmatter
            .iter()
            .flat_map(|fm| &fm.touches)
            .flat_map(|touch| {
                touch
                    .symbols
                    .iter()
                    .map(move |symbol| touch_key(touch, symbol))
            })
            .collect::<BTreeSet<_>>();
        if let Some(frontmatter) = validation.current.frontmatter.as_mut() {
            for touch in &mut frontmatter.touches {
                let original = touch.symbols.len();
                let context = touch.clone();
                touch
                    .symbols
                    .retain(|symbol| !touch_keys.contains(&touch_key(&context, symbol)));
                validation.carried += original - touch.symbols.len();
            }
        }
    }
    Ok(validation)
}

fn write_immutable(root: &Path, path: &Path, bytes: &[u8], limit: u64) -> Result<(), String> {
    match bounded_fs::read_regular_file(root, path, limit, limit, ReadControl::default()) {
        Ok(existing) if existing.bytes == bytes => Ok(()),
        Ok(_) => Err("preflight_approval_changed".into()),
        Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            bounded_fs::write_atomic_regular_file(root, path, bytes, true)
                .map_err(|_| "preflight_approval_write_failed".into())
        }
        Err(_) => Err("preflight_approval_unavailable".into()),
    }
}

pub(crate) fn retain(
    root: &Path,
    spec: &Path,
    state: &RunState,
    body: &str,
    validation: &Validation,
    index_checked: bool,
) -> Result<String, String> {
    let record = Approval {
        schema_version: SCHEMA_VERSION,
        repository_identity: state
            .repository_identity
            .clone()
            .ok_or("preflight_approval_repository_missing")?,
        spec_path: state.spec_path.clone(),
        spec_sha256: crate::run_task::hash_text(body),
        baseline_oid: state.baseline_ref.clone(),
        initial_worktree_revision: initial_revision(state)?,
        iteration: state.iteration,
        preflight_started_at: state.started_at,
        indexed_preconditions_covered: index_checked,
        basis: validation.basis.into(),
        parent: validation.parent.clone(),
    };
    let bytes = serde_json::to_vec(&record).map_err(|_| "preflight_approval_invalid")?;
    if bytes.len() as u64 > RECEIPT_LIMIT || body.len() as u64 > SPEC_LIMIT {
        return Err("preflight_approval_limit".into());
    }
    let pin = crate::run_task::hash_text(std::str::from_utf8(&bytes).expect("JSON is UTF-8"));
    let directory = directory(root, spec);
    write_immutable(
        root,
        &directory.join(format!("{}.md", record.spec_sha256)),
        body.as_bytes(),
        SPEC_LIMIT,
    )?;
    write_immutable(
        root,
        &directory.join(format!("{pin}.json")),
        &bytes,
        RECEIPT_LIMIT,
    )?;
    Ok(pin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(body: &str) -> RunState {
        serde_json::from_value(serde_json::json!({
            "spec_path": ".mastermind/tasks/001-reapproval/spec.md",
            "repository_identity": format!("git-worktree:sha256:{}", "a".repeat(64)),
            "spec_hash": crate::run_task::hash_text(body),
            "baseline_ref": "b".repeat(40), "started_at": 123, "iteration": 1
        }))
        .unwrap()
    }

    const BODY: &str = "---\ntouches:\n  - file: src.py\n    language: python\n    symbols: [old_api]\n---\n# Contract\n## Pre-edit symbol snapshot\n- `old_api` — 2 callers\n## Phase 1\n**File:** `src.py`\nFIND:\n```python\ndef old_api():\n```\n";

    #[test]
    fn only_identical_effective_preconditions_are_carried() {
        let root = tempfile::tempdir().unwrap();
        let mut previous = state(BODY);
        let path = root.path().join(&previous.spec_path);
        let parsed = spec::parse_str(&previous.spec_path, BODY);
        let first = prepare(root.path(), &path, &parsed, BODY, None, true).unwrap();
        previous.preflight_approval_sha256 =
            Some(retain(root.path(), &path, &previous, BODY, &first, true).unwrap());
        let fresh = prepare(root.path(), &path, &parsed, BODY, Some(&previous), true).unwrap();
        assert_eq!(fresh.carried, 0);
        assert_eq!(fresh.current.find_blocks.len(), 1);
        assert_eq!(fresh.current.pre_edit_snapshot.len(), 1);
        assert_eq!(
            fresh.current.frontmatter.unwrap().touches[0].symbols.len(),
            1
        );
        let revised = BODY.replace("Phase 1", "Phase renamed") + "\nA clarification.\n";
        let parsed = spec::parse_str(&previous.spec_path, &revised);
        let carried =
            prepare(root.path(), &path, &parsed, &revised, Some(&previous), true).unwrap();
        assert_eq!(carried.carried, 3);
        assert!(carried.current.find_blocks.is_empty());
        assert!(carried.current.pre_edit_snapshot.is_empty());
        assert!(carried.current.frontmatter.unwrap().touches[0]
            .symbols
            .is_empty());

        let revised = BODY
            .replace("src.py", "other.py")
            .replace("2 callers", "3 callers");
        let parsed = spec::parse_str(&previous.spec_path, &revised);
        let changed =
            prepare(root.path(), &path, &parsed, &revised, Some(&previous), true).unwrap();
        assert_eq!(changed.carried, 0);
        assert_eq!(changed.current.pre_edit_snapshot.len(), 1);
        assert_eq!(changed.current.find_blocks.len(), 1);
    }

    #[test]
    fn an_approval_from_the_older_spec_parser_cannot_be_reapproved() {
        let root = tempfile::tempdir().unwrap();
        let mut previous = state(BODY);
        let path = root.path().join(&previous.spec_path);
        let parsed = spec::parse_str(&previous.spec_path, BODY);
        let first = prepare(root.path(), &path, &parsed, BODY, None, true).unwrap();
        let pin = retain(root.path(), &path, &previous, BODY, &first, true).unwrap();
        let directory = directory(root.path(), &path);
        let current = std::fs::read_to_string(directory.join(format!("{pin}.json"))).unwrap();
        let older = current.replace("\"schema_version\":2", "\"schema_version\":1");
        assert_ne!(older, current);
        let older_pin = crate::run_task::hash_text(&older);
        std::fs::write(directory.join(format!("{older_pin}.json")), older).unwrap();
        let revised = BODY.replace("Phase 1", "Phase renamed");
        let parsed = spec::parse_str(&previous.spec_path, &revised);

        previous.preflight_approval_sha256 = Some(pin);
        let carried =
            prepare(root.path(), &path, &parsed, &revised, Some(&previous), true).unwrap();
        assert_eq!(carried.carried, 3);

        previous.preflight_approval_sha256 = Some(older_pin);
        assert_eq!(
            prepare(root.path(), &path, &parsed, &revised, Some(&previous), true).err(),
            Some("preflight_approval_schema_outdated: start a new task".to_string())
        );
        validate(root.path(), &path, &previous).unwrap();
    }

    #[test]
    fn an_older_approval_of_a_misplaced_frontmatter_block_still_validates() {
        let body = format!("\u{feff}{BODY}");
        let root = tempfile::tempdir().unwrap();
        let mut previous = state(&body);
        let path = root.path().join(&previous.spec_path);
        let parsed = spec::parse_str(&previous.spec_path, &body);
        let first = prepare(root.path(), &path, &parsed, &body, None, true).unwrap();
        let pin = retain(root.path(), &path, &previous, &body, &first, true).unwrap();
        let directory = directory(root.path(), &path);
        let current = std::fs::read_to_string(directory.join(format!("{pin}.json"))).unwrap();
        let older = current.replace("\"schema_version\":2", "\"schema_version\":1");
        let older_pin = crate::run_task::hash_text(&older);
        std::fs::write(directory.join(format!("{older_pin}.json")), older).unwrap();

        previous.preflight_approval_sha256 = Some(pin);
        assert_eq!(
            validate(root.path(), &path, &previous).err(),
            Some("preflight_approval_spec_invalid".to_string())
        );
        previous.preflight_approval_sha256 = Some(older_pin);
        validate(root.path(), &path, &previous).unwrap();
    }

    #[test]
    fn corrupt_or_missing_pinned_approval_cannot_use_legacy_fallback() {
        for artifact in ["receipt", "spec", "missing", "binding"] {
            let root = tempfile::tempdir().unwrap();
            let mut previous = state(BODY);
            let path = root.path().join(&previous.spec_path);
            let parsed = spec::parse_str(&previous.spec_path, BODY);
            let first = prepare(root.path(), &path, &parsed, BODY, None, true).unwrap();
            let pin = retain(root.path(), &path, &previous, BODY, &first, true).unwrap();
            previous.preflight_approval_sha256 = Some(pin.clone());
            let receipt = directory(root.path(), &path).join(format!("{pin}.json"));
            match artifact {
                "receipt" => std::fs::write(receipt, "{}").unwrap(),
                "spec" => std::fs::write(
                    directory(root.path(), &path).join(format!("{}.md", previous.spec_hash)),
                    "changed",
                )
                .unwrap(),
                "missing" => std::fs::remove_file(receipt).unwrap(),
                "binding" => previous.baseline_ref = "c".repeat(40),
                _ => unreachable!(),
            }
            assert!(
                prepare(root.path(), &path, &parsed, BODY, Some(&previous), true).is_err(),
                "{artifact}"
            );
        }
    }

    #[test]
    fn legacy_skipped_index_checks_are_revalidated_live() {
        let root = tempfile::tempdir().unwrap();
        let mut previous = state(BODY);
        previous.allow_no_index = true;
        let path = root.path().join(&previous.spec_path);
        let parsed = spec::parse_str(&previous.spec_path, BODY);
        let validation = prepare(root.path(), &path, &parsed, BODY, Some(&previous), true).unwrap();
        assert_eq!(validation.carried, 1);
        assert_eq!(validation.current.pre_edit_snapshot.len(), 1);
        assert_eq!(
            validation.current.frontmatter.unwrap().touches[0]
                .symbols
                .len(),
            1
        );
        let no_explicit_approval =
            prepare(root.path(), &path, &parsed, BODY, Some(&previous), false).unwrap();
        assert_eq!(no_explicit_approval.carried, 0);
    }

    #[test]
    fn reapproval_requires_retained_ancestor_evidence() {
        for missing in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let mut previous = state(BODY);
            let path = root.path().join(&previous.spec_path);
            let parsed = spec::parse_str(&previous.spec_path, BODY);
            let first = prepare(root.path(), &path, &parsed, BODY, None, true).unwrap();
            let parent = retain(root.path(), &path, &previous, BODY, &first, true).unwrap();
            previous.preflight_approval_sha256 = Some(parent.clone());
            let revised = format!("{BODY}\nA clarification.\n");
            let parsed = spec::parse_str(&previous.spec_path, &revised);
            let validation =
                prepare(root.path(), &path, &parsed, &revised, Some(&previous), true).unwrap();
            previous.iteration = 2;
            previous.started_at = 124;
            previous.spec_hash = crate::run_task::hash_text(&revised);
            previous.preflight_approval_sha256 =
                Some(retain(root.path(), &path, &previous, &revised, &validation, true).unwrap());
            assert!(validate(root.path(), &path, &previous).is_ok());
            let ancestor = directory(root.path(), &path).join(format!("{parent}.json"));
            if missing {
                std::fs::remove_file(ancestor).unwrap();
            } else {
                std::fs::write(ancestor, "corrupt").unwrap();
            }
            assert!(validate(root.path(), &path, &previous).is_err());
        }
    }
}
