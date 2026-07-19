//! Multi-language code indexer.
//!
//! Each language is a [`LanguageExtractor`] in its own submodule. Dispatch here
//! walks the file tree, picks an extractor per extension, parses with tree-sitter
//! in parallel via rayon, and serializes writes through one SQLite connection.

use crate::store::{PendingFile, PendingSymbol, Store};
use ignore::{IncrementalIgnore, WalkBuilder};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tree_sitter::{Parser, Tree};

mod cpp;
mod csharp;
mod go;
mod java;
mod javascript;
mod php;
mod python;
mod rust_lang;
mod typescript;

pub use cpp::CppExtractor;
pub use csharp::CsharpExtractor;
pub use go::GoExtractor;
pub use java::JavaExtractor;
pub use javascript::JavascriptExtractor;
pub use php::PhpExtractor;
pub use python::PythonExtractor;
pub use rust_lang::RustExtractor;
pub use typescript::TypescriptExtractor;

/// Semantic contract for the extractor output stored in SQLite. Bump this when
/// an extractor or grammar change can alter symbols, edges, ownership, or paths
/// without requiring a database schema migration.
pub const EXTRACTOR_CONTRACT_VERSION: &str = "mmcg-extractors-v1";
pub const EXTRACTOR_CONTRACT_META_KEY: &str = "extractor_contract_version";

/// Hard bound for source reads. Generated artifacts with a source-looking
/// extension must not turn indexing into an unbounded allocation.
pub const MAX_INDEXABLE_FILE_SIZE: u64 = 5 * 1024 * 1024;
const BINARY_SNIFF_BYTES: u64 = 8 * 1024;
const MAX_HISTORY_ARTIFACT_SIZE: u64 = 1024 * 1024;
const MAX_HISTORY_ENTRIES: usize = 5_000;

/// Parsed files waiting for the single SQLite writer at once. This bounds peak
/// memory without changing the existing parallel-parse/single-writer model.
pub const PARSE_BATCH_SIZE: usize = 64;

/// Per-language symbol/edge extractor.
///
/// Implementors receive a parsed tree plus source bytes and append symbols/edges
/// to a [`PendingFile`]. The synthetic module symbol is added at `module_index`
/// before `extract` runs — use it as parent for top-level calls and source of
/// import edges.
pub trait LanguageExtractor: Send + Sync {
    fn language(&self) -> tree_sitter::Language;
    fn name(&self) -> &'static str;
    fn extract(&self, tree: &Tree, source: &[u8], pending: &mut PendingFile, module_index: usize);
}

/// Map a file extension to a language extractor.
pub fn extractor_for_path(path: &Path) -> Option<Box<dyn LanguageExtractor>> {
    let ext = path.extension().and_then(|e| e.to_str())?;
    match ext {
        "py" => Some(Box::new(PythonExtractor)),
        "ts" | "tsx" => Some(Box::new(TypescriptExtractor::new(ext == "tsx"))),
        "js" | "jsx" | "mjs" | "cjs" => Some(Box::new(JavascriptExtractor)),
        "rs" => Some(Box::new(RustExtractor)),
        "cs" => Some(Box::new(CsharpExtractor)),
        "go" => Some(Box::new(GoExtractor)),
        "java" => Some(Box::new(JavaExtractor)),
        "php" | "phtml" => Some(Box::new(PhpExtractor)),
        // C / C++ share one tree-sitter-cpp grammar — OK since C is mostly a C++
        // subset; rare C-only keyword identifiers (e.g. `new` as a var) may
        // mis-parse — see cpp.rs precision disclaimer.
        "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hh" | "hxx" | "ipp" | "tpp" => {
            Some(Box::new(CppExtractor))
        }
        _ => None,
    }
}

/// Directory names skipped during the walk.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".mastermind",
    ".venv",
    "venv",
    "__pycache__",
    "node_modules",
    "target",
    "dist",
    "build",
    ".tox",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".next",
    ".turbo",
    ".cache",
];

#[derive(Debug, Default, Clone)]
pub struct IndexStats {
    /// Files seen during the walk (including ones with no extractor).
    pub files_scanned: u32,
    /// Files parsed and committed (re-indexed this run).
    pub files_indexed: u32,
    /// Files that failed to parse or commit.
    pub files_failed: u32,
    /// Unsupported extensions — skipped before parsing.
    pub files_skipped: u32,
    /// Supported files rejected because their content appears binary.
    pub files_skipped_binary: u32,
    /// Supported files rejected because they exceed [`MAX_INDEXABLE_FILE_SIZE`].
    pub files_skipped_too_large: u32,
    /// Indexable files whose stored mtime is current — skipped without parsing.
    pub files_unchanged: u32,
    /// In the index but gone from disk — purged.
    pub files_purged: u32,
    pub symbols_total: u32,
    pub edges_total: u32,
    pub by_language: std::collections::BTreeMap<String, u32>,
    /// Count of `.mastermind/tasks/<NNN>-<name>/spec.md` files added to the FTS5
    /// corpus. Zero when the directory doesn't exist (no `mastermind init`).
    pub task_specs_indexed: u32,
    /// Durable Markdown artifacts in the rebuildable project-history corpus.
    pub history_entries_indexed: u32,
    /// Existing history artifacts rejected by admission checks or unreadable.
    pub history_entries_skipped: u32,
    /// True when the 5,000-artifact work limit omitted candidate files.
    pub history_entries_truncated: bool,
    /// True when this run rebuilt an index made by an older extractor contract.
    pub extractor_contract_rebuilt: bool,
    pub duration_ms: u128,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ProjectHistoryIndexStats {
    pub indexed: u32,
    pub skipped: u32,
    pub truncated: bool,
}

pub struct Indexer {
    root: PathBuf,
}

impl Indexer {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Index everything reachable from `root`. Incremental by default — files whose
    /// filesystem mtime is `<=` stored mtime are skipped. `force_full=true` re-indexes
    /// regardless of mtime (e.g. after a schema change or to recover a corrupted index).
    ///
    /// Files in the index but gone from disk are purged at the end. Writes to `store`.
    pub fn index_all(&self, store: &mut Store, force_full: bool) -> Result<IndexStats, IndexError> {
        let start = SystemTime::now();
        let stored_extractor_contract = store
            .meta_value(EXTRACTOR_CONTRACT_META_KEY)
            .map_err(|error| IndexError::Other(error.to_string()))?;
        let extractor_contract_current =
            stored_extractor_contract.as_deref() == Some(EXTRACTOR_CONTRACT_VERSION);
        let extractor_contract_rebuild_required = !extractor_contract_current
            && store
                .file_count()
                .map_err(|error| IndexError::Other(error.to_string()))?
                > 0;
        let force_full = force_full || !extractor_contract_current;

        // Phase 1: walk filesystem
        let candidates = source_candidates(&self.root);

        let mut stats = IndexStats {
            files_scanned: candidates.len() as u32,
            ..Default::default()
        };

        // Phase 2: classify candidates serially (cheap — one lookup per file).
        // Builds (to_parse, current_paths) for phases 3-5.
        let mut to_parse: Vec<PathBuf> = Vec::new();
        let mut current_paths: std::collections::HashSet<String> =
            std::collections::HashSet::with_capacity(candidates.len());

        for path in &candidates {
            if extractor_for_path(path).is_none() {
                stats.files_skipped += 1;
                continue;
            }
            match source_admission(path) {
                Ok(()) => {}
                Err(IndexError::Skipped(IndexSkipReason::Binary)) => {
                    stats.files_skipped_binary += 1;
                    continue;
                }
                Err(IndexError::Skipped(IndexSkipReason::TooLarge { .. })) => {
                    stats.files_skipped_too_large += 1;
                    continue;
                }
                Err(_) => {
                    stats.files_failed += 1;
                    continue;
                }
            }
            let rel = path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            current_paths.insert(rel.clone());

            if !force_full {
                let fs_mtime = path
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64);

                if let (Some(fs_mt), Ok(Some(stored_mt))) = (fs_mtime, store.file_mtime(&rel)) {
                    if fs_mt <= stored_mt {
                        stats.files_unchanged += 1;
                        continue;
                    }
                }
            }

            to_parse.push(path.clone());
        }

        // Phases 3-4: parse a bounded batch in parallel, then commit it through
        // SQLite's single writer before parsing the next batch. The old all-at-once
        // collection retained every PendingFile until parsing the whole repository.
        for batch in to_parse.chunks(PARSE_BATCH_SIZE) {
            let parsed: Vec<Result<PendingFile, IndexError>> = batch
                .par_iter()
                .filter_map(|path| {
                    let extractor = extractor_for_path(path)?;
                    Some(parse_one(path, &self.root, extractor.as_ref()))
                })
                .collect();

            for outcome in parsed {
                match outcome {
                    Ok(pending) => {
                        stats.symbols_total += pending.symbols.len() as u32;
                        stats.edges_total += pending.edges.len() as u32;
                        if let Some(lang) = guess_language_for(&pending.path) {
                            *stats.by_language.entry(lang.to_string()).or_insert(0) += 1;
                        }
                        match store.commit_file(pending) {
                            Ok(()) => stats.files_indexed += 1,
                            Err(_) => stats.files_failed += 1,
                        }
                    }
                    Err(IndexError::Skipped(IndexSkipReason::Binary)) => {
                        stats.files_skipped_binary += 1;
                    }
                    Err(IndexError::Skipped(IndexSkipReason::TooLarge { .. })) => {
                        stats.files_skipped_too_large += 1;
                    }
                    Err(_) => stats.files_failed += 1,
                }
            }
        }

        // Phase 5: purge orphans (in index, no longer on disk). Safe only after a
        // FULL root scan — a partial scan would wrongly purge the unscanned subtree.
        if let Ok(indexed) = store.indexed_paths() {
            for path in indexed {
                if !current_paths.contains(&path) && store.purge_file(&path).is_ok() {
                    stats.files_purged += 1;
                }
            }
        }

        // Phase 6: refresh the task-spec FTS5 corpus. Scan
        // `.mastermind/tasks/<NNN>-<name>/spec.md` (each task its own folder;
        // top-level `_*.md` are shared assets and bare `*.md` is legacy 0.6.x
        // layout, both excluded). Whole-corpus replace — spec sets are small
        // (<100 files), so atomic replace beats delta tracking and avoids stale
        // entries on rename/delete.
        if let Ok(count) = self.index_task_specs(store) {
            stats.task_specs_indexed = count;
        }
        let history = self.index_project_history(store)?;
        stats.history_entries_indexed = history.indexed;
        stats.history_entries_skipped = history.skipped;
        stats.history_entries_truncated = history.truncated;

        let canonical_root = self
            .root
            .canonicalize()
            .map_err(|error| IndexError::Io(error.to_string()))?;
        store
            .set_meta("index_root", &canonical_root.to_string_lossy())
            .map_err(|error| IndexError::Other(error.to_string()))?;
        if stats.files_failed == 0 {
            store
                .set_meta(EXTRACTOR_CONTRACT_META_KEY, EXTRACTOR_CONTRACT_VERSION)
                .map_err(|error| IndexError::Other(error.to_string()))?;
            stats.extractor_contract_rebuilt = extractor_contract_rebuild_required;
        }

        stats.duration_ms = start.elapsed().map(|d| d.as_millis()).unwrap_or(0);
        Ok(stats)
    }

    /// Scan `.mastermind/tasks/<NNN>-<name>/spec.md` and replace the FTS5 corpus.
    /// Silent no-op when the directory is absent (no `mastermind init`). Returns
    /// the count of indexed specs.
    ///
    /// Layout (since 0.7.0): each task is a folder holding `spec.md` plus related
    /// artifacts (audit notes, screenshots, prior versions). Top-level
    /// `_`-prefixed files (e.g. `_lessons.md`) are shared assets, excluded from
    /// search.
    pub fn index_task_specs(&self, store: &mut Store) -> Result<u32, IndexError> {
        let tasks_dir = self.root.join(".mastermind").join("tasks");
        if !tasks_dir.is_dir() {
            // No `.mastermind/tasks/` — clear any stale entries from a prior run too.
            store
                .replace_task_specs(&[])
                .map_err(|e| IndexError::Other(e.to_string()))?;
            return Ok(0);
        }

        let mut entries: Vec<crate::store::TaskSpecEntry> = Vec::new();
        for dirent in std::fs::read_dir(&tasks_dir).map_err(|e| IndexError::Io(e.to_string()))? {
            let dirent = dirent.map_err(|e| IndexError::Io(e.to_string()))?;
            let path = dirent.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // Per-task folders only. Bare top-level `.md` (legacy 0.6.x) and
            // `_`-prefixed names (shared assets, private scratch) are excluded.
            if !path.is_dir() || name.starts_with('_') || name.starts_with('.') {
                continue;
            }
            let spec_path = path.join("spec.md");
            let Ok(body) = std::fs::read_to_string(&spec_path) else {
                continue;
            };
            let title = extract_spec_title(&body, name);
            let rel = spec_path
                .strip_prefix(&self.root)
                .unwrap_or(&spec_path)
                .to_string_lossy()
                .replace('\\', "/");
            entries.push(crate::store::TaskSpecEntry {
                path: rel,
                title,
                body,
            });
        }
        let count = entries.len() as u32;
        store
            .replace_task_specs(&entries)
            .map_err(|e| IndexError::Other(e.to_string()))?;
        Ok(count)
    }

    /// Rebuild the derived search corpus for durable project-history artifacts.
    /// Only known workflow files are admitted; arbitrary task scratch files and
    /// symlinks are excluded. The Markdown files remain the source of truth.
    pub fn index_project_history(
        &self,
        store: &mut Store,
    ) -> Result<ProjectHistoryIndexStats, IndexError> {
        let mut candidates: Vec<(PathBuf, &'static str)> = Vec::new();
        let mut add_if_present = |path: PathBuf, kind: &'static str| {
            if std::fs::symlink_metadata(&path).is_ok() {
                candidates.push((path, kind));
            }
        };
        add_if_present(self.root.join("CONTEXT.md"), "context");
        if let Ok(entries) = std::fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                if name.starts_with("CONTEXT-archive-") && name.ends_with(".md") {
                    add_if_present(path, "context");
                }
            }
        }

        let tasks_dir = self.root.join(".mastermind").join("tasks");
        add_if_present(tasks_dir.join("_lessons.md"), "lesson");
        if tasks_dir.is_dir() {
            for dirent in
                std::fs::read_dir(&tasks_dir).map_err(|error| IndexError::Io(error.to_string()))?
            {
                let dirent = dirent.map_err(|error| IndexError::Io(error.to_string()))?;
                let path = dirent.path();
                let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                if !metadata.file_type().is_dir()
                    || metadata.file_type().is_symlink()
                    || name.starts_with('_')
                    || name.starts_with('.')
                {
                    continue;
                }
                add_if_present(path.join("spec.md"), "task_spec");
                add_if_present(path.join("executor-report.md"), "executor_report");
                add_if_present(path.join("audit.md"), "audit");
                // Pre-0.39 tasks could keep release notes beside the spec.
                add_if_present(path.join("release-notes.md"), "release_notes");
            }
        }
        let releases_dir = self.root.join(".mastermind").join("releases");
        if let Ok(entries) = std::fs::read_dir(&releases_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_markdown = path.extension().and_then(|value| value.to_str()) == Some("md");
                if is_markdown {
                    add_if_present(path, "release_notes");
                }
            }
        }
        candidates.sort_by(|left, right| left.0.cmp(&right.0));
        candidates.dedup_by(|left, right| left.0 == right.0);
        let truncated = candidates.len() > MAX_HISTORY_ENTRIES;
        candidates.truncate(MAX_HISTORY_ENTRIES);

        let mut entries = Vec::new();
        let mut skipped = 0_u32;
        for (path, kind) in candidates {
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                skipped += 1;
                continue;
            };
            if !metadata.file_type().is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > MAX_HISTORY_ARTIFACT_SIZE
            {
                skipped += 1;
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&path) else {
                skipped += 1;
                continue;
            };
            let fallback = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or(kind);
            let title = extract_spec_title(&body, fallback);
            let rel = path
                .strip_prefix(&self.root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            entries.push(crate::store::ProjectHistoryEntry {
                path: rel,
                kind: kind.to_string(),
                title,
                body,
            });
        }
        let count = entries.len() as u32;
        store
            .replace_project_history(&entries)
            .map_err(|error| IndexError::Other(error.to_string()))?;
        store
            .set_meta("project_history_skipped", &skipped.to_string())
            .map_err(|error| IndexError::Other(error.to_string()))?;
        store
            .set_meta(
                "project_history_truncated",
                if truncated { "true" } else { "false" },
            )
            .map_err(|error| IndexError::Other(error.to_string()))?;
        Ok(ProjectHistoryIndexStats {
            indexed: count,
            skipped,
            truncated,
        })
    }

    /// Re-index a single file. Used by the watcher.
    pub fn index_one(&self, store: &mut Store, path: &Path) -> Result<(), IndexError> {
        let extractor = extractor_for_path(path)
            .ok_or_else(|| IndexError::Parse(format!("no extractor for {path:?}")))?;
        let pending = parse_one(path, &self.root, extractor.as_ref())?;
        store
            .commit_file(pending)
            .map_err(|e| IndexError::Other(e.to_string()))
    }
}

pub(crate) fn is_skipped_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

fn source_walk_builder(root: &Path) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .require_git(false)
        .filter_entry(|entry| !is_skipped_dir(entry.file_name().to_str().unwrap_or("")));
    builder
}

/// All non-ignored files below `root`, in deterministic path order. Language
/// filtering and content admission happen separately so stats stay truthful.
pub(crate) fn source_candidates(root: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = source_walk_builder(root)
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(|entry| entry.into_path())
        .collect();
    paths.sort();
    paths
}

/// Stateful ignore matcher for watcher events. It applies the same gitignore
/// and global-ignore rules as the initial walk, including nested `.gitignore`s.
pub(crate) struct SourceMatcher {
    root: PathBuf,
    matcher: IncrementalIgnore,
}

impl SourceMatcher {
    pub(crate) fn new(root: &Path) -> Self {
        let matcher = source_walk_builder(root)
            .build_matchers()
            .into_iter()
            .next()
            .expect("one ignore matcher for one source root");
        Self {
            root: root.to_path_buf(),
            matcher,
        }
    }

    pub(crate) fn is_ignored(&mut self, path: &Path, is_dir: bool) -> bool {
        let Some(relative) = path
            .strip_prefix(&self.root)
            .ok()
            .map(Path::to_path_buf)
            .or_else(|| self.matcher.normalize(path))
        else {
            return true;
        };
        if relative
            .components()
            .any(|component| is_skipped_dir(component.as_os_str().to_str().unwrap_or("")))
        {
            return true;
        }
        self.matcher.matched(relative, is_dir).is_ignore()
    }
}

/// First `# Heading` line of a markdown spec — falls back to the filename
/// (minus extension) if no heading exists.
fn extract_spec_title(body: &str, filename: &str) -> String {
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let t = rest.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    filename.trim_end_matches(".md").to_string()
}

fn guess_language_for(rel_path: &str) -> Option<&'static str> {
    if rel_path.ends_with(".py") {
        Some("python")
    } else if rel_path.ends_with(".tsx") {
        Some("tsx")
    } else if rel_path.ends_with(".ts") {
        Some("typescript")
    } else if rel_path.ends_with(".jsx")
        || rel_path.ends_with(".js")
        || rel_path.ends_with(".mjs")
        || rel_path.ends_with(".cjs")
    {
        // `.jsx` is a JavaScript dialect — store as "javascript", not a distinct
        // "jsx". The MCP `language` enum and `lang_from_ext` already treat it as
        // javascript; "jsx" made `.jsx` symbols invisible to every
        // `language: "javascript"` filter.
        Some("javascript")
    } else if rel_path.ends_with(".rs") {
        Some("rust")
    } else if rel_path.ends_with(".cs") {
        Some("csharp")
    } else if rel_path.ends_with(".go") {
        Some("go")
    } else if rel_path.ends_with(".java") {
        Some("java")
    } else if rel_path.ends_with(".php") || rel_path.ends_with(".phtml") {
        Some("php")
    } else if rel_path.ends_with(".c")
        || rel_path.ends_with(".cc")
        || rel_path.ends_with(".cpp")
        || rel_path.ends_with(".cxx")
        || rel_path.ends_with(".h")
        || rel_path.ends_with(".hpp")
        || rel_path.ends_with(".hh")
        || rel_path.ends_with(".hxx")
        || rel_path.ends_with(".ipp")
        || rel_path.ends_with(".tpp")
    {
        Some("cpp")
    } else {
        None
    }
}

pub(crate) fn parse_one(
    path: &Path,
    root: &Path,
    extractor: &dyn LanguageExtractor,
) -> Result<PendingFile, IndexError> {
    let source = read_source_bounded(path)?;
    // Milliseconds — second-precision missed edits within the same second as the
    // previous index run. i64 millis covers ~292M years.
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map_err(|e| IndexError::Io(e.to_string()))?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");

    parse_blob(&rel, &source, mtime, extractor)
}

pub(crate) fn source_admission(path: &Path) -> Result<(), IndexError> {
    let metadata = path
        .metadata()
        .map_err(|error| IndexError::Io(error.to_string()))?;
    if metadata.len() > MAX_INDEXABLE_FILE_SIZE {
        return Err(IndexError::Skipped(IndexSkipReason::TooLarge {
            size: metadata.len(),
        }));
    }
    let mut file = std::fs::File::open(path).map_err(|error| IndexError::Io(error.to_string()))?;
    let mut prefix = Vec::with_capacity(BINARY_SNIFF_BYTES as usize);
    file.by_ref()
        .take(BINARY_SNIFF_BYTES)
        .read_to_end(&mut prefix)
        .map_err(|error| IndexError::Io(error.to_string()))?;
    if is_binary_content(&prefix) {
        return Err(IndexError::Skipped(IndexSkipReason::Binary));
    }
    Ok(())
}

fn read_source_bounded(path: &Path) -> Result<Vec<u8>, IndexError> {
    let metadata = path
        .metadata()
        .map_err(|error| IndexError::Io(error.to_string()))?;
    if metadata.len() > MAX_INDEXABLE_FILE_SIZE {
        return Err(IndexError::Skipped(IndexSkipReason::TooLarge {
            size: metadata.len(),
        }));
    }

    let mut file = std::fs::File::open(path).map_err(|error| IndexError::Io(error.to_string()))?;
    let mut source = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(MAX_INDEXABLE_FILE_SIZE + 1)
        .read_to_end(&mut source)
        .map_err(|error| IndexError::Io(error.to_string()))?;
    if source.len() as u64 > MAX_INDEXABLE_FILE_SIZE {
        return Err(IndexError::Skipped(IndexSkipReason::TooLarge {
            size: source.len() as u64,
        }));
    }
    if is_binary_content(&source[..source.len().min(BINARY_SNIFF_BYTES as usize)]) {
        return Err(IndexError::Skipped(IndexSkipReason::Binary));
    }
    Ok(source)
}

pub(crate) fn is_binary_content(content: &[u8]) -> bool {
    content.contains(&0)
}

/// Parse a file's bytes WITHOUT touching the filesystem. Used by
/// `mmcg_symbols_changed_since`, which needs the symbol set at an old git ref —
/// bytes come from `git show <ref>:<path>`, not disk.
///
/// `mtime` is caller-supplied; for git-blob parsing pass `0` (result is
/// transient, never stored). `rel_path` is relative to the project root and
/// picks the extractor via its extension.
pub(crate) fn parse_blob(
    rel_path: &str,
    source: &[u8],
    mtime: i64,
    extractor: &dyn LanguageExtractor,
) -> Result<PendingFile, IndexError> {
    let mut parser = Parser::new();
    let language = extractor.language();
    parser
        .set_language(&language)
        .map_err(|e| IndexError::Parse(e.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| IndexError::Parse("tree-sitter parse returned None".to_string()))?;

    let line_count = source.iter().filter(|&&b| b == b'\n').count() as u32 + 1;
    let language = guess_language_for(rel_path).unwrap_or("").to_string();
    let mut pending = PendingFile {
        path: rel_path.to_string(),
        mtime,
        content_sha256: format!("{:x}", Sha256::digest(source)),
        language,
        symbols: Vec::new(),
        edges: Vec::new(),
    };

    // Synthetic module symbol — owns top-level imports and module-scope calls.
    pending.symbols.push(PendingSymbol {
        name: "<module>".to_string(),
        kind: "module".to_string(),
        line_start: 1,
        line_end: line_count,
        signature: Some(format!("module {rel_path}")),
        parent_index: None,
        decorators: None,
    });
    let module_index = pending.symbols.len() - 1;

    extractor.extract(&tree, source, &mut pending, module_index);
    Ok(pending)
}

#[derive(Debug)]
pub enum IndexError {
    Io(String),
    Parse(String),
    Other(String),
    Skipped(IndexSkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexSkipReason {
    Binary,
    TooLarge { size: u64 },
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexError::Io(m) => write!(f, "io: {m}"),
            IndexError::Parse(m) => write!(f, "parse: {m}"),
            IndexError::Other(m) => write!(f, "other: {m}"),
            IndexError::Skipped(IndexSkipReason::Binary) => write!(f, "skipped binary source"),
            IndexError::Skipped(IndexSkipReason::TooLarge { size }) => write!(
                f,
                "skipped source with {size} bytes (limit {MAX_INDEXABLE_FILE_SIZE})"
            ),
        }
    }
}

impl std::error::Error for IndexError {}

#[cfg(test)]
mod incremental_tests {
    use super::*;
    use crate::store::Store;
    use std::env;
    use std::fs;
    use std::time::{Duration, SystemTime};

    fn setup(name: &str) -> (PathBuf, PathBuf) {
        let dir = env::temp_dir().join(format!("mmcg-incr-{}-{name}", std::process::id()));
        if dir.exists() {
            fs::remove_dir_all(&dir).ok();
        }
        fs::create_dir_all(&dir).unwrap();
        let db = dir.join("idx.db");
        (dir, db)
    }

    #[test]
    fn incremental_skips_unchanged_files() {
        let (dir, db) = setup("skips_unchanged");
        fs::write(dir.join("a.py"), "def foo(): pass\n").unwrap();
        fs::write(dir.join("b.py"), "def bar(): pass\n").unwrap();

        let mut store = Store::open(&db).unwrap();
        let indexer = Indexer::new(&dir);

        let first = indexer.index_all(&mut store, false).unwrap();
        assert_eq!(first.files_indexed, 2, "first run should index both files");
        assert_eq!(first.files_unchanged, 0);
        assert!(!first.extractor_contract_rebuilt);

        let second = indexer.index_all(&mut store, false).unwrap();
        assert_eq!(second.files_indexed, 0, "no changes → nothing re-indexed");
        assert_eq!(
            second.files_unchanged, 2,
            "both files should be marked unchanged"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn force_reindexes_everything() {
        let (dir, db) = setup("force_reindex");
        fs::write(dir.join("a.py"), "def foo(): pass\n").unwrap();

        let mut store = Store::open(&db).unwrap();
        let indexer = Indexer::new(&dir);

        indexer.index_all(&mut store, false).unwrap();
        let forced = indexer.index_all(&mut store, true).unwrap();
        assert_eq!(forced.files_indexed, 1, "force should re-parse");
        assert_eq!(forced.files_unchanged, 0);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn touching_a_file_triggers_reindex() {
        let (dir, db) = setup("touch_reindex");
        let a = dir.join("a.py");
        let b = dir.join("b.py");
        fs::write(&a, "def foo(): pass\n").unwrap();
        fs::write(&b, "def bar(): pass\n").unwrap();

        let mut store = Store::open(&db).unwrap();
        let indexer = Indexer::new(&dir);
        indexer.index_all(&mut store, false).unwrap();

        // Bump a.py mtime 10s into the future — bypasses second-resolution issues.
        let f = fs::File::options().write(true).open(&a).unwrap();
        f.set_modified(SystemTime::now() + Duration::from_secs(10))
            .unwrap();

        let stats = indexer.index_all(&mut store, false).unwrap();
        assert_eq!(stats.files_indexed, 1, "only a.py should re-index");
        assert_eq!(stats.files_unchanged, 1, "b.py should be unchanged");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn deleted_files_get_purged() {
        let (dir, db) = setup("delete_purge");
        let a = dir.join("a.py");
        let b = dir.join("b.py");
        fs::write(&a, "def foo(): pass\n").unwrap();
        fs::write(&b, "def bar(): pass\n").unwrap();

        let mut store = Store::open(&db).unwrap();
        let indexer = Indexer::new(&dir);
        indexer.index_all(&mut store, false).unwrap();

        fs::remove_file(&b).unwrap();

        let stats = indexer.index_all(&mut store, false).unwrap();
        assert_eq!(stats.files_purged, 1, "b.py should be purged from index");
        assert_eq!(stats.files_unchanged, 1, "a.py should be unchanged");

        let remaining: Vec<String> = store.indexed_paths().unwrap();
        assert!(!remaining.iter().any(|p| p.ends_with("b.py")));
        assert!(remaining.iter().any(|p| p.ends_with("a.py")));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn task_specs_indexed_from_mastermind_dir() {
        let (dir, db) = setup("task_specs");
        // No `.mastermind/tasks/` yet — first run reports 0 specs.
        let indexer = Indexer::new(&dir);
        let mut store = Store::open(&db).unwrap();
        let stats1 = indexer.index_all(&mut store, false).unwrap();
        assert_eq!(stats1.task_specs_indexed, 0);

        // Two task folders + one shared template (underscore prefix excluded).
        // Layout: .mastermind/tasks/<NNN>-<name>/spec.md
        let tasks_dir = dir.join(".mastermind").join("tasks");
        let spec_a = tasks_dir.join("001-rate-limiter");
        let spec_b = tasks_dir.join("002-cache-eviction");
        fs::create_dir_all(&spec_a).unwrap();
        fs::create_dir_all(&spec_b).unwrap();
        fs::write(
            spec_a.join("spec.md"),
            "# Add per-tenant rate limiting\n\nUse token bucket with Redis.\n",
        )
        .unwrap();
        fs::write(
            spec_b.join("spec.md"),
            "# Cache eviction strategy\n\nLRU with TTL on user records.\n",
        )
        .unwrap();
        fs::write(
            tasks_dir.join("_lessons.md"),
            "# Lessons — should not appear in search\n\nGeneric scaffold.\n",
        )
        .unwrap();
        // A bare top-level `.md` (legacy 0.6.x layout) must be ignored.
        fs::write(
            tasks_dir.join("099-legacy-flat.md"),
            "# Legacy flat spec\n\nShould be skipped — needs migration.\n",
        )
        .unwrap();

        let stats2 = indexer.index_all(&mut store, false).unwrap();
        assert_eq!(
            stats2.task_specs_indexed, 2,
            "underscore + legacy flat excluded"
        );

        // FTS5 query proves body content is searchable.
        let hits = store.search_task_specs("token bucket", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.contains("001-rate-limiter/spec.md"));

        // Lessons file excluded — its unique phrase finds nothing.
        let empty = store.search_task_specs("scaffold", 10).unwrap();
        assert!(empty.is_empty());

        // Legacy flat file is also excluded.
        let legacy = store.search_task_specs("legacy flat", 10).unwrap();
        assert!(legacy.is_empty());

        // Remove one spec folder — next index pass wipes it from the corpus.
        fs::remove_dir_all(&spec_b).unwrap();
        let stats3 = indexer.index_all(&mut store, false).unwrap();
        assert_eq!(stats3.task_specs_indexed, 1);
        let gone = store.search_task_specs("LRU TTL", 10).unwrap();
        assert!(gone.is_empty());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn project_history_indexes_only_durable_workflow_artifacts() {
        let (dir, db) = setup("project_history");
        let task_dir = dir.join(".mastermind/tasks/001-auth-boundary");
        fs::create_dir_all(&task_dir).unwrap();
        fs::write(
            dir.join("CONTEXT.md"),
            "# Context\n\nDecision: auth is enforced at admission.\n",
        )
        .unwrap();
        fs::write(
            dir.join("CONTEXT-archive-2025.md"),
            "# Archived context\n\nThe legacy gateway owned token exchange.\n",
        )
        .unwrap();
        fs::write(
            dir.join(".mastermind/tasks/_lessons.md"),
            "# Lessons\n\nA middleware-only guard was bypassed.\n",
        )
        .unwrap();
        fs::write(
            task_dir.join("spec.md"),
            "# Harden admission\n\nEnforce authorization before reads.\n",
        )
        .unwrap();
        fs::write(
            task_dir.join("executor-report.md"),
            "# Executor report\n\nAuthorization gate wired to raw admission.\n",
        )
        .unwrap();
        fs::write(
            task_dir.join("audit.md"),
            "# Audit\n\nVerified the runtime boundary.\n",
        )
        .unwrap();
        let releases_dir = dir.join(".mastermind/releases");
        fs::create_dir_all(&releases_dir).unwrap();
        fs::write(
            releases_dir.join("001-auth-boundary.md"),
            "# Release\n\nAdmission authorization is now enforced.\n",
        )
        .unwrap();
        fs::write(
            task_dir.join("release-notes.md"),
            vec![b'x'; (MAX_HISTORY_ARTIFACT_SIZE + 1) as usize],
        )
        .unwrap();
        fs::write(task_dir.join("notes.md"), "private scratch phrase\n").unwrap();

        let mut store = Store::open(&db).unwrap();
        let indexer = Indexer::new(&dir);
        let stats = indexer.index_all(&mut store, false).unwrap();
        assert_eq!(stats.history_entries_indexed, 7);
        assert_eq!(stats.history_entries_skipped, 1);
        assert!(!stats.history_entries_truncated);
        assert_eq!(store.project_history_count().unwrap(), 7);
        let lesson = store
            .search_project_history("middleware bypassed", Some("lesson"), 10)
            .unwrap();
        assert_eq!(lesson.len(), 1);
        assert!(store
            .search_project_history("private scratch", None, 10)
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .search_project_history("legacy gateway", Some("context"), 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .search_project_history("Admission authorization", Some("release_notes"), 10)
                .unwrap()
                .len(),
            1
        );

        fs::remove_file(task_dir.join("audit.md")).unwrap();
        let stats = indexer.index_all(&mut store, false).unwrap();
        assert_eq!(stats.history_entries_indexed, 6);
        assert!(store
            .search_project_history("runtime boundary", Some("audit"), 10)
            .unwrap()
            .is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn jsx_files_indexed_as_javascript() {
        // Regression: `.jsx` stored under language "jsx" — not in the MCP
        // `language` enum, never matches a `language: "javascript"` filter — so
        // `.jsx` defs silently vanished from language-scoped queries (the exact
        // case the filter exists for: monorepo collisions).
        let (dir, db) = setup("jsx_as_js");
        fs::write(
            dir.join("App.jsx"),
            "export function App() { return null; }\n",
        )
        .unwrap();

        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();

        // Must be reachable through a `language: "javascript"` filter.
        let js_files = store.files_under(None, Some("javascript")).unwrap();
        assert!(
            js_files.iter().any(|f| f.path.ends_with("App.jsx")),
            "App.jsx should be found under language=javascript"
        );

        // Nothing should remain under the bogus "jsx" language.
        let jsx_files = store.files_under(None, Some("jsx")).unwrap();
        assert!(
            jsx_files.is_empty(),
            "no file should be stored under language=jsx"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn index_all_stamps_canonical_root_and_content_sha256() {
        let (dir, db) = setup("impact_snapshot_metadata");
        let bytes = b"def value():\n    return 1\n";
        fs::write(dir.join("app.py"), bytes).unwrap();
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, true).unwrap();
        assert_eq!(
            store.meta_value("index_root").unwrap().as_deref(),
            Some(dir.canonicalize().unwrap().to_string_lossy().as_ref())
        );
        assert_eq!(
            store.file_content_sha256("app.py").unwrap().as_deref(),
            Some(format!("{:x}", Sha256::digest(bytes)).as_str())
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extractor_contract_mismatch_forces_a_full_reindex() {
        let (dir, db) = setup("extractor_contract");
        fs::write(dir.join("app.py"), "def current(): pass\n").unwrap();
        let mut store = Store::open(&db).unwrap();
        let indexer = Indexer::new(&dir);

        indexer.index_all(&mut store, false).unwrap();
        store
            .set_meta(EXTRACTOR_CONTRACT_META_KEY, "obsolete-contract")
            .unwrap();

        let stats = indexer.index_all(&mut store, false).unwrap();
        assert_eq!(stats.files_indexed, 1);
        assert_eq!(stats.files_unchanged, 0);
        assert!(stats.extractor_contract_rebuilt);
        assert!(store.extractor_contract_current().unwrap());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn binary_and_oversized_sources_are_rejected_without_failing_the_run() {
        let (dir, db) = setup("source_admission");
        fs::write(dir.join("good.rs"), "pub fn good() {}\n").unwrap();
        fs::write(dir.join("binary.rs"), b"pub fn hidden() {}\0payload").unwrap();
        let large = fs::File::create(dir.join("large.rs")).unwrap();
        large.set_len(MAX_INDEXABLE_FILE_SIZE + 1).unwrap();

        let mut store = Store::open(&db).unwrap();
        let stats = Indexer::new(&dir).index_all(&mut store, false).unwrap();

        assert_eq!(stats.files_indexed, 1);
        assert_eq!(stats.files_skipped_binary, 1);
        assert_eq!(stats.files_skipped_too_large, 1);
        assert_eq!(stats.files_failed, 0);
        assert_eq!(store.indexed_paths().unwrap(), vec!["good.rs"]);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn source_walk_and_watcher_matcher_share_gitignore_rules() {
        let (dir, _db) = setup("gitignore");
        let generated = dir.join("generated/ignored.rs");
        let kept = dir.join("src/kept.rs");
        let nested_ignored = dir.join("nested/ignored.rs");
        let nested_kept = dir.join("nested/kept.rs");
        fs::create_dir_all(generated.parent().unwrap()).unwrap();
        fs::create_dir_all(kept.parent().unwrap()).unwrap();
        fs::create_dir_all(nested_ignored.parent().unwrap()).unwrap();
        fs::write(dir.join(".gitignore"), "generated/\n").unwrap();
        fs::write(dir.join("nested/.gitignore"), "*.rs\n!kept.rs\n").unwrap();
        fs::write(&generated, "pub fn ignored() {}\n").unwrap();
        fs::write(&kept, "pub fn kept() {}\n").unwrap();
        fs::write(&nested_ignored, "pub fn ignored() {}\n").unwrap();
        fs::write(&nested_kept, "pub fn kept() {}\n").unwrap();

        let candidates = source_candidates(&dir);
        assert!(!candidates.contains(&generated));
        assert!(candidates.contains(&kept));
        assert!(!candidates.contains(&nested_ignored));
        assert!(candidates.contains(&nested_kept));

        let mut matcher = SourceMatcher::new(&dir);
        assert!(matcher.is_ignored(&generated, false));
        assert!(!matcher.is_ignored(&kept, false));
        assert!(matcher.is_ignored(&nested_ignored, false));
        assert!(!matcher.is_ignored(&nested_kept, false));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn indexing_more_than_one_parse_batch_commits_every_file() {
        let (dir, db) = setup("bounded_batches");
        for index in 0..=PARSE_BATCH_SIZE {
            fs::write(
                dir.join(format!("source_{index}.rs")),
                format!("pub fn source_{index}() {{}}\n"),
            )
            .unwrap();
        }

        let mut store = Store::open(&db).unwrap();
        let stats = Indexer::new(&dir).index_all(&mut store, false).unwrap();

        assert_eq!(stats.files_indexed as usize, PARSE_BATCH_SIZE + 1);
        assert_eq!(stats.files_failed, 0);
        assert_eq!(store.indexed_paths().unwrap().len(), PARSE_BATCH_SIZE + 1);

        fs::remove_dir_all(&dir).ok();
    }
}

/// Helpers exposed to the per-language submodules.
pub(crate) mod common {
    use crate::store::{PendingEdge, PendingFile, PendingSymbol};
    use tree_sitter::Node;

    pub fn push_def(
        pending: &mut PendingFile,
        name: String,
        kind: &str,
        node: &Node,
        signature: Option<String>,
        parent_index: Option<usize>,
    ) -> usize {
        push_def_with_decorators(pending, name, kind, node, signature, parent_index, None)
    }

    /// Like `push_def` but records decorator/attribute names. `decorators` must be
    /// pre-formatted as `,name1,name2,` (leading/trailing commas) so the
    /// `unreferenced` query can match individual names via `LIKE ',name,'`.
    pub fn push_def_with_decorators(
        pending: &mut PendingFile,
        name: String,
        kind: &str,
        node: &Node,
        signature: Option<String>,
        parent_index: Option<usize>,
        decorators: Option<String>,
    ) -> usize {
        pending.symbols.push(PendingSymbol {
            name,
            kind: kind.to_string(),
            line_start: (node.start_position().row + 1) as u32,
            line_end: (node.end_position().row + 1) as u32,
            signature,
            parent_index,
            decorators,
        });
        pending.symbols.len() - 1
    }

    pub fn push_call(
        pending: &mut PendingFile,
        from_index: usize,
        to_name: String,
        to_path: Option<String>,
        line: u32,
    ) {
        push_call_with_type(pending, from_index, to_name, to_path, None, line)
    }

    /// When the receiver/namespace is a type (e.g. Rust `SessionStore::new()`),
    /// pass it as `to_type` so `mmcg_callers <Type>` finds these sites.
    pub fn push_call_with_type(
        pending: &mut PendingFile,
        from_index: usize,
        to_name: String,
        to_path: Option<String>,
        to_type: Option<String>,
        line: u32,
    ) {
        pending.edges.push(PendingEdge {
            from_index,
            to_name,
            to_path,
            to_type,
            kind: "calls".to_string(),
            line,
        });
    }

    pub fn push_import(
        pending: &mut PendingFile,
        module_index: usize,
        to_name: String,
        to_path: Option<String>,
        line: u32,
    ) {
        pending.edges.push(PendingEdge {
            from_index: module_index,
            to_name,
            to_path,
            to_type: None,
            kind: "imports".to_string(),
            line,
        });
    }

    pub fn node_text<'a>(node: &Node, source: &'a [u8]) -> Option<&'a str> {
        node.utf8_text(source).ok()
    }

    pub fn line_of(node: &Node) -> u32 {
        (node.start_position().row + 1) as u32
    }
}
