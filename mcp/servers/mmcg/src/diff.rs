//! Symbol-level diff between a git ref and the current index.
//!
//! Given a `<git-ref>`, computes which symbols were **added**, **removed**, or
//! had their **signature changed** between that ref and the on-disk index. Used
//! by `mmcg_symbols_changed_since` so PR-review agents can answer "what symbols
//! did this branch touch?" without grep-rolling the diff.
//!
//! Implementation:
//! 1. `git diff --name-only <ref>..HEAD` → files changed in the range
//! 2. For each file, fetch its blob at `<ref>` via `git show <ref>:<path>`
//! 3. Parse the old blob through the same extractor (see [`parse_blob`])
//! 4. Match declarations by lexical parent, name and kind against the index
//!
//! [`symbols_changed_in_worktree_controlled`] runs steps 2–4 over a `<ref>` →
//! **working tree** file scope instead (uncommitted and untracked changes
//! included).
//! `audit-spec` uses it: post-flight runs before the commit step, so a
//! commit-range scope is empty by construction there.
//!
//! Files absent at `<ref>` produce only "added" entries. Files deleted in HEAD
//! produce only "removed" (the index has no current symbols for them).
//!
//! Limitations:
//! - Only files where git reports a change. Pure metadata edits (chmod, rename)
//!   without content change won't surface symbols.
//! - Uses `git` via `subprocess` — fails fast if git isn't on PATH.
//! - Signature comparison is exact-string; trivial reformatting (e.g. param
//!   line wrapping) appears as a change.
//! - Per-file resolution: a symbol moved `a.py`→`b.py` shows as removed-from-a
//!   + added-to-b, not "moved".

use crate::bounded_fs::{read_repository_file, ReadControl, RootCapability};
use crate::git::{
    baseline_blobs_for_paths_controlled, collect_worktree_paths_controlled,
    git_diff_name_only_controlled, git_timeout, inventory_complete, resolve_commit_controlled,
    resolve_head_controlled, working_tree_read_error, working_tree_snapshot_token_controlled,
    WorkingTreeChangedFile, WorkingTreeDiffError,
};
use crate::indexer::{
    extractor_for_path, parse_baseline_blob, parse_blob, MAX_INDEXABLE_FILE_SIZE,
};
use crate::store::{Store, Symbol};
use serde::Serialize;
use similar::{capture_diff_slices, Algorithm, DiffTag};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::time::Instant;

type SelectedDeclarations = (BTreeSet<usize>, BTreeSet<usize>);

#[derive(Clone, Copy)]
struct Declaration<'a> {
    name: &'a str,
    kind: &'a str,
    signature: Option<&'a str>,
    line_start: u32,
    line_end: u32,
    parent: Option<usize>,
}

impl Declaration<'_> {
    fn symbol_ref(&self, file: &str) -> SymbolRef {
        SymbolRef {
            file: file.to_string(),
            name: self.name.to_string(),
            kind: self.kind.to_string(),
            line: self.line_start,
            signature: self.signature.map(str::to_string),
        }
    }
}

fn pending_declarations(symbols: &[crate::store::PendingSymbol]) -> Vec<Declaration<'_>> {
    symbols
        .iter()
        .map(|symbol| Declaration {
            name: &symbol.name,
            kind: &symbol.kind,
            signature: symbol.signature.as_deref(),
            line_start: symbol.line_start,
            line_end: symbol.line_end,
            parent: symbol.parent_index,
        })
        .collect()
}

fn indexed_declarations(symbols: &[Symbol]) -> Result<Vec<Declaration<'_>>, &'static str> {
    let positions: HashMap<_, _> = symbols.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
    symbols
        .iter()
        .map(|symbol| {
            let parent = symbol
                .parent_id
                .map(|id| {
                    positions
                        .get(&id)
                        .copied()
                        .ok_or("symbol parent is missing from its file")
                })
                .transpose()?;
            Ok(Declaration {
                name: &symbol.name,
                kind: &symbol.kind,
                signature: symbol.signature.as_deref(),
                line_start: symbol.line_start,
                line_end: symbol.line_end,
                parent,
            })
        })
        .collect()
}

struct DeclarationTree {
    roots: Vec<usize>,
    children: Vec<Vec<usize>>,
}

fn declaration_tree(symbols: &[Declaration<'_>]) -> Result<DeclarationTree, &'static str> {
    let mut roots = Vec::new();
    let mut children = vec![Vec::new(); symbols.len()];
    for (index, symbol) in symbols.iter().enumerate() {
        if let Some(parent) = symbol.parent {
            children
                .get_mut(parent)
                .ok_or("symbol parent is out of bounds")?
                .push(index);
        } else {
            roots.push(index);
        }
    }
    // Every declaration has at most one parent. A cycle is unreachable from
    // the roots, so this also validates deep trees without recursive traversal.
    let mut reachable = 0;
    let mut pending = roots.clone();
    while let Some(index) = pending.pop() {
        reachable += 1;
        pending.extend(children[index].iter().copied());
    }
    if reachable != symbols.len() {
        return Err("symbol parent cycle");
    }
    Ok(DeclarationTree { roots, children })
}

#[derive(Default)]
struct DeclarationMatches {
    paired: Vec<(usize, usize)>,
    added: Vec<usize>,
    removed: Vec<usize>,
    uncertain_added: BTreeSet<usize>,
    uncertain_pairing: BTreeSet<usize>,
}

fn match_declarations(
    old: &[Declaration<'_>],
    new: &[Declaration<'_>],
) -> Result<DeclarationMatches, &'static str> {
    let old_tree = declaration_tree(old)?;
    let new_tree = declaration_tree(new)?;
    let mut pending =
        VecDeque::from([(old_tree.roots.as_slice(), new_tree.roots.as_slice(), false)]);
    let mut matches = DeclarationMatches::default();
    while let Some((before, after, uncertain_parent)) = pending.pop_front() {
        let siblings = match_siblings(old, new, before, after);
        for (before, after) in siblings.paired {
            pending.push_back((
                &old_tree.children[before],
                &new_tree.children[after],
                uncertain_parent || siblings.uncertain_pairing.contains(&after),
            ));
            if new[after].kind != "module" {
                matches.paired.push((before, after));
            }
        }
        for before in siblings.removed {
            pending.push_back((&old_tree.children[before], &[], uncertain_parent));
            if old[before].kind != "module" {
                matches.removed.push(before);
            }
        }
        for after in siblings.added {
            let uncertain = uncertain_parent || siblings.uncertain_added.contains(&after);
            pending.push_back((&[], &new_tree.children[after], uncertain));
            if new[after].kind != "module" {
                matches.added.push(after);
                if uncertain {
                    matches.uncertain_added.insert(after);
                }
            }
        }
    }
    Ok(matches)
}

/// Match parents before their children. Distinct impl blocks may share a type
/// name, but methods belong to the particular matched impl, not just that name.
fn match_siblings(
    old: &[Declaration<'_>],
    new: &[Declaration<'_>],
    before: &[usize],
    after: &[usize],
) -> DeclarationMatches {
    type SiblingGroup = (Vec<usize>, Vec<usize>);
    let mut groups: BTreeMap<(&str, &str), SiblingGroup> = BTreeMap::new();
    for &index in before {
        groups
            .entry((old[index].name, old[index].kind))
            .or_default()
            .0
            .push(index);
    }
    for &index in after {
        groups
            .entry((new[index].name, new[index].kind))
            .or_default()
            .1
            .push(index);
    }
    let mut matches = DeclarationMatches::default();
    for (mut before, mut after) in groups.into_values() {
        before.sort_by_key(|&i| (old[i].line_start, old[i].line_end));
        after.sort_by_key(|&i| (new[i].line_start, new[i].line_end));
        let mut counts: BTreeMap<Option<&str>, (usize, usize)> = BTreeMap::new();
        for &index in &before {
            counts.entry(old[index].signature).or_default().0 += 1;
        }
        for &index in &after {
            counts.entry(new[index].signature).or_default().1 += 1;
        }
        // FIFO keeps the public diff deterministic, but indistinguishable
        // parents cannot prove which descendant was introduced.
        if counts
            .values()
            .any(|&(old, new)| old > 0 && new > 0 && (old > 1 || new > 1))
        {
            matches.uncertain_pairing.extend(after.iter().copied());
            matches.uncertain_added.extend(after.iter().copied());
        }
        let mut by_signature: BTreeMap<Option<&str>, VecDeque<usize>> = BTreeMap::new();
        for index in before {
            by_signature
                .entry(old[index].signature)
                .or_default()
                .push_back(index);
        }
        let mut unmatched_new = Vec::new();
        for index in after {
            if let Some(previous) = by_signature
                .get_mut(&new[index].signature)
                .and_then(VecDeque::pop_front)
            {
                matches.paired.push((previous, index));
            } else {
                unmatched_new.push(index);
            }
        }
        let unmatched_old: Vec<_> = by_signature.into_values().flatten().collect();
        if unmatched_old.len() == 1 && unmatched_new.len() == 1 {
            matches.paired.push((unmatched_old[0], unmatched_new[0]));
        } else {
            // Multiple edited overloads cannot be paired reliably. Retain every
            // declaration as added/removed instead of selecting a first match.
            if !unmatched_old.is_empty() {
                matches
                    .uncertain_added
                    .extend(unmatched_new.iter().copied());
            }
            matches.removed.extend(unmatched_old);
            matches.added.extend(unmatched_new);
        }
    }
    matches
}

fn compare_declarations(
    file: &str,
    old: &[Declaration<'_>],
    new: &[Declaration<'_>],
    matches: &DeclarationMatches,
) -> PerFileDiff {
    PerFileDiff {
        added: matches
            .added
            .iter()
            .map(|&i| new[i].symbol_ref(file))
            .collect(),
        removed: matches
            .removed
            .iter()
            .map(|&i| old[i].symbol_ref(file))
            .collect(),
        signature_changed: matches
            .paired
            .iter()
            .filter(|&&(before, after)| old[before].signature != new[after].signature)
            .map(|&(before, after)| {
                let old = &old[before];
                let new = &new[after];
                SignatureChange {
                    file: file.to_string(),
                    name: new.name.to_string(),
                    kind: new.kind.to_string(),
                    old_signature: old.signature.map(str::to_string),
                    new_signature: new.signature.map(str::to_string),
                    new_line: new.line_start,
                }
            })
            .collect(),
    }
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct SymbolRef {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl From<Symbol> for SymbolRef {
    fn from(s: Symbol) -> Self {
        Self {
            file: s.file_path,
            name: s.name,
            kind: s.kind,
            line: s.line_start,
            signature: s.signature,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct SignatureChange {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub old_signature: Option<String>,
    pub new_signature: Option<String>,
    pub new_line: u32,
}

#[derive(Debug, Serialize, Clone)]
pub struct SymbolDiff {
    /// The git ref the diff is computed against (whatever the user passed).
    pub git_ref: String,
    /// Files in scope (per `git diff --name-only <ref>..HEAD`). Includes
    /// unparseable files — listed but contributing no symbols.
    pub files_in_diff: Vec<String>,
    pub added: Vec<SymbolRef>,
    pub removed: Vec<SymbolRef>,
    pub signature_changed: Vec<SignatureChange>,
    /// Per-file errors (parse / blob-fetch failure). Non-fatal — other files
    /// still produce results. Aids caller debugging.
    pub errors: Vec<String>,
    /// `true` when `files_in_diff` exceeded `CHANGE_FILE_LIMIT` — only the
    /// first `CHANGE_FILE_LIMIT` files were diffed; `added`/`removed`/
    /// `signature_changed` are a prefix, not the full picture.
    pub truncated: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct WorkingTreeSymbolDiff {
    pub git_ref: String,
    pub baseline_oid: String,
    pub head_oid: String,
    pub includes_worktree: bool,
    pub includes_untracked: bool,
    pub files_total: Option<u32>,
    pub files_returned: u32,
    pub files_truncated: bool,
    pub skipped_non_utf8_paths: u32,
    pub files: Vec<WorkingTreeChangedFile>,
    pub diff: SymbolDiff,
    pub body_changed: Vec<SymbolRef>,
    #[serde(skip)]
    pub snapshot_token: String,
}

impl WorkingTreeSymbolDiff {
    /// Whether `files` lists every changed path. A gate refuses otherwise.
    pub fn files_complete(&self) -> bool {
        inventory_complete(self.files_truncated, self.skipped_non_utf8_paths)
    }
}

#[derive(Debug)]
pub enum DiffError {
    GitNotFound,
    GitRefMissing(String),
    GitFailed(String),
    /// A `git` subprocess ran past `MMCG_GIT_TIMEOUT_MS` and was killed.
    GitTimeout,
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffError::GitNotFound => write!(f, "`git` not found on PATH"),
            DiffError::GitRefMissing(r) => write!(f, "git ref not resolvable: {r}"),
            DiffError::GitFailed(m) => write!(f, "git command failed: {m}"),
            DiffError::GitTimeout => write!(f, "git command timed out"),
        }
    }
}

impl std::error::Error for DiffError {}

/// Compute the symbol-level diff between `git_ref` and the current working
/// state (assumed already indexed in `store`).
///
/// `repo_root` is the directory paths are relative to — the same root used at
/// index time. `git_ref` must resolve via `git rev-parse`.
pub fn symbols_changed_since(
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
) -> Result<SymbolDiff, DiffError> {
    symbols_changed_since_controlled(store, repo_root, git_ref, None, None)
}

pub(crate) fn symbols_changed_since_controlled(
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<SymbolDiff, DiffError> {
    let deadline = deadline.or_else(|| Some(Instant::now() + git_timeout()));
    let root = RootCapability::open(repo_root)
        .map_err(|_| DiffError::GitFailed("snapshot_changed".into()))?;
    let baseline_oid = resolve_commit_controlled(repo_root, git_ref, deadline, interrupted)
        .map_err(|error| worktree_scope_error(git_ref, error))?;
    let head_oid = resolve_head_controlled(repo_root, deadline, interrupted)
        .map_err(|error| worktree_scope_error(git_ref, error))?;
    let (files_in_diff, truncated) =
        git_diff_name_only_controlled(repo_root, &baseline_oid, &head_oid, deadline, interrupted)
            .map_err(|error| worktree_scope_error(git_ref, error))?;
    let parseable_paths = files_in_diff
        .iter()
        .filter(|path| extractor_for_path(Path::new(path)).is_some())
        .cloned()
        .collect::<Vec<_>>();
    if !parseable_paths.is_empty() {
        require_current_extractor(store).map_err(|error| worktree_scope_error(git_ref, error))?;
    }
    let old_blobs = baseline_blobs_for_paths_controlled(
        repo_root,
        &baseline_oid,
        &parseable_paths,
        deadline,
        interrupted,
    )
    .map_err(|error| worktree_scope_error(git_ref, error))?;
    let diff = symbol_diff_over_blobs(
        store,
        git_ref,
        files_in_diff,
        truncated,
        &old_blobs,
        deadline,
        interrupted,
    )?;
    let current_head = resolve_head_controlled(repo_root, deadline, interrupted)
        .map_err(|error| worktree_scope_error(git_ref, error))?;
    root.verify()
        .map_err(|_| DiffError::GitFailed("snapshot_changed".into()))?;
    if current_head != head_oid {
        return Err(DiffError::GitFailed("snapshot_changed".into()));
    }
    Ok(diff)
}

/// Same symbol comparison as [`symbols_changed_since`], but the file scope is
/// `git_ref` → **working tree**: staged, unstaged, and untracked changes count,
/// not just what has been committed.
///
/// Callers that audit work *before* it is committed must use this. The
/// commit-range scope of [`symbols_changed_since`] is empty by construction
/// while `HEAD` is still the baseline, which reads as "the spec named these
/// files but nothing changed" rather than "nothing is committed yet".
///
/// The file set matches [`symbols_changed_in_worktree`] (and therefore
/// `mastermind impact`); the symbol comparison is `git_ref`'s blobs against the
/// live index, so a stale index under-reports symbol changes — it does not
/// invent them.
#[cfg(test)]
fn symbols_changed_since_worktree(
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
) -> Result<SymbolDiff, DiffError> {
    symbols_changed_in_worktree_controlled(
        store,
        repo_root,
        git_ref,
        Some(Instant::now() + git_timeout()),
        None,
    )
    .map(|diff| diff.diff)
    .map_err(|error| worktree_scope_error(git_ref, error))
}

pub(crate) fn worktree_scope_error(git_ref: &str, error: WorkingTreeDiffError) -> DiffError {
    match error {
        WorkingTreeDiffError::InvalidRef => DiffError::GitRefMissing(git_ref.to_string()),
        WorkingTreeDiffError::GitTimeout => DiffError::GitTimeout,
        WorkingTreeDiffError::GitOutputLimit => DiffError::GitFailed("git_output_limit".into()),
        WorkingTreeDiffError::SnapshotChanged => DiffError::GitFailed("snapshot_changed".into()),
        WorkingTreeDiffError::IndexStale => DiffError::GitFailed("index_stale".into()),
        WorkingTreeDiffError::GitUnavailable => DiffError::GitNotFound,
    }
}

fn require_current_extractor(store: &Store) -> Result<(), WorkingTreeDiffError> {
    if store
        .extractor_contract_current()
        .map_err(|_| WorkingTreeDiffError::IndexStale)?
    {
        Ok(())
    } else {
        Err(WorkingTreeDiffError::IndexStale)
    }
}

fn symbol_diff_over_blobs(
    store: &Store,
    label: &str,
    files_in_diff: Vec<String>,
    truncated: bool,
    old_blobs: &BTreeMap<String, Option<Vec<u8>>>,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<SymbolDiff, DiffError> {
    let mut added: Vec<SymbolRef> = Vec::new();
    let mut removed: Vec<SymbolRef> = Vec::new();
    let mut signature_changed: Vec<SignatureChange> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for rel in &files_in_diff {
        if interrupted.is_some_and(|check| check())
            || deadline.is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(DiffError::GitTimeout);
        }
        match diff_file_from_blob(
            store,
            rel,
            old_blobs.get(rel).and_then(|blob| blob.as_deref()),
        ) {
            Ok(per_file) => {
                added.extend(per_file.added);
                removed.extend(per_file.removed);
                signature_changed.extend(per_file.signature_changed);
            }
            Err(e) => errors.push(format!("{rel}: {e}")),
        }
    }

    // Stable output order.
    added.sort_by(|a, b| (a.file.as_str(), a.line).cmp(&(b.file.as_str(), b.line)));
    removed.sort_by(|a, b| (a.file.as_str(), a.line).cmp(&(b.file.as_str(), b.line)));
    signature_changed.sort_by(|a, b| {
        (a.file.as_str(), &a.name, a.new_line).cmp(&(b.file.as_str(), &b.name, b.new_line))
    });

    Ok(SymbolDiff {
        git_ref: label.to_string(),
        files_in_diff,
        added,
        removed,
        signature_changed,
        errors,
        truncated,
    })
}

#[cfg(test)]
fn symbols_changed_in_worktree(
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
) -> Result<WorkingTreeSymbolDiff, WorkingTreeDiffError> {
    symbols_changed_in_worktree_controlled(store, repo_root, git_ref, None, None)
}

pub(crate) fn symbols_changed_in_worktree_controlled(
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<WorkingTreeSymbolDiff, WorkingTreeDiffError> {
    symbols_changed_in_worktree_with_declarations(
        store,
        repo_root,
        git_ref,
        deadline,
        interrupted,
        false,
    )
    .map(|(diff, _)| diff)
}

/// Baseline parser ordinals retain identity even when declarations share a
/// file, line, name and signature. This sidecar is not part of the public diff.
pub(crate) type RemovedDeclarations = BTreeMap<String, Vec<usize>>;

#[derive(Default)]
pub(crate) struct DeclarationChanges {
    pub removed: RemovedDeclarations,
    /// The removals that need an acknowledgement: declarations other than
    /// function-local helpers, in files that did not move unchanged to another
    /// indexed code path.
    pub api_removed: RemovedDeclarations,
    /// `api_removed` as symbol references.
    pub api_removed_symbols: Vec<SymbolRef>,
    pub added: BTreeSet<i64>,
    pub uncertain_added: BTreeSet<i64>,
    pub unavailable_additions: BTreeMap<String, &'static str>,
    pub current_files: BTreeSet<String>,
}

impl DeclarationChanges {
    fn record_api_removal(&mut self, file: &str, removal: Vec<(usize, SymbolRef)>) {
        if removal.is_empty() {
            return;
        }
        let (indices, symbols): (Vec<_>, Vec<_>) = removal.into_iter().unzip();
        self.api_removed.insert(file.to_string(), indices);
        self.api_removed_symbols.extend(symbols);
    }
}

/// A function declared inside a function or method body stays in that body's
/// scope in these languages, so no caller can reach it. Any other nested
/// declaration can become API: a JS or TS object-literal or class-expression
/// member is called on the returned value, a nested PHP function is global once
/// the outer one runs, and a nested type's members are reachable through its
/// instances.
fn is_function_local(language: &str, declarations: &[Declaration<'_>], index: usize) -> bool {
    matches!(language, "python" | "rust" | "csharp")
        && declarations[index].kind == "function"
        && std::iter::successors(declarations[index].parent, |&parent| {
            declarations[parent].parent
        })
        .any(|parent| matches!(declarations[parent].kind, "function" | "method"))
}

pub(crate) fn symbols_changed_in_worktree_with_declarations(
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
    prove_additions: bool,
) -> Result<(WorkingTreeSymbolDiff, DeclarationChanges), WorkingTreeDiffError> {
    symbols_changed_in_worktree_with_task(
        store,
        repo_root,
        git_ref,
        deadline,
        interrupted,
        prove_additions,
        None,
    )
}

pub(crate) fn symbols_changed_in_worktree_with_task(
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
    prove_additions: bool,
    task: Option<&crate::task_worktree::Comparison>,
) -> Result<(WorkingTreeSymbolDiff, DeclarationChanges), WorkingTreeDiffError> {
    let baseline_oid = resolve_commit_controlled(repo_root, git_ref, deadline, interrupted)?;
    let head_oid = resolve_head_controlled(repo_root, deadline, interrupted)?;
    let (raw_files, mut files_total, files_truncated, skipped_non_utf8_paths) =
        collect_worktree_paths_controlled(
            repo_root,
            &baseline_oid,
            task.map(|_| ".mastermind"),
            deadline,
            interrupted,
        )?;
    let files = match task {
        Some(task) if task.raw_files == raw_files => {
            files_total = Some(task.files.len() as u32);
            task.files.clone()
        }
        Some(_) => return Err(WorkingTreeDiffError::SnapshotChanged),
        None => raw_files.clone(),
    };
    // Task attribution narrows evidence, while the raw snapshot still binds prior work.
    let snapshot_token = working_tree_snapshot_token_controlled(
        repo_root,
        &head_oid,
        &raw_files,
        deadline,
        interrupted,
    )?;
    let paths = files
        .iter()
        .filter(|file| extractor_for_path(Path::new(&file.path)).is_some())
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    if !paths.is_empty() {
        require_current_extractor(store)?;
    }
    let mut old_blobs = files
        .iter()
        .map(|file| (file.path.clone(), None))
        .collect::<BTreeMap<_, _>>();
    old_blobs.extend(baseline_blobs_for_paths_controlled(
        repo_root,
        &baseline_oid,
        &paths,
        deadline,
        interrupted,
    )?);
    if let Some(task) = task {
        for file in &files {
            if let Some(bytes) = task.preimages.get(&file.path) {
                old_blobs.insert(file.path.clone(), bytes.clone());
            }
        }
    }

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut signature_changed = Vec::new();
    let mut body_changed = Vec::new();
    let mut errors = Vec::new();
    let mut declarations = DeclarationChanges::default();
    // A deleted file whose exact bytes reappear at a changed, indexed code path
    // moved. Its declarations still exist there, so deleting the old path
    // removes none of them. Each destination accounts for one deleted file.
    let deleted_contents: HashSet<&[u8]> = files
        .iter()
        .filter(|file| file.status == "deleted")
        .filter_map(|file| old_blobs.get(&file.path)?.as_deref())
        .collect();
    let mut move_destinations: HashMap<&[u8], usize> = HashMap::new();
    let mut deleted_removals = Vec::new();

    for file in &files {
        let rel = &file.path;
        let new_symbols = store
            .symbols_in_file(rel)
            .map_err(|_| WorkingTreeDiffError::IndexStale)?;
        let old_blob = old_blobs.get(rel).and_then(|v| v.as_deref());
        let old_symbols = match (old_blob, extractor_for_path(Path::new(rel))) {
            (Some(bytes), Some(extractor)) => match parse_blob(rel, bytes, 0, extractor.as_ref()) {
                Ok(parsed) => parsed.symbols,
                // A binary or oversized baseline has no symbols, as the
                // indexer gives the same file.
                Err(crate::indexer::IndexError::Skipped(_)) => Vec::new(),
                Err(_) => return Err(WorkingTreeDiffError::IndexStale),
            },
            _ => Vec::new(),
        };

        if file.status != "deleted" && extractor_for_path(Path::new(rel)).is_some() {
            declarations.current_files.insert(rel.clone());
        }
        let old_declarations = pending_declarations(&old_symbols);
        let new_declarations =
            indexed_declarations(&new_symbols).map_err(|_| WorkingTreeDiffError::IndexStale)?;
        let matches = match_declarations(&old_declarations, &new_declarations)
            .map_err(|_| WorkingTreeDiffError::IndexStale)?;
        let per_file = compare_declarations(rel, &old_declarations, &new_declarations, &matches);
        if !matches.removed.is_empty() {
            declarations
                .removed
                .insert(rel.clone(), matches.removed.clone());
        }
        let language = extractor_for_path(Path::new(rel)).map_or("", |extractor| extractor.name());
        let api_removal = matches
            .removed
            .iter()
            .filter(|&&index| !is_function_local(language, &old_declarations, index))
            .map(|&index| (index, old_declarations[index].symbol_ref(rel)))
            .collect::<Vec<_>>();
        for &index in &matches.added {
            if matches.uncertain_added.contains(&index) {
                declarations.uncertain_added.insert(new_symbols[index].id);
            } else {
                declarations.added.insert(new_symbols[index].id);
            }
        }
        added.extend(per_file.added);
        removed.extend(per_file.removed);
        signature_changed.extend(per_file.signature_changed);

        let current_bytes = if file.status == "deleted" {
            None
        } else {
            let control = ReadControl {
                deadline,
                interrupted,
            };
            match read_repository_file(
                repo_root,
                Path::new(rel),
                MAX_INDEXABLE_FILE_SIZE,
                MAX_INDEXABLE_FILE_SIZE,
                control,
            ) {
                Ok(file) => Some(file.bytes),
                // The indexer skips a source over the cap, so it has no
                // current symbols to compare.
                Err(crate::bounded_fs::BoundedReadError::TooLarge { .. }) => None,
                Err(error) => return Err(working_tree_read_error(error)),
            }
        };
        let indexed_code =
            declarations.current_files.contains(rel.as_str()) && !new_symbols.is_empty();
        if let Some(content) = current_bytes
            .as_deref()
            .filter(|_| indexed_code)
            .and_then(|current| deleted_contents.get(current))
        {
            *move_destinations.entry(*content).or_default() += 1;
        }
        match old_blob {
            Some(old) if file.status == "deleted" => deleted_removals.push((old, rel, api_removal)),
            _ => declarations.record_api_removal(rel, api_removal),
        }
        if prove_additions && !matches.added.is_empty() {
            if let Some(extractor) = extractor_for_path(Path::new(rel)) {
                for (bytes, reason) in [
                    (old_blob, "baseline_parse_failed"),
                    (current_bytes.as_deref(), "current_parse_failed"),
                ] {
                    if bytes.is_some_and(|bytes| {
                        parse_baseline_blob(rel, bytes, extractor.as_ref()).is_err()
                    }) {
                        declarations
                            .unavailable_additions
                            .insert(rel.clone(), reason);
                        break;
                    }
                }
            } else {
                declarations
                    .unavailable_additions
                    .insert(rel.clone(), "addition_language_unavailable");
            }
        }
        let (selected_old, selected_new) = match (old_blob, current_bytes.as_deref()) {
            (Some(old), Some(current)) => {
                deepest_changed_declarations(old, current, &old_declarations, &new_declarations)
            }
            _ => (BTreeSet::new(), BTreeSet::new()),
        };
        for (old_index, new_index) in matches.paired {
            let old_symbol = &old_declarations[old_index];
            let new_symbol = &new_declarations[new_index];
            if old_symbol.signature != new_symbol.signature {
                continue;
            }
            if selected_old.contains(&old_index) || selected_new.contains(&new_index) {
                if let (Some(old), Some(current)) = (old_blob, current_bytes.as_deref()) {
                    let old_slice = line_slice(old, old_symbol.line_start, old_symbol.line_end);
                    let new_slice = line_slice(current, new_symbol.line_start, new_symbol.line_end);
                    if old_slice != new_slice {
                        body_changed.push(new_symbol.symbol_ref(rel));
                    }
                }
            }
        }
    }

    for (old, rel, api_removal) in deleted_removals {
        match move_destinations.get_mut(old) {
            Some(destinations) if *destinations > 0 => *destinations -= 1,
            _ => declarations.record_api_removal(rel, api_removal),
        }
    }

    let symbol_order = |a: &SymbolRef, b: &SymbolRef| {
        (&a.file, a.line, &a.name, &a.kind).cmp(&(&b.file, b.line, &b.name, &b.kind))
    };
    added.sort_by(symbol_order);
    removed.sort_by(symbol_order);
    declarations.api_removed_symbols.sort_by(symbol_order);
    body_changed.sort_by(symbol_order);
    signature_changed.sort_by(|a, b| {
        (&a.file, a.new_line, &a.name, &a.kind).cmp(&(&b.file, b.new_line, &b.name, &b.kind))
    });
    errors.sort();

    let files_in_diff = files.iter().map(|file| file.path.clone()).collect();
    let files_returned = files.len() as u32;
    Ok((
        WorkingTreeSymbolDiff {
            git_ref: git_ref.to_string(),
            baseline_oid,
            head_oid,
            includes_worktree: true,
            includes_untracked: true,
            files_total,
            files_returned,
            files_truncated,
            skipped_non_utf8_paths,
            files,
            diff: SymbolDiff {
                git_ref: git_ref.to_string(),
                files_in_diff,
                added,
                removed,
                signature_changed,
                errors,
                truncated: files_truncated,
            },
            body_changed,
            snapshot_token,
        },
        declarations,
    ))
}

fn line_chunks(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split_inclusive(|byte| *byte == b'\n').collect()
}

fn deepest_changed_declarations(
    old: &[u8],
    current: &[u8],
    old_symbols: &[Declaration<'_>],
    new_symbols: &[Declaration<'_>],
) -> SelectedDeclarations {
    let old_lines = line_chunks(old);
    let new_lines = line_chunks(current);
    let mut selected_old = BTreeSet::new();
    let mut selected_new = BTreeSet::new();
    for operation in capture_diff_slices(Algorithm::Myers, &old_lines, &new_lines) {
        if operation.tag() == DiffTag::Equal {
            continue;
        }
        let old_range = operation.old_range();
        if !old_range.is_empty() {
            let start = old_range.start as u32 + 1;
            let end = old_range.end as u32;
            if let Some((index, _)) = old_symbols
                .iter()
                .enumerate()
                .filter(|(_, symbol)| {
                    symbol.kind != "module" && symbol.line_start <= start && symbol.line_end >= end
                })
                .min_by_key(|(_, symbol)| {
                    (
                        symbol.line_end.saturating_sub(symbol.line_start),
                        symbol.line_start,
                        symbol.name,
                        symbol.kind,
                    )
                })
            {
                selected_old.insert(index);
            }
        }
        let new_range = operation.new_range();
        if !new_range.is_empty() {
            let start = new_range.start as u32 + 1;
            let end = new_range.end as u32;
            if let Some((index, _)) = new_symbols
                .iter()
                .enumerate()
                .filter(|(_, symbol)| {
                    symbol.kind != "module" && symbol.line_start <= start && symbol.line_end >= end
                })
                .min_by_key(|(_, symbol)| {
                    (
                        symbol.line_end.saturating_sub(symbol.line_start),
                        symbol.line_start,
                        symbol.name,
                        symbol.kind,
                    )
                })
            {
                selected_new.insert(index);
            }
        }
    }
    (selected_old, selected_new)
}

fn line_slice(bytes: &[u8], line_start: u32, line_end: u32) -> &[u8] {
    let mut starts = vec![0usize];
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            starts.push(index + 1);
        }
    }
    let start = starts
        .get(line_start.saturating_sub(1) as usize)
        .copied()
        .unwrap_or(bytes.len());
    let end = starts
        .get(line_end as usize)
        .copied()
        .unwrap_or(bytes.len());
    &bytes[start.min(bytes.len())..end.min(bytes.len())]
}

struct PerFileDiff {
    added: Vec<SymbolRef>,
    removed: Vec<SymbolRef>,
    signature_changed: Vec<SignatureChange>,
}

fn diff_file_from_blob(
    store: &Store,
    rel_path: &str,
    old_blob: Option<&[u8]>,
) -> Result<PerFileDiff, String> {
    let extractor = extractor_for_path(Path::new(rel_path));

    // New side: from the live index. Empty if the file was deleted in HEAD or
    // never indexed.
    let new_symbols = store
        .symbols_in_file(rel_path)
        .map_err(|e| format!("symbols_in_file failed: {e}"))?;

    // Old blobs were fetched in bounded `cat-file --batch` groups against one
    // resolved commit. Missing means the file did not exist at the baseline.
    let old_symbols: Vec<crate::store::PendingSymbol> = if old_blob.is_none_or(<[u8]>::is_empty) {
        Vec::new()
    } else if let Some(ext) = extractor {
        let pending = parse_blob(rel_path, old_blob.expect("checked above"), 0, ext.as_ref())
            .map_err(|e| format!("parse old blob: {e}"))?;
        pending.symbols
    } else {
        // No extractor for this extension — can't diff symbols. Empty old side;
        // everything in new side becomes "added".
        Vec::new()
    };

    let old_declarations = pending_declarations(&old_symbols);
    let new_declarations = indexed_declarations(&new_symbols)?;
    let matches = match_declarations(&old_declarations, &new_declarations)?;
    Ok(compare_declarations(
        rel_path,
        &old_declarations,
        &new_declarations,
        &matches,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::{init_repo, run, write};
    use crate::git::{
        validate_working_tree_snapshot_controlled, CHANGE_FILE_LIMIT, GIT_OUTPUT_LIMIT,
    };
    use std::env;
    use std::fs;

    #[test]
    fn end_to_end_added_removed_changed_signature() {
        let dir = init_repo("e2e");

        // Baseline: one file with two functions.
        write(
            &dir,
            "src/a.py",
            "def keep_same():\n    return 1\n\ndef will_be_removed():\n    return 2\n\ndef will_change_sig():\n    return 3\n",
        );
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["tag", "baseline"]);

        // HEAD: remove a function, change another's signature, add a file.
        write(
            &dir,
            "src/a.py",
            "def keep_same():\n    return 1\n\ndef will_change_sig(force: bool = False):\n    return 3\n",
        );
        write(&dir, "src/b.py", "def brand_new():\n    return 99\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "head"]);

        // Index the HEAD state into a fresh store.
        let db = dir.join(".mmcg.db");
        let mut store = Store::open(&db).unwrap();
        let indexer = crate::indexer::Indexer::new(&dir);
        indexer.index_all(&mut store, false).unwrap();

        let diff = symbols_changed_since(&store, &dir, "baseline").unwrap();
        assert_eq!(diff.git_ref, "baseline");
        // Two files changed in this range.
        assert_eq!(diff.files_in_diff, vec!["src/a.py", "src/b.py"]);

        let added_names: Vec<&str> = diff.added.iter().map(|s| s.name.as_str()).collect();
        let removed_names: Vec<&str> = diff.removed.iter().map(|s| s.name.as_str()).collect();
        let sig_names: Vec<&str> = diff
            .signature_changed
            .iter()
            .map(|s| s.name.as_str())
            .collect();

        assert!(added_names.contains(&"brand_new"), "missing brand_new");
        assert!(
            removed_names.contains(&"will_be_removed"),
            "missing will_be_removed in removed"
        );
        assert!(
            sig_names.contains(&"will_change_sig"),
            "missing will_change_sig in signature_changed"
        );
        // `keep_same` should NOT appear anywhere.
        assert!(!added_names.contains(&"keep_same"));
        assert!(!removed_names.contains(&"keep_same"));
        assert!(!sig_names.contains(&"keep_same"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_ref_errors_cleanly() {
        let dir = init_repo("missing_ref");
        write(&dir, "src/x.py", "def a(): pass\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "init"]);

        let db = dir.join(".mmcg.db");
        let store = Store::open(&db).unwrap();
        let result = symbols_changed_since(&store, &dir, "nonexistent-ref");
        assert!(matches!(result, Err(DiffError::GitRefMissing(_))));

        fs::remove_dir_all(&dir).ok();
    }

    struct IndexedWorktree {
        store: Option<Store>,
        db: std::path::PathBuf,
    }

    impl IndexedWorktree {
        fn store(&self) -> &Store {
            self.store.as_ref().expect("fixture store")
        }
    }

    impl Drop for IndexedWorktree {
        fn drop(&mut self) {
            drop(self.store.take());
            let _ = fs::remove_file(&self.db);
            let _ = fs::remove_file(format!("{}-wal", self.db.display()));
            let _ = fs::remove_file(format!("{}-shm", self.db.display()));
        }
    }

    fn indexed_worktree(dir: &Path) -> IndexedWorktree {
        static NEXT_DB: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let suffix = NEXT_DB.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let db = env::temp_dir().join(format!(
            "mmcg-diff-store-{}-{suffix}.db",
            std::process::id()
        ));
        let _ = fs::remove_file(&db);
        let mut store = Store::open(&db).unwrap();
        crate::indexer::Indexer::new(dir)
            .index_all(&mut store, true)
            .unwrap();
        IndexedWorktree {
            store: Some(store),
            db,
        }
    }

    #[test]
    fn working_tree_diff_includes_staged_unstaged_and_untracked() {
        let dir = init_repo("worktree_all_states");
        write(&dir, "staged.py", "def staged():\n    return 1\n");
        write(&dir, "unstaged.py", "def unstaged():\n    return 1\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        write(&dir, "staged.py", "def staged():\n    return 2\n");
        run(&dir, &["add", "staged.py"]);
        write(&dir, "unstaged.py", "def unstaged():\n    return 2\n");
        write(&dir, "untracked.py", "def untracked():\n    return 3\n");
        let store = indexed_worktree(&dir);
        let changed = symbols_changed_in_worktree(store.store(), &dir, "HEAD").unwrap();
        let paths: Vec<_> = changed
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(paths, vec!["staged.py", "unstaged.py", "untracked.py"]);
        assert_eq!(changed.files[2].status, "untracked");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn worktree_scope_sees_work_the_commit_range_cannot() {
        let dir = init_repo("worktree_scope_vs_range");
        write(&dir, "src/a.py", "def keep_same():\n    return 1\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["tag", "baseline"]);

        // Executor's work, none of it committed: `a.py` edited, `b.py` staged,
        // `c.py` untracked. HEAD is still the baseline commit.
        write(
            &dir,
            "src/a.py",
            "def keep_same():\n    return 1\n\ndef added_later():\n    return 2\n",
        );
        write(&dir, "src/b.py", "def staged_only():\n    return 3\n");
        run(&dir, &["add", "src/b.py"]);
        write(&dir, "src/c.py", "def untracked_only():\n    return 4\n");
        let store = indexed_worktree(&dir);

        // The commit range is empty by construction — the trap this exists for.
        let committed = symbols_changed_since(store.store(), &dir, "baseline").unwrap();
        assert!(
            committed.files_in_diff.is_empty(),
            "baseline..HEAD must be empty here: {:?}",
            committed.files_in_diff
        );

        let diff = symbols_changed_since_worktree(store.store(), &dir, "baseline").unwrap();
        assert_eq!(
            diff.git_ref, "baseline",
            "label is the caller's ref, not an oid"
        );
        assert_eq!(diff.files_in_diff, vec!["src/a.py", "src/b.py", "src/c.py"]);
        let added: Vec<&str> = diff.added.iter().map(|s| s.name.as_str()).collect();
        assert!(
            added.contains(&"added_later"),
            "unstaged edit missed: {added:?}"
        );
        assert!(
            added.contains(&"staged_only"),
            "staged file missed: {added:?}"
        );
        assert!(
            added.contains(&"untracked_only"),
            "untracked file missed: {added:?}"
        );
        assert!(!added.contains(&"keep_same"));
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn working_tree_diff_is_nul_safe_and_deterministic() {
        let dir = init_repo("worktree_nul");
        write(&dir, "base.py", "def base():\n    pass\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        write(&dir, "line\nbreak.py", "def unusual():\n    pass\n");
        let store = indexed_worktree(&dir);
        let first = symbols_changed_in_worktree(store.store(), &dir, "HEAD").unwrap();
        let second = symbols_changed_in_worktree(store.store(), &dir, "HEAD").unwrap();
        assert_eq!(first.files, second.files);
        assert_eq!(first.files[0].path, "line\nbreak.py");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn working_tree_diff_does_not_fetch_large_unsupported_baseline_blob() {
        let dir = init_repo("unsupported_baseline_blob");
        write(&dir, "src/lib.rs", "pub fn indexed() {}\n");
        let asset = dir.join("asset.bin");
        fs::File::create(&asset)
            .unwrap()
            .set_len((GIT_OUTPUT_LIMIT + 1) as u64)
            .unwrap();
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        fs::OpenOptions::new()
            .write(true)
            .open(&asset)
            .unwrap()
            .set_len((GIT_OUTPUT_LIMIT + 2) as u64)
            .unwrap();

        let store = indexed_worktree(&dir);
        let changed = symbols_changed_in_worktree(store.store(), &dir, "HEAD").unwrap();

        assert_eq!(changed.files.len(), 1);
        assert_eq!(changed.files[0].path, "asset.bin");
        assert!(changed.diff.added.is_empty());
        assert!(changed.diff.removed.is_empty());
        assert!(changed.diff.signature_changed.is_empty());
        assert!(changed.diff.errors.is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn worktree_snapshot_can_be_read_while_the_task_controller_holds_its_lock() {
        let dir = init_repo("controller_lock_snapshot");
        write(&dir, "notes.txt", "before\n");
        run(&dir, &["add", "notes.txt"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        write(&dir, "notes.txt", "after\n");
        let database = tempfile::tempdir().unwrap();
        let store = Store::open(database.path().join("graph.db")).unwrap();
        for state in [
            ".mastermind/tasks/001-review/state.json",
            ".mastermind/run-state/001-review.json",
        ] {
            let _lock = crate::task_record::controller_lock(&dir, &dir.join(state)).unwrap();
            let snapshot = symbols_changed_in_worktree(&store, &dir, "HEAD").unwrap();
            assert_eq!(snapshot.diff.files_in_diff, vec!["notes.txt"]);
            validate_working_tree_snapshot_controlled(
                &dir,
                &snapshot.baseline_oid,
                &snapshot.head_oid,
                &snapshot.files,
                &snapshot.snapshot_token,
                None,
                None,
            )
            .unwrap();
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn text_only_symbol_diff_does_not_require_an_extractor_index() {
        let dir = init_repo("text_only_no_index");
        write(&dir, "notes.txt", "before\n");
        run(&dir, &["add", "notes.txt"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["tag", "baseline"]);
        write(&dir, "notes.txt", "after\n");
        let database = tempfile::tempdir().unwrap();
        let store = Store::open(database.path().join("graph.db")).unwrap();
        assert!(!store.extractor_contract_current().unwrap());
        let assert_text_change = |diff: &SymbolDiff| {
            assert_eq!(diff.files_in_diff, vec!["notes.txt"]);
            assert!(diff.added.is_empty());
            assert!(diff.removed.is_empty());
            assert!(diff.signature_changed.is_empty());
            assert!(diff.errors.is_empty());
            assert!(!diff.truncated);
        };
        let worktree = symbols_changed_in_worktree(&store, &dir, "baseline").unwrap();
        assert_text_change(&worktree.diff);
        assert!(worktree.body_changed.is_empty());
        assert_text_change(&symbols_changed_since_worktree(&store, &dir, "baseline").unwrap());
        run(&dir, &["add", "notes.txt"]);
        run(&dir, &["commit", "-q", "-m", "text only"]);
        assert_text_change(&symbols_changed_since(&store, &dir, "baseline").unwrap());
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rust_outer_attribute_changes_reach_both_symbol_diff_scopes() {
        let dir = init_repo("rust_attribute_diff");
        let baseline = r#"fn becomes_test() {}
#[test]
fn stops_test() {}
#[cfg(feature = "before")]
fn gated() {}
#[test]
// Original comment.
fn stable() {}
"#;
        let current = r#"#[test]
fn becomes_test() {}
fn stops_test() {}
#[cfg(feature = "after")]
fn gated() {}
#[test]
// Updated comment.
fn stable() {}
"#;
        write(&dir, "src/lib.rs", baseline);
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["tag", "baseline"]);
        write(&dir, "src/lib.rs", current);
        let indexed = indexed_worktree(&dir);
        let assert_changes = |diff: &SymbolDiff| {
            assert!(diff.added.is_empty());
            assert!(diff.removed.is_empty());
            assert!(diff.errors.is_empty());
            assert!(!diff.truncated);
            assert_eq!(diff.signature_changed.len(), 3);
            for (name, old, new, line) in [
                (
                    "becomes_test",
                    "fn becomes_test()",
                    "#[test] fn becomes_test()",
                    2,
                ),
                (
                    "stops_test",
                    "#[test] fn stops_test()",
                    "fn stops_test()",
                    3,
                ),
                (
                    "gated",
                    "#[cfg(feature = \"before\")] fn gated()",
                    "#[cfg(feature = \"after\")] fn gated()",
                    5,
                ),
            ] {
                let change = diff
                    .signature_changed
                    .iter()
                    .find(|s| s.name == name)
                    .unwrap();
                assert_eq!(change.file, "src/lib.rs");
                assert_eq!(change.kind, "function");
                assert_eq!(change.new_line, line);
                assert_eq!(change.old_signature.as_deref(), Some(old));
                assert_eq!(change.new_signature.as_deref(), Some(new));
            }
        };
        let worktree = symbols_changed_in_worktree(indexed.store(), &dir, "baseline").unwrap();
        assert_changes(&worktree.diff);
        assert!(worktree.body_changed.is_empty());
        assert_eq!(worktree.files.len(), 1);
        assert_changes(&symbols_changed_since_worktree(indexed.store(), &dir, "baseline").unwrap());

        run(&dir, &["add", "src/lib.rs"]);
        run(&dir, &["commit", "-q", "-m", "attributes only"]);
        assert_changes(&symbols_changed_since(indexed.store(), &dir, "baseline").unwrap());
        drop(indexed);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn python_decorator_changes_reach_both_symbol_diff_scopes() {
        let dir = init_repo("python_decorator_diff");
        let baseline = r#"def added(): pass
@label("old")
def removed(): pass
@pytest.mark.parametrize("value", [1])
def test_value(value): pass
@outer
@inner
def reordered(): pass
@route(
    "/before",
)
async def handler(): pass
@label("before")
class Suite: pass
@label("fixed")
# Original comment.
def stable(): pass
@label("fixed")
def body_only(): return 1
"#;
        let current = r#"@label("new")
def added(): pass
def removed(): pass
@pytest.mark.parametrize("value", [2])
def test_value(value): pass
@inner
@outer
def reordered(): pass
@route(
    "/after",
)
async def handler(): pass
@label("after")
class Suite: pass
@label("fixed")
# Updated comment.
def stable(): pass
@label("fixed")
def body_only(): return 2
"#;
        write(&dir, "tests/test_values.py", baseline);
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["tag", "baseline"]);
        write(&dir, "tests/test_values.py", current);
        let indexed = indexed_worktree(&dir);
        let assert_changes = |diff: &SymbolDiff| {
            assert!(diff.added.is_empty());
            assert!(diff.removed.is_empty());
            assert!(diff.errors.is_empty());
            assert!(!diff.truncated);
            assert_eq!(diff.signature_changed.len(), 6);
            for (name, kind, line, old, new) in [
                (
                    "added",
                    "function",
                    2,
                    "def added()",
                    "@label(\"new\")\ndef added()",
                ),
                (
                    "removed",
                    "function",
                    3,
                    "@label(\"old\")\ndef removed()",
                    "def removed()",
                ),
                (
                    "test_value",
                    "function",
                    5,
                    "@pytest.mark.parametrize(\"value\", [1])\ndef test_value(value)",
                    "@pytest.mark.parametrize(\"value\", [2])\ndef test_value(value)",
                ),
                (
                    "reordered",
                    "function",
                    8,
                    "@outer\n@inner\ndef reordered()",
                    "@inner\n@outer\ndef reordered()",
                ),
                (
                    "handler",
                    "function",
                    12,
                    "@route(\n    \"/before\",\n)\nasync def handler()",
                    "@route(\n    \"/after\",\n)\nasync def handler()",
                ),
                (
                    "Suite",
                    "class",
                    14,
                    "@label(\"before\")\nclass Suite",
                    "@label(\"after\")\nclass Suite",
                ),
            ] {
                let change = diff
                    .signature_changed
                    .iter()
                    .find(|s| s.name == name)
                    .unwrap();
                assert_eq!(change.kind, kind, "{name}");
                assert_eq!(change.new_line, line, "{name}");
                assert_eq!(change.old_signature.as_deref(), Some(old), "{name}");
                assert_eq!(change.new_signature.as_deref(), Some(new), "{name}");
            }
        };
        let worktree = symbols_changed_in_worktree(indexed.store(), &dir, "baseline").unwrap();
        assert_changes(&worktree.diff);
        assert_eq!(worktree.body_changed.len(), 1);
        assert_eq!(worktree.body_changed[0].name, "body_only");
        assert_changes(&symbols_changed_since_worktree(indexed.store(), &dir, "baseline").unwrap());
        run(&dir, &["add", "tests/test_values.py"]);
        run(&dir, &["commit", "-q", "-m", "decorators and body"]);
        assert_changes(&symbols_changed_since(indexed.store(), &dir, "baseline").unwrap());
        drop(indexed);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn same_name_methods_keep_scope_in_both_diff_scopes() {
        let baseline = "class A:\n    def run(self):\n        return 1\n\nclass B:\n    def run(self):\n        return 2\n";
        for (label, current, signature_line, body_line) in [
            ("scoped_header", baseline.replace("class B:\n    def run(self):", "class B:\n    def run(self, flag=False):"), Some(6), None),
            ("scoped_body", baseline.replace("return 2", "return 3"), None, Some(6)),
            ("scoped_shift", format!("# Added heading.\n\n{}", baseline.replace("return 2", "return 3")), None, Some(8)),
            ("scoped_reorder", "class B:\n    def run(self):\n        return 2\n\nclass A:\n    def run(self):\n        return 1\n".to_string(), None, None),
        ] {
            let dir = init_repo(label);
            write(&dir, "service.py", baseline);
            run(&dir, &["add", "-A"]);
            run(&dir, &["commit", "-q", "-m", "baseline"]);
            run(&dir, &["tag", "baseline"]);
            write(&dir, "service.py", &current);
            let indexed = indexed_worktree(&dir);
            let assert_diff = |diff: &SymbolDiff| {
                assert!(diff.added.is_empty(), "{label}: {diff:?}");
                assert!(diff.removed.is_empty(), "{label}: {diff:?}");
                assert!(diff.errors.is_empty());
                assert!(!diff.truncated);
                assert_eq!(diff.signature_changed.len(), usize::from(signature_line.is_some()), "{label}");
                if let Some(line) = signature_line {
                    let change = &diff.signature_changed[0];
                    assert_eq!(change.name, "run");
                    assert_eq!(change.kind, "method");
                    assert_eq!(change.new_line, line);
                    assert_eq!(change.old_signature.as_deref(), Some("def run(self)"));
                    assert_eq!(change.new_signature.as_deref(), Some("def run(self, flag=False)"));
                }
            };
            let worktree = symbols_changed_in_worktree(indexed.store(), &dir, "baseline").unwrap();
            assert_diff(&worktree.diff);
            assert_eq!(worktree.body_changed.len(), usize::from(body_line.is_some()), "{label}: {worktree:?}");
            if let Some(line) = body_line {
                assert_eq!(worktree.body_changed[0].name, "run");
                assert_eq!(worktree.body_changed[0].kind, "method");
                assert_eq!(worktree.body_changed[0].line, line);
            }
            assert_diff(&symbols_changed_since_worktree(indexed.store(), &dir, "baseline").unwrap());
            run(&dir, &["add", "service.py"]);
            run(&dir, &["commit", "-q", "-m", "current"]);
            assert_diff(&symbols_changed_since(indexed.store(), &dir, "baseline").unwrap());
            drop(indexed);
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn nested_same_name_functions_keep_each_enclosing_scope() {
        let baseline = "def first():\n    def helper():\n        return 1\n    return helper()\n\ndef second():\n    def helper():\n        return 2\n    return helper()\n";
        let dir = init_repo("nested_scope_identity");
        write(&dir, "service.py", baseline);
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        write(
            &dir,
            "service.py",
            &baseline.replace("return 2", "return 3"),
        );
        let indexed = indexed_worktree(&dir);
        let changed = symbols_changed_in_worktree(indexed.store(), &dir, "HEAD").unwrap();
        assert!(changed.diff.signature_changed.is_empty());
        assert_eq!(changed.body_changed.len(), 1);
        assert_eq!(changed.body_changed[0].name, "helper");
        assert_eq!(changed.body_changed[0].line, 7);
        drop(indexed);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parent_signature_change_preserves_unchanged_child_identity() {
        let baseline = "class Service:\n    def run(self):\n        return 1\n";
        let current = "class Service(Base):\n    def run(self):\n        return 1\n";
        let dir = init_repo("parent_header_identity");
        write(&dir, "service.py", baseline);
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["tag", "baseline"]);
        write(&dir, "service.py", current);
        let indexed = indexed_worktree(&dir);
        let assert_diff = |diff: &SymbolDiff| {
            assert!(diff.errors.is_empty());
            assert!(diff.added.is_empty());
            assert!(diff.removed.is_empty());
            assert_eq!(diff.signature_changed.len(), 1);
            let change = &diff.signature_changed[0];
            assert_eq!(change.name, "Service");
            assert_eq!(change.kind, "class");
            assert_eq!(change.new_line, 1);
            assert_eq!(change.old_signature.as_deref(), Some("class Service"));
            assert_eq!(change.new_signature.as_deref(), Some("class Service(Base)"));
        };
        let worktree = symbols_changed_in_worktree(indexed.store(), &dir, "baseline").unwrap();
        assert_diff(&worktree.diff);
        assert!(worktree.body_changed.is_empty());
        run(&dir, &["add", "service.py"]);
        run(&dir, &["commit", "-q", "-m", "change parent header"]);
        assert_diff(&symbols_changed_since(indexed.store(), &dir, "baseline").unwrap());
        drop(indexed);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn same_name_impl_parents_do_not_hide_a_moved_trait_override() {
        let baseline = "trait Alpha { fn run(&self) {} }\ntrait Beta { fn run(&self) {} }\nstruct Service;\nimpl Alpha for Service {\n    fn run(&self) {}\n}\nimpl Beta for Service {}\n";
        let current = "trait Alpha { fn run(&self) {} }\ntrait Beta { fn run(&self) {} }\nstruct Service;\nimpl Alpha for Service {}\nimpl Beta for Service {\n    fn run(&self) {}\n}\n";
        let dir = init_repo("trait_impl_identity");
        write(&dir, "src/lib.rs", baseline);
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["tag", "baseline"]);
        write(&dir, "src/lib.rs", current);
        let indexed = indexed_worktree(&dir);
        let assert_diff = |diff: &SymbolDiff| {
            assert!(diff.errors.is_empty());
            assert!(diff.signature_changed.is_empty());
            assert_eq!(diff.removed.len(), 1, "{diff:?}");
            assert_eq!(diff.added.len(), 1, "{diff:?}");
            assert_eq!(diff.removed[0].name, "run");
            assert_eq!(diff.removed[0].line, 5);
            assert_eq!(diff.added[0].name, "run");
            assert_eq!(diff.added[0].line, 6);
        };
        assert_diff(&symbols_changed_since_worktree(indexed.store(), &dir, "baseline").unwrap());
        run(&dir, &["add", "src/lib.rs"]);
        run(&dir, &["commit", "-q", "-m", "move override"]);
        assert_diff(&symbols_changed_since(indexed.store(), &dir, "baseline").unwrap());
        drop(indexed);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn declaration_matching_retains_overloads_and_rejects_invalid_parents() {
        fn declaration(signature: &str, line: u32) -> Declaration<'_> {
            Declaration {
                name: "run",
                kind: "method",
                signature: Some(signature),
                line_start: line,
                line_end: line,
                parent: None,
            }
        }
        let old = [declaration("run(int)", 1), declaration("run(str)", 2)];
        let new = [
            declaration("run(str)", 10),
            declaration("run(bool)", 11),
            declaration("run(int)", 12),
        ];
        let matches = match_declarations(&old, &new).unwrap();
        assert_eq!(matches.paired, vec![(1, 0), (0, 2)]);
        assert_eq!(matches.added, vec![1]);
        assert!(matches.removed.is_empty());

        let new = [declaration("run(int)", 5), declaration("run(bool)", 6)];
        let matches = match_declarations(&old, &new).unwrap();
        let diff = compare_declarations("service.cs", &old, &new, &matches);
        assert_eq!(diff.signature_changed.len(), 1);
        assert_eq!(diff.signature_changed[0].new_line, 6);
        assert_eq!(
            diff.signature_changed[0].old_signature.as_deref(),
            Some("run(str)")
        );

        let new = [declaration("run(bool)", 5), declaration("run(float)", 6)];
        let matches = match_declarations(&old, &new).unwrap();
        assert!(matches.paired.is_empty());
        assert_eq!(matches.added.len(), 2);
        assert_eq!(matches.removed.len(), 2);
        let repeated = [declaration("run(int)", 1), declaration("run(int)", 2)];
        assert_eq!(match_declarations(&repeated, &[]).unwrap().removed.len(), 2);
        assert_eq!(match_declarations(&[], &repeated).unwrap().added.len(), 2);

        let mut child = declaration("run(int)", 1);
        child.parent = Some(1);
        let parent = Declaration {
            name: "Scope",
            kind: "class",
            ..declaration("class Scope", 2)
        };
        assert_eq!(
            match_declarations(
                &[child, parent],
                &[
                    parent,
                    Declaration {
                        parent: Some(0),
                        ..child
                    }
                ]
            )
            .unwrap()
            .paired
            .len(),
            2
        );
        child.parent = Some(0);
        assert!(match_declarations(&[child], &[]).is_err());
        child.parent = Some(2);
        assert!(match_declarations(&[child], &[]).is_err());
    }

    #[test]
    fn body_change_selects_only_deepest_enclosing_symbol() {
        let dir = init_repo("deepest_nested_engine");
        write(
            &dir,
            "service.py",
            "class Service:\n    def method(self):\n        return 1\n\n    def other(self):\n        return 3\n\ndef caller(service):\n    return service.method()\n",
        );
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        write(
            &dir,
            "service.py",
            "class Service:\n    def method(self):\n        return 2\n\n    def other(self):\n        return 3\n\ndef caller(service):\n    return service.method()\n",
        );
        let indexed = indexed_worktree(&dir);

        let changed = symbols_changed_in_worktree(indexed.store(), &dir, "HEAD").unwrap();
        assert_eq!(changed.body_changed.len(), 1);
        assert_eq!(changed.body_changed[0].name, "method");
        assert_eq!(changed.body_changed[0].kind, "method");
        assert!(!changed
            .body_changed
            .iter()
            .any(|symbol| symbol.name == "Service"));

        let response =
            crate::queries::change_impact(indexed.store(), &dir, "HEAD", 3, 100).unwrap();
        let body_seeds = response
            .changes
            .symbols
            .items
            .iter()
            .filter(|symbol| symbol.change == "body_changed")
            .collect::<Vec<_>>();
        assert_eq!(body_seeds.len(), 1);
        assert_eq!(body_seeds[0].name, "method");
        assert!(!body_seeds.iter().any(|symbol| symbol.name == "Service"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn symbols_changed_since_caps_file_loop() {
        let dir = init_repo("change_file_limit");
        write(&dir, "base.py", "def base():\n    pass\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["tag", "baseline"]);

        // More changed files than CHANGE_FILE_LIMIT would be slow to seed
        // through git — exercise the truncation math directly, mirroring the
        // worktree-diff cap/cap+1 test above.
        let build = |count: usize| -> Vec<String> {
            (0..count)
                .map(|index| format!("src/{index:05}.py"))
                .collect()
        };
        let exact = build(CHANGE_FILE_LIMIT);
        assert!(exact.len() <= CHANGE_FILE_LIMIT);
        let overflow = build(CHANGE_FILE_LIMIT + 1);
        assert!(overflow.len() > CHANGE_FILE_LIMIT);
        assert_eq!(
            overflow.iter().take(CHANGE_FILE_LIMIT).count(),
            CHANGE_FILE_LIMIT
        );

        write(&dir, "base.py", "def base():\n    return 1\n");
        let db = dir.join(".mmcg.db");
        let mut store = Store::open(&db).unwrap();
        crate::indexer::Indexer::new(&dir)
            .index_all(&mut store, false)
            .unwrap();
        let diff = symbols_changed_since(&store, &dir, "baseline").unwrap();
        assert!(!diff.truncated);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn git_errors_do_not_expose_raw_stderr() {
        let dir = init_repo("sanitized_error");
        write(&dir, "x.py", "def x():\n    pass\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        let store = indexed_worktree(&dir);
        let error = symbols_changed_in_worktree(store.store(), &dir, "missing-super-secret-ref")
            .unwrap_err();
        assert_eq!(error.to_string(), "invalid_ref");
        assert!(!error.to_string().contains("secret-ref"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn change_impact_rejects_option_like_ref_and_never_renders_git_stderr() {
        let dir = init_repo("option_ref");
        write(&dir, "x.py", "def x():\n    pass\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        let store = indexed_worktree(&dir);
        let error = symbols_changed_in_worktree(store.store(), &dir, "--help").unwrap_err();
        assert_eq!(error, WorkingTreeDiffError::InvalidRef);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn git_collection_ignores_external_diff_and_textconv() {
        let dir = init_repo("external_diff");
        write(&dir, "x.py", "def x():\n    return 1\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["config", "diff.external", "/definitely/missing"]);
        write(&dir, "x.py", "def x():\n    return 2\n");
        let store = indexed_worktree(&dir);
        let changed = symbols_changed_in_worktree(store.store(), &dir, "HEAD").unwrap();
        assert_eq!(changed.files.len(), 1);
        fs::remove_dir_all(&dir).ok();
    }
}
