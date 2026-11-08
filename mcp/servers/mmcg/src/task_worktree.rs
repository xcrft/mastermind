//! Initial task inputs distinguish prior local work from the executor's changes.

use crate::bounded_fs::{self, BoundedReadError, ReadControl, RootCapability};
use crate::diff::{self, WorkingTreeChangedFile};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

const MAX_ENTRIES: usize = 1_000;
const MANIFEST_LIMIT: usize = 512 * 1024;
const FILE_LIMIT: u64 = crate::indexer::MAX_INDEXABLE_FILE_SIZE;
const TOTAL_LIMIT: u64 = 32 * 1024 * 1024;
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
            || serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > MANIFEST_LIMIT
        {
            return Err("initial_worktree_invalid".into());
        }
        let mut previous: Option<&str> = None;
        let mut total = 0u64;
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
                total = total
                    .checked_add(*bytes)
                    .ok_or("initial_worktree_byte_limit")?;
                if total > TOTAL_LIMIT {
                    return Err("initial_worktree_byte_limit".into());
                }
            }
            previous = Some(&entry.path);
        }
        Ok(())
    }

    pub(crate) fn revision(&self) -> Result<String, String> {
        Ok(crate::hex::encode(&Sha256::digest(
            serde_json::to_vec(self).map_err(|e| e.to_string())?,
        )))
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
                        if file.declared_len != *bytes || sha(&file.bytes) != *sha256 {
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

fn sha(bytes: &[u8]) -> String {
    crate::hex::encode(&Sha256::digest(bytes))
}

fn artifacts(root: &Path, spec: &Path) -> Result<BTreeSet<String>, String> {
    let capability = RootCapability::open(root).map_err(|e| e.to_string())?;
    [
        spec.to_path_buf(),
        spec.with_file_name("executor-report.md"),
        spec.with_file_name("audit.md"),
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
                sha256: sha(&file.bytes),
                bytes: file.declared_len,
                executable: file.identity.attributes() & 0o111 != 0,
            };
            Ok((state, Some(file.bytes)))
        }
        Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok((PathState::Missing, None))
        }
        Err(error) => Err(format!(
            "initial_worktree_file_unavailable: {path}: {error}"
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

pub(crate) fn capture(root: &Path, spec: &Path, baseline: &str) -> Result<InitialWorktree, String> {
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
    };
    for file in &files {
        let (state, bytes) = state(&capability, &file.path, deadline)?;
        initial.entries.push(InitialPath {
            path: file.path.clone(),
            state,
        });
        initial.validate(baseline)?;
        if let Some(bytes) = bytes {
            let path = blob_path(root, spec, &sha(&bytes));
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
}
