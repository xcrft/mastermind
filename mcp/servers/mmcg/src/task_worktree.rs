//! Initial task inputs distinguish prior local work from the executor's changes.

use crate::bounded_fs::{self, BoundedReadError, ReadControl, RootCapability};
use crate::diff::{self, WorkingTreeChangedFile};
use crate::onboarding;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

const MAX_ENTRIES: usize = 1_000;
const MANIFEST_LIMIT: usize = 512 * 1024;
const FILE_LIMIT: u64 = crate::indexer::MAX_INDEXABLE_FILE_SIZE;
/// Bytes of regular files an initial worktree may hold until the project sets
/// its own budget.
pub(crate) const DEFAULT_WORKTREE_MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
/// Version 2 inventories include edits behind assume-unchanged and
/// skip-worktree. A version 1 manifest left those out, so comparing it with a
/// current inventory would charge them to the executor. It is still read.
const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialWorktree {
    pub schema_version: u32,
    pub baseline_oid: String,
    pub entries: Vec<InitialPath>,
    /// Regular-file bytes the entries may total, fixed when the manifest is
    /// captured. A manifest captured before project budgets existed has none
    /// and is read against `onboarding::MAX_TOTAL_BYTES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_bytes: Option<u64>,
}

/// A total byte allowance over a group of files, charged file by file. One
/// type charges every budget and words every exhausted-budget error, so each
/// names its setting, its numbers and the `init` flag that raises it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ByteBudget {
    /// The `setup.json` key that holds the limit.
    setting: &'static str,
    /// The `init` flag that raises it.
    flag: &'static str,
    /// How a raised value reaches the task, ending the error message.
    takes_effect: &'static str,
    /// Bytes the group may total.
    limit: u64,
    /// Bytes charged so far; never above `limit`.
    counted: u64,
}

impl ByteBudget {
    /// The initial worktree manifest's total of regular-file bytes.
    pub(crate) fn worktree(limit: u64) -> Self {
        Self {
            setting: "worktree_max_total_bytes",
            flag: "--worktree-max-total-bytes",
            takes_effect: "before the task's first pre-flight",
            limit,
            counted: 0,
        }
    }

    /// The bytes the history and strict workflow snapshots may read.
    pub(crate) fn snapshot(limit: u64) -> Self {
        Self {
            setting: "snapshot_max_total_bytes",
            flag: "--snapshot-max-total-bytes",
            takes_effect: "and repeat `mastermind run-task <spec> --pre-only`",
            limit,
            counted: 0,
        }
    }

    /// The most one read may return: the per-file limit, or what is left.
    pub(crate) fn read_limit(&self, file_limit: u64) -> u64 {
        file_limit.min(self.limit.saturating_sub(self.counted))
    }

    /// Counts `bytes` of the next file, naming it by `path` when known. An
    /// `Err` leaves the count unchanged.
    pub(crate) fn charge(&mut self, bytes: u64, path: Option<&str>) -> Result<(), String> {
        match self.counted.checked_add(bytes) {
            Some(total) if total <= self.limit => {
                self.counted = total;
                Ok(())
            }
            _ => Err(self.exhausted(bytes, path)),
        }
    }

    /// Words a failed read: a file over its own limit keeps the per-file
    /// wording, a smaller one that did not fit reports the budget.
    pub(crate) fn describe(&self, error: &BoundedReadError, file_limit: u64) -> String {
        match error {
            BoundedReadError::TooLarge { size, .. } if *size > file_limit => {
                format!("file has {size} bytes, limit is {file_limit}")
            }
            BoundedReadError::TooLarge { size, .. } => self.exhausted(*size, None),
            other => other.to_string(),
        }
    }

    fn exhausted(&self, size: u64, path: Option<&str>) -> String {
        let next = match path {
            Some(path) => format!("next file `{}`", crate::terminal::escape(path)),
            None => "this file".into(),
        };
        format!(
            "{} {} is exhausted: {} bytes counted, {next} has {size} bytes; raise it with \
             `mastermind init {} <bytes>` {}",
            self.setting, self.limit, self.counted, self.flag, self.takes_effect
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialPath {
    pub path: String,
    pub state: PathState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PathState {
    Missing,
    Regular {
        sha256: String,
        bytes: u64,
        executable: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnershipEvidence {
    pub basis: &'static str,
    pub initial_revision: String,
    pub unchanged_prior_files: Vec<String>,
}

#[derive(PartialEq, Eq)]
pub(crate) struct Comparison {
    pub raw_files: Vec<WorkingTreeChangedFile>,
    pub files: Vec<WorkingTreeChangedFile>,
    pub preimages: BTreeMap<String, Option<Vec<u8>>>,
    pub evidence: OwnershipEvidence,
    current_paths: BTreeMap<String, PathState>,
}

impl InitialWorktree {
    pub(crate) fn validate(&self, baseline: &str) -> Result<(), String> {
        if !(1..=SCHEMA_VERSION).contains(&self.schema_version)
            || self.baseline_oid != baseline
            || !diff::is_full_git_oid(baseline)
            || self.entries.len() > MAX_ENTRIES
            || self
                .max_total_bytes
                .is_some_and(|max| !onboarding::total_bytes_in_range(max))
            || serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > MANIFEST_LIMIT
        {
            return Err("initial_worktree_invalid".into());
        }
        let mut previous: Option<&str> = None;
        let mut budget =
            ByteBudget::worktree(self.max_total_bytes.unwrap_or(onboarding::MAX_TOTAL_BYTES));
        for entry in &self.entries {
            let path = bounded_fs::normalize_repository_relative_path(Path::new(&entry.path))
                .map_err(|_| "initial_worktree_path_invalid")?;
            if path != entry.path
                || path.starts_with(".mastermind/")
                || previous.is_some_and(|previous| previous >= path.as_str())
            {
                return Err("initial_worktree_path_invalid".into());
            }
            if let PathState::Regular { sha256, bytes, .. } = &entry.state {
                if sha256.len() != 64
                    || !sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    || *bytes > FILE_LIMIT
                {
                    return Err("initial_worktree_file_invalid".into());
                }
                if let Err(message) = budget.charge(*bytes, Some(&entry.path)) {
                    return Err(format!("initial_worktree_byte_limit: {message}"));
                }
            }
            previous = Some(&entry.path);
        }
        Ok(())
    }

    pub(crate) fn revision(&self) -> Result<String, String> {
        Ok(crate::hex::sha256(
            serde_json::to_vec(self).map_err(|e| e.to_string())?,
        ))
    }

    pub(crate) fn preimages(
        &self,
        root: &Path,
        spec: &Path,
    ) -> Result<BTreeMap<String, Option<Vec<u8>>>, String> {
        self.validate(&self.baseline_oid)?;
        self.entries
            .iter()
            .map(|entry| {
                let bytes = match &entry.state {
                    PathState::Missing => None,
                    PathState::Regular { sha256, bytes, .. } => {
                        let file = bounded_fs::read_regular_file(
                            root,
                            &blob_path(root, spec, sha256),
                            FILE_LIMIT,
                            FILE_LIMIT,
                            ReadControl::default(),
                        )
                        .map_err(|_| "initial_worktree_preimage_unavailable")?;
                        if file.declared_len != *bytes || crate::hex::sha256(&file.bytes) != *sha256
                        {
                            return Err("initial_worktree_preimage_changed".into());
                        }
                        Some(file.bytes)
                    }
                };
                Ok((entry.path.clone(), bytes))
            })
            .collect()
    }
}

fn blob_path(root: &Path, spec: &Path, hash: &str) -> PathBuf {
    crate::run_task::state_file_path(root, spec)
        .with_file_name("preflight-inputs")
        .join(hash)
}

fn artifacts(root: &Path, spec: &Path) -> Result<BTreeSet<String>, String> {
    let capability = RootCapability::open(root).map_err(|e| e.to_string())?;
    [
        spec.to_path_buf(),
        crate::run_task::executor_report_path(spec),
        crate::run_task::audit_file_path(spec),
        crate::run_task::release_file_path(root, spec),
        crate::run_task::history_review_file_path(root, spec),
        crate::run_task::state_file_path(root, spec),
    ]
    .into_iter()
    .map(|path| {
        let path = capability
            .repository_relative(&path)
            .map_err(|_| "initial_worktree_artifact_path_invalid")?;
        bounded_fs::normalize_repository_relative_path(&path)
            .map_err(|_| "initial_worktree_artifact_path_invalid".into())
    })
    .collect()
}

fn state(
    root: &RootCapability,
    path: &str,
    deadline: Instant,
) -> Result<(PathState, Option<Vec<u8>>), String> {
    match bounded_fs::read_regular_file_with_capability(
        root,
        &root.requested_root().join(path),
        FILE_LIMIT,
        FILE_LIMIT,
        ReadControl {
            deadline: Some(deadline),
            interrupted: None,
        },
    ) {
        Ok(file) => {
            let state = PathState::Regular {
                sha256: crate::hex::sha256(&file.bytes),
                bytes: file.declared_len,
                executable: file.identity.attributes() & 0o111 != 0,
            };
            Ok((state, Some(file.bytes)))
        }
        Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok((PathState::Missing, None))
        }
        Err(error @ BoundedReadError::TooLarge { .. }) => Err(format!(
            "initial_worktree_file_unavailable: {}: {error}. The task keeps a copy of \
             every uncommitted file, so add this one to .gitignore, commit it or move it \
             out of the worktree",
            crate::terminal::escape(path)
        )),
        Err(error) => Err(format!(
            "initial_worktree_file_unavailable: {}: {error}",
            crate::terminal::escape(path)
        )),
    }
}

fn inventory(
    root: &Path,
    baseline: &str,
    deadline: Instant,
) -> Result<Vec<WorkingTreeChangedFile>, String> {
    diff::complete_worktree_paths_controlled(
        root,
        baseline,
        Some(".mastermind"),
        Some(deadline),
        None,
    )
    .map_err(|error| format!("initial_worktree_inventory: {}", error.code()))?
    .ok_or_else(|| "initial_worktree_inventory_incomplete".into())
}

/// Records the files already changed or untracked at `baseline`, refusing a
/// worktree whose regular files total more than `max_total_bytes`. The manifest
/// keeps that budget for the life of the task.
pub(crate) fn capture(
    root: &Path,
    spec: &Path,
    baseline: &str,
    max_total_bytes: u64,
) -> Result<InitialWorktree, String> {
    let deadline = Instant::now() + diff::git_timeout();
    let capability = RootCapability::open(root).map_err(|e| e.to_string())?;
    let artifacts = artifacts(root, spec)?;
    let files = inventory(root, baseline, deadline)?
        .into_iter()
        .filter(|file| !artifacts.contains(&file.path))
        .collect::<Vec<_>>();
    if files.len() > MAX_ENTRIES {
        return Err("initial_worktree_file_limit".into());
    }
    let mut initial = InitialWorktree {
        schema_version: SCHEMA_VERSION,
        baseline_oid: baseline.into(),
        entries: Vec::new(),
        max_total_bytes: Some(max_total_bytes),
    };
    for file in &files {
        let (state, bytes) = state(&capability, &file.path, deadline)?;
        initial.entries.push(InitialPath {
            path: file.path.clone(),
            state,
        });
        initial.validate(baseline)?;
        if let Some(bytes) = bytes {
            let path = blob_path(root, spec, &crate::hex::sha256(&bytes));
            match bounded_fs::read_regular_file(
                root,
                &path,
                FILE_LIMIT,
                FILE_LIMIT,
                ReadControl::default(),
            ) {
                Ok(existing) if existing.bytes == bytes => {}
                Ok(_) => return Err("initial_worktree_preimage_changed".into()),
                Err(BoundedReadError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    bounded_fs::write_atomic_regular_file(root, &path, &bytes, true)
                        .map_err(|_| "initial_worktree_preimage_write_failed")?;
                }
                Err(_) => return Err("initial_worktree_preimage_unavailable".into()),
            }
        }
    }
    if inventory(root, baseline, deadline)?
        .into_iter()
        .filter(|file| !artifacts.contains(&file.path))
        .collect::<Vec<_>>()
        != files
    {
        return Err("initial_worktree_changed_during_capture".into());
    }
    for entry in &initial.entries {
        if state(&capability, &entry.path, deadline)?.0 != entry.state {
            return Err("initial_worktree_changed_during_capture".into());
        }
    }
    capability
        .verify()
        .map_err(|_| "initial_worktree_root_changed")?;
    Ok(initial)
}

pub(crate) fn compare(
    root: &Path,
    spec: &Path,
    initial: &InitialWorktree,
    deadline: Instant,
) -> Result<Comparison, String> {
    initial.validate(&initial.baseline_oid)?;
    // An older manifest stays readable for status and history, but cannot be
    // compared with an inventory that sees more.
    if initial.schema_version < SCHEMA_VERSION {
        return Err("initial_worktree_schema_outdated: start a new task".into());
    }
    let capability = RootCapability::open(root).map_err(|e| e.to_string())?;
    let raw_files = inventory(root, &initial.baseline_oid, deadline)?;
    let mut selected = raw_files
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect::<BTreeMap<_, _>>();
    for artifact in artifacts(root, spec)? {
        selected.remove(&artifact);
    }
    let preimages = initial.preimages(root, spec)?;
    let mut unchanged_prior_files = Vec::new();
    let mut current_paths = BTreeMap::new();
    for entry in &initial.entries {
        let current = state(&capability, &entry.path, deadline)?.0;
        current_paths.insert(entry.path.clone(), current.clone());
        if current == entry.state {
            selected.remove(&entry.path);
            unchanged_prior_files.push(entry.path.clone());
        } else {
            selected
                .entry(entry.path.clone())
                .or_insert_with(|| WorkingTreeChangedFile {
                    path: entry.path.clone(),
                    status: if current == PathState::Missing {
                        "deleted"
                    } else {
                        "modified"
                    }
                    .into(),
                });
        }
    }
    for file in &raw_files {
        if let std::collections::btree_map::Entry::Vacant(entry) =
            current_paths.entry(file.path.clone())
        {
            entry.insert(state(&capability, &file.path, deadline)?.0);
        }
    }
    capability
        .verify()
        .map_err(|_| "initial_worktree_root_changed")?;
    Ok(Comparison {
        raw_files,
        files: selected.into_values().collect(),
        preimages,
        evidence: OwnershipEvidence {
            basis: "initial_worktree",
            initial_revision: initial.revision()?,
            unchanged_prior_files,
        },
        current_paths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A version 1 manifest missed edits behind index flags. It stays
    /// readable, but comparing it would charge those edits to the executor.
    #[test]
    fn a_manifest_from_before_flagged_files_were_listed_is_read_but_not_compared() {
        let baseline = "0".repeat(40);
        let initial = InitialWorktree {
            schema_version: 1,
            baseline_oid: baseline.clone(),
            entries: Vec::new(),
            max_total_bytes: None,
        };
        assert_eq!(initial.validate(&baseline), Ok(()));
        let refused = compare(
            Path::new("."),
            Path::new("spec.md"),
            &initial,
            Instant::now() + diff::git_timeout(),
        )
        .err();
        assert_eq!(
            refused.as_deref(),
            Some("initial_worktree_schema_outdated: start a new task")
        );
    }

    const MIB: u64 = 1024 * 1024;

    fn manifest(max_total_bytes: Option<u64>, sizes: &[u64]) -> InitialWorktree {
        InitialWorktree {
            schema_version: SCHEMA_VERSION,
            baseline_oid: "0".repeat(40),
            entries: sizes
                .iter()
                .enumerate()
                .map(|(index, bytes)| InitialPath {
                    path: format!("file-{index:04}"),
                    state: PathState::Regular {
                        sha256: "0".repeat(64),
                        bytes: *bytes,
                        executable: false,
                    },
                })
                .collect(),
            max_total_bytes,
        }
    }

    fn validate(initial: &InitialWorktree) -> Result<(), String> {
        initial.validate(&"0".repeat(40))
    }

    #[test]
    fn a_manifest_holds_exactly_its_recorded_budget() {
        let mut sizes = vec![5 * MIB; 12];
        sizes.push(4 * MIB);
        assert_eq!(validate(&manifest(Some(64 * MIB), &sizes)), Ok(()));
        sizes.push(1);
        assert_eq!(
            validate(&manifest(Some(64 * MIB), &sizes)),
            Err(
                "initial_worktree_byte_limit: worktree_max_total_bytes 67108864 is exhausted: \
                 67108864 bytes counted, next file `file-0013` has 1 bytes; raise it with \
                 `mastermind init --worktree-max-total-bytes <bytes>` before the task's first \
                 pre-flight"
                    .into()
            )
        );
    }

    #[test]
    fn the_exhausted_budget_names_the_file_that_did_not_fit() {
        let sizes = vec![5 * MIB; 7];
        assert_eq!(
            validate(&manifest(Some(32 * MIB), &sizes)),
            Err(
                "initial_worktree_byte_limit: worktree_max_total_bytes 33554432 is exhausted: \
                 31457280 bytes counted, next file `file-0006` has 5242880 bytes; raise it with \
                 `mastermind init --worktree-max-total-bytes <bytes>` before the task's first \
                 pre-flight"
                    .into()
            )
        );
    }

    #[test]
    fn a_manifest_without_a_recorded_budget_is_read_against_one_gibibyte() {
        assert_eq!(validate(&manifest(None, &[5 * MIB; 7])), Ok(()));
        let mut sizes = vec![5 * MIB; 204];
        sizes.push(4 * MIB);
        assert_eq!(validate(&manifest(None, &sizes)), Ok(()));
        sizes.push(1);
        let error = validate(&manifest(None, &sizes)).unwrap_err();
        assert!(
            error.starts_with(
                "initial_worktree_byte_limit: worktree_max_total_bytes 1073741824 is exhausted: "
            ),
            "{error}"
        );
    }

    #[test]
    fn a_recorded_budget_outside_the_shared_range_is_an_invalid_manifest() {
        for max in [0, 1_073_741_825] {
            assert_eq!(
                validate(&manifest(Some(max), &[])),
                Err("initial_worktree_invalid".into()),
                "{max}"
            );
        }
        assert_eq!(validate(&manifest(Some(1), &[1])), Ok(()));
        assert_eq!(validate(&manifest(Some(1_073_741_824), &[])), Ok(()));
    }

    #[test]
    fn a_file_over_the_per_file_limit_is_still_an_invalid_file_under_a_large_budget() {
        assert_eq!(
            validate(&manifest(Some(64 * MIB), &[5 * MIB + 1])),
            Err("initial_worktree_file_invalid".into())
        );
    }

    #[test]
    fn a_manifest_without_a_recorded_budget_keeps_its_revision() {
        let legacy = format!(
            r#"{{"schema_version":2,"baseline_oid":"{}","entries":[]}}"#,
            "0".repeat(40)
        );
        let initial: InitialWorktree = serde_json::from_str(&legacy).unwrap();
        assert_eq!(initial.max_total_bytes, None);
        assert_eq!(serde_json::to_string(&initial).unwrap(), legacy);
        assert_eq!(
            initial.revision().unwrap(),
            crate::hex::sha256(legacy.as_bytes())
        );
    }

    #[test]
    fn a_budget_counts_up_to_its_limit_and_reports_the_next_read_that_does_not_fit() {
        let mut budget = ByteBudget::snapshot(10);
        assert_eq!(budget.read_limit(100), 10);
        assert_eq!(budget.charge(4, None), Ok(()));
        assert_eq!(budget.read_limit(100), 6);
        assert_eq!(budget.read_limit(3), 3);
        assert_eq!(budget.charge(6, None), Ok(()));
        assert_eq!(budget.read_limit(100), 0);
        assert_eq!(
            budget.charge(1, None),
            Err(
                "snapshot_max_total_bytes 10 is exhausted: 10 bytes counted, this file has 1 \
                 bytes; raise it with `mastermind init --snapshot-max-total-bytes <bytes>` and \
                 repeat `mastermind run-task <spec> --pre-only`"
                    .into()
            )
        );
    }

    #[test]
    fn file_names_from_git_reach_the_message_with_terminal_controls_escaped() {
        let name = "big\u{1b}[2J.bin";
        let error = ByteBudget::snapshot(0).charge(1, Some(name)).unwrap_err();
        assert!(error.contains("next file `big\\u{1b}[2J.bin`"), "{error}");

        // Windows refuses control characters in file names.
        if cfg!(windows) {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        std::fs::File::create(directory.path().join(name))
            .unwrap()
            .set_len(FILE_LIMIT + 1)
            .unwrap();
        let root = RootCapability::open(directory.path()).unwrap();
        let error = state(&root, name, Instant::now() + diff::git_timeout()).unwrap_err();
        assert!(
            error.starts_with("initial_worktree_file_unavailable: big\\u{1b}[2J.bin: "),
            "{error}"
        );
    }

    #[test]
    fn a_charge_that_would_overflow_the_count_is_refused() {
        let mut budget = ByteBudget::snapshot(u64::MAX);
        assert_eq!(budget.charge(u64::MAX, None), Ok(()));
        assert!(budget.charge(1, None).is_err());
    }

    #[test]
    fn a_read_over_its_file_limit_keeps_the_per_file_wording() {
        let budget = ByteBudget::snapshot(10);
        let too_large = |size| BoundedReadError::TooLarge { size, limit: 10 };
        assert_eq!(
            budget.describe(&too_large(101), 100),
            "file has 101 bytes, limit is 100"
        );
        assert!(budget.describe(&too_large(11), 100).starts_with(
            "snapshot_max_total_bytes 10 is exhausted: 0 bytes counted, this file has 11 bytes"
        ));
    }
}
