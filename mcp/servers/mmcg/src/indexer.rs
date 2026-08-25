//! Multi-language code indexer.
//!
//! Each language is a [`LanguageExtractor`] in its own submodule. Dispatch here
//! walks the file tree, picks an extractor per extension, parses with tree-sitter
//! in parallel via rayon, and serializes writes through one SQLite connection.

use crate::bounded_fs::{
    inspect_path_kind_with_capability, read_directory_names, read_directory_names_with_capability,
    read_regular_file, read_regular_file_expected, read_regular_file_with_capability,
    BoundedPathKind, BoundedReadError, ReadControl, RootCapability, StableFileIdentity,
};
use crate::store::{
    normalize_concept_documentation, PendingConceptCorpus, PendingConceptDocumentation,
    PendingFile, PendingSymbol, Store,
};
use ignore::{IncrementalIgnore, WalkBuilder};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
mod vue;

pub use cpp::CppExtractor;
pub use csharp::CsharpExtractor;
pub use go::GoExtractor;
pub use java::JavaExtractor;
pub use javascript::JavascriptExtractor;
pub use php::PhpExtractor;
pub use python::PythonExtractor;
pub use rust_lang::RustExtractor;
pub use typescript::TypescriptExtractor;
pub use vue::VueExtractor;

/// Semantic contract for the extractor output stored in SQLite. Bump this when
/// an extractor or grammar change can alter symbols, edges, ownership, or paths
/// without requiring a database schema migration.
pub const EXTRACTOR_CONTRACT_VERSION: &str = "mmcg-extractors-v5";
pub const EXTRACTOR_CONTRACT_META_KEY: &str = "extractor_contract_version";

/// Bind a persisted codegraph to the repository it was built from.
///
/// Every indexer run records `index_root`; consumers that combine a `Store`
/// with filesystem or Git state must validate that identity before trusting
/// symbol, caller, or history results. Without this check a perfectly healthy
/// database from another repository can satisfy verification claims.
pub fn validate_index_root(store: &Store, requested_root: &Path) -> Result<(), String> {
    let requested = requested_root
        .canonicalize()
        .map_err(|error| format!("cannot resolve repository root: {error}"))?;
    let stored = store
        .meta_value("index_root")
        .map_err(|error| format!("cannot read index root: {error}"))?
        .ok_or_else(|| "index has no repository identity; run `mastermind index .`".to_string())?;
    let stored = PathBuf::from(stored)
        .canonicalize()
        .map_err(|error| format!("cannot resolve stored index root: {error}"))?;
    if stored != requested {
        return Err(format!(
            "index belongs to `{}`, not `{}`; rebuild it for this repository or pass the correct --index",
            stored.display(),
            requested.display()
        ));
    }
    Ok(())
}

/// Hard bound for source reads. Generated artifacts with a source-looking
/// extension must not turn indexing into an unbounded allocation.
pub const MAX_INDEXABLE_FILE_SIZE: u64 = 5 * 1024 * 1024;
const SKIPPED_PATH_SAMPLE_LIMIT: usize = 20;
const BINARY_SNIFF_BYTES: u64 = 8 * 1024;
const GIT_TRACKED_PATH_OUTPUT_LIMIT: usize = 64 * 1024 * 1024;
const MAX_HISTORY_ARTIFACT_SIZE: u64 = 1024 * 1024;
const MAX_HISTORY_ENTRIES: usize = 5_000;
const MAX_HISTORY_AGGREGATE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_HISTORY_DIRECTORY_ENTRIES: usize = 50_000;
pub const AUTO_REFRESH_SOURCE_CANDIDATE_LIMIT: usize = 20_000;
pub const AUTO_REFRESH_SOURCE_AGGREGATE_BYTES: u64 = 512 * 1024 * 1024;

/// Parsed files waiting for the single SQLite writer at once. This bounds peak
/// memory without changing the existing parallel-parse/single-writer model.
pub const PARSE_BATCH_SIZE: usize = 64;

const CONCEPT_DOCUMENTATION_RAW_MAX_BYTES: usize = 2 * 1024;
const CONCEPT_DOCUMENTATION_FILE_MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RawConceptDocumentation {
    pub symbol_index: usize,
    pub text: String,
    pub size_limited: bool,
}

#[derive(Debug, Default)]
pub(super) struct DocumentationTextBuilder {
    text: String,
    size_limited: bool,
}

impl DocumentationTextBuilder {
    pub fn push_line(&mut self, line: &str) {
        if !self.text.is_empty() {
            self.push_fragment("\n");
        }
        self.push_fragment(line);
    }

    pub fn push_fragment(&mut self, fragment: &str) {
        let remaining = CONCEPT_DOCUMENTATION_RAW_MAX_BYTES.saturating_sub(self.text.len());
        if fragment.len() <= remaining {
            self.text.push_str(fragment);
            return;
        }
        let mut end = remaining;
        while end > 0 && !fragment.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&fragment[..end]);
        self.size_limited = true;
    }

    pub fn finish(self, symbol_index: usize) -> Option<RawConceptDocumentation> {
        (!self.text.trim().is_empty()).then_some(RawConceptDocumentation {
            symbol_index,
            text: self.text,
            size_limited: self.size_limited,
        })
    }
}

#[derive(Debug)]
struct IndexedPendingFile {
    pending: PendingFile,
    documentation: PendingConceptCorpus,
}

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

fn placeholder_documentation_value(value: &str) -> bool {
    let trimmed = value
        .trim()
        .trim_matches(|character: char| {
            matches!(character, '"' | '\'' | '`' | ',' | ';' | ')' | ']' | '}')
        })
        .to_ascii_lowercase();
    if trimmed.is_empty()
        || trimmed.starts_with("${")
        || trimmed.starts_with("{{")
        || trimmed.starts_with('<')
        || trimmed == "example"
        || trimmed.starts_with("example_")
        || trimmed.starts_with("example-")
        || trimmed.starts_with("your_")
        || trimmed.starts_with("your-")
    {
        return true;
    }
    if matches!(
        trimmed.as_str(),
        "placeholder"
            | "redacted"
            | "changeme"
            | "change_me"
            | "change-me"
            | "dummy"
            | "fake"
            | "test"
            | "none"
            | "null"
            | "value"
            | "todo"
            | "secret_here"
            | "token_here"
            | "password_here"
            | "api_key_here"
    ) {
        return true;
    }
    trimmed
        .chars()
        .all(|character| matches!(character, 'x' | '*' | '-' | '_' | '.'))
}

fn assignment_key_like_secret(value: &str) -> bool {
    let trimmed = value
        .trim()
        .trim_start_matches(['-', '*', '+', '>', '`'])
        .trim()
        .trim_matches(|character: char| matches!(character, '"' | '\'' | '`'));
    if trimmed.is_empty() {
        return false;
    }
    let compact = trimmed
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    let has_internal_space = trimmed.chars().any(char::is_whitespace);
    !has_internal_space
        && (compact.ends_with("apikey")
            || compact.ends_with("secret")
            || compact.ends_with("token")
            || compact.ends_with("password"))
}

fn assigned_secret_like(line: &str) -> bool {
    for (index, separator) in line.char_indices() {
        if !matches!(separator, '=' | ':') {
            continue;
        }
        let left = &line[..index];
        let left = if separator == '=' {
            let trimmed = left.trim_end();
            let start = trimmed
                .rfind(|character: char| {
                    character.is_whitespace() || matches!(character, ',' | ';' | '(' | '{')
                })
                .map_or(0, |position| position + 1);
            &trimmed[start..]
        } else {
            left
        };
        let mut right = line[index + separator.len_utf8()..].trim_start();
        if !assignment_key_like_secret(left) {
            continue;
        }
        right = right.trim_start_matches(['"', '\'', '`']);
        let end = right
            .find(|character: char| {
                character.is_whitespace()
                    || matches!(character, '"' | '\'' | '`' | ',' | ';' | ')' | ']' | '}')
            })
            .unwrap_or(right.len());
        if !placeholder_documentation_value(&right[..end]) {
            return true;
        }
    }
    false
}

fn bearer_authorization_like(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let mut search_start = 0usize;
    while let Some(relative) = lower[search_start..].find("authorization") {
        let authorization = search_start + relative;
        let before_ok = authorization == 0
            || !matches!(
                lower.as_bytes()[authorization - 1],
                b'a'..=b'z' | b'0'..=b'9' | b'_'
            );
        let mut rest = lower[authorization + "authorization".len()..].trim_start();
        if matches!(rest.as_bytes().first(), Some(b'"' | b'\'' | b'`')) {
            rest = rest[1..].trim_start();
        }
        if before_ok && (rest.starts_with(':') || rest.starts_with('=')) {
            rest = rest[1..].trim_start().trim_start_matches(['"', '\'', '`']);
            if let Some(value) = rest.strip_prefix("bearer") {
                if value.chars().next().is_some_and(char::is_whitespace) {
                    let value = value.trim_start();
                    let end = value
                        .find(|character: char| {
                            character.is_whitespace()
                                || matches!(
                                    character,
                                    '"' | '\'' | '`' | ',' | ';' | ')' | ']' | '}'
                                )
                        })
                        .unwrap_or(value.len());
                    if !placeholder_documentation_value(&value[..end]) {
                        return true;
                    }
                }
            }
        }
        search_start = authorization + "authorization".len();
    }
    false
}

fn aws_access_key_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20 {
        return false;
    }
    for start in 0..=bytes.len() - 20 {
        if !matches!(&bytes[start..start + 4], b"AKIA" | b"ASIA") {
            continue;
        }
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let end = start + 20;
        let after_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before_ok
            && after_ok
            && bytes[start + 4..end]
                .iter()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

fn secret_like_documentation(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    (lower.contains("-----begin ") && lower.contains("private key-----"))
        || aws_access_key_like(value)
        || value
            .lines()
            .any(|line| bearer_authorization_like(line) || assigned_secret_like(line))
}

fn build_concept_documentation_corpus(
    language: &str,
    mut candidates: Vec<RawConceptDocumentation>,
) -> PendingConceptCorpus {
    let language_supported = crate::store::concept_documentation_language_supported(language);
    let mut corpus = PendingConceptCorpus {
        language_supported,
        ..PendingConceptCorpus::default()
    };
    if !language_supported {
        return corpus;
    }

    candidates.sort_by_key(|candidate| candidate.symbol_index);
    let mut documents = BTreeMap::<usize, String>::new();
    for candidate in candidates {
        if secret_like_documentation(&candidate.text) {
            corpus.secret_omitted = corpus.secret_omitted.saturating_add(1);
            continue;
        }
        let Some(normalized) = normalize_concept_documentation(&candidate.text) else {
            corpus.size_omitted = corpus.size_omitted.saturating_add(1);
            continue;
        };
        let mut size_limited = candidate.size_limited || normalized.truncated;
        if normalized.value.is_empty() {
            if size_limited {
                corpus.size_omitted = corpus.size_omitted.saturating_add(1);
            }
            continue;
        }
        let document = documents.entry(candidate.symbol_index).or_default();
        let mut seen_terms = document
            .split(' ')
            .filter(|term| !term.is_empty())
            .map(str::to_string)
            .collect::<HashSet<_>>();
        for term in normalized.value.split(' ') {
            if !seen_terms.insert(term.to_string()) {
                continue;
            }
            let separator = usize::from(!document.is_empty());
            if document
                .len()
                .saturating_add(separator)
                .saturating_add(term.len())
                > crate::store::CONCEPT_DOCUMENTATION_MAX_BYTES
            {
                size_limited = true;
                break;
            }
            if separator == 1 {
                document.push(' ');
            }
            document.push_str(term);
        }
        if size_limited {
            corpus.size_omitted = corpus.size_omitted.saturating_add(1);
        }
    }

    let mut aggregate_bytes = 0usize;
    for (symbol_index, documentation_search) in documents {
        if documentation_search.is_empty() {
            continue;
        }
        if aggregate_bytes.saturating_add(documentation_search.len())
            > CONCEPT_DOCUMENTATION_FILE_MAX_BYTES
        {
            corpus.size_omitted = corpus.size_omitted.saturating_add(1);
            continue;
        }
        aggregate_bytes += documentation_search.len();
        corpus.documents.push(PendingConceptDocumentation {
            symbol_index,
            documentation_search,
        });
    }
    corpus
}

/// Map a file extension to a language extractor.
pub fn extractor_for_path(path: &Path) -> Option<Box<dyn LanguageExtractor>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    match ext.as_str() {
        "py" | "pyi" => Some(Box::new(PythonExtractor)),
        "ts" | "tsx" => Some(Box::new(TypescriptExtractor::new(ext == "tsx"))),
        "js" | "jsx" | "mjs" | "cjs" => Some(Box::new(JavascriptExtractor)),
        "rs" => Some(Box::new(RustExtractor)),
        "cs" => Some(Box::new(CsharpExtractor)),
        "go" => Some(Box::new(GoExtractor)),
        "java" => Some(Box::new(JavaExtractor)),
        "php" | "phtml" => Some(Box::new(PhpExtractor)),
        "vue" => Some(Box::new(VueExtractor)),
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
    /// Bounded deterministic sample of unsupported-extension paths.
    pub skipped_paths: Vec<String>,
    /// Supported files rejected because their content appears binary.
    pub files_skipped_binary: u32,
    /// Bounded deterministic sample of binary-classified source paths.
    pub skipped_binary_paths: Vec<String>,
    /// Supported files rejected because they exceed [`MAX_INDEXABLE_FILE_SIZE`].
    pub files_skipped_too_large: u32,
    /// Bounded deterministic sample of oversized source paths.
    pub skipped_too_large_paths: Vec<String>,
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
    /// Searchable symbol concepts present after the run; content is never logged.
    pub concept_rows_indexed: u32,
    /// Defensive orphan cleanup count. Foreign keys normally keep this at zero.
    pub concept_orphans_purged: u32,
    /// True when this run rebuilt an older concept-normalization contract.
    pub concept_contract_rebuilt: bool,
    /// Languages whose owned documentation contributes normalized concept terms.
    pub concept_documentation_supported_languages: String,
    /// Searchable documentation documents present after the run.
    pub concept_documentation_indexed: u32,
    /// Candidates omitted because they matched a credential pattern.
    pub concept_documentation_secret_omitted: u32,
    /// Candidates truncated or omitted by per-symbol/per-file size limits.
    pub concept_documentation_size_omitted: u32,
    /// Indexed source files whose language has no documentation collector.
    pub concept_documentation_unsupported_language_files: u32,
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
    #[cfg(test)]
    after_file_commit: Option<Box<dyn Fn() + Send + Sync>>,
}

struct ProjectHistorySnapshot {
    entries: Vec<crate::store::ProjectHistoryEntry>,
    stats: ProjectHistoryIndexStats,
    inventory_token: String,
}

type HistoryCandidate = (PathBuf, &'static str);
type HistoryCandidateInventory = (Vec<HistoryCandidate>, bool);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectHistoryFreshness {
    Fresh,
    Stale,
    Incomplete,
    SnapshotChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IndexLimits {
    source_candidates: Option<usize>,
    source_declared_bytes: Option<u64>,
}

impl IndexLimits {
    pub(crate) const MANUAL: Self = Self {
        source_candidates: None,
        source_declared_bytes: None,
    };

    pub(crate) const AUTO_REFRESH: Self = Self {
        source_candidates: Some(AUTO_REFRESH_SOURCE_CANDIDATE_LIMIT),
        source_declared_bytes: Some(AUTO_REFRESH_SOURCE_AGGREGATE_BYTES),
    };
}

impl Indexer {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            #[cfg(test)]
            after_file_commit: None,
        }
    }

    #[cfg(test)]
    fn with_after_file_commit(mut self, callback: impl Fn() + Send + Sync + 'static) -> Self {
        self.after_file_commit = Some(Box::new(callback));
        self
    }

    fn bind_or_validate_index_root(&self, store: &Store) -> Result<(), IndexError> {
        let existing_root = store
            .meta_value("index_root")
            .map_err(|error| IndexError::Other(error.to_string()))?;
        if existing_root.is_some() {
            return validate_index_root(store, &self.root).map_err(IndexError::Other);
        }

        let has_unbound_data = store
            .file_count()
            .map_err(|error| IndexError::Other(error.to_string()))?
            > 0
            || store
                .task_specs_count()
                .map_err(|error| IndexError::Other(error.to_string()))?
                > 0
            || store
                .project_history_count()
                .map_err(|error| IndexError::Other(error.to_string()))?
                > 0
            || !store
                .scratchpad_read(None, None, None, 1)
                .map_err(|error| IndexError::Other(error.to_string()))?
                .is_empty()
            || store
                .fact_source_count()
                .map_err(|error| IndexError::Other(error.to_string()))?
                > 0;
        if has_unbound_data {
            return Err(IndexError::Other(
                "existing index has no repository identity; rebuild it in a new database"
                    .to_string(),
            ));
        }

        let canonical_root = self
            .root
            .canonicalize()
            .map_err(|error| IndexError::Io(error.to_string()))?;
        store
            .set_meta("index_root", &canonical_root.to_string_lossy())
            .map_err(|error| IndexError::Other(error.to_string()))
    }

    /// Index everything reachable from `root`. Incremental by default — files whose
    /// filesystem mtime is `<=` stored mtime are skipped. `force_full=true` re-indexes
    /// regardless of mtime (e.g. after a schema change or to recover a corrupted index).
    ///
    /// Files in the index but gone from disk are purged at the end. Writes to `store`.
    pub fn index_all(&self, store: &mut Store, force_full: bool) -> Result<IndexStats, IndexError> {
        self.index_all_with_limits(store, force_full, IndexLimits::MANUAL)
    }

    pub(crate) fn index_all_with_limits(
        &self,
        store: &mut Store,
        force_full: bool,
        limits: IndexLimits,
    ) -> Result<IndexStats, IndexError> {
        let start = SystemTime::now();
        ensure_indexing_active(store)?;
        self.bind_or_validate_index_root(store)?;
        store
            .ensure_concept_schema()
            .map_err(|error| IndexError::Other(error.to_string()))?;
        let stored_extractor_contract = store
            .meta_value(EXTRACTOR_CONTRACT_META_KEY)
            .map_err(|error| IndexError::Other(error.to_string()))?;
        let extractor_contract_current =
            stored_extractor_contract.as_deref() == Some(EXTRACTOR_CONTRACT_VERSION);
        let concept_contract_current = store
            .concept_contract_current()
            .map_err(|error| IndexError::Other(error.to_string()))?;
        let indexed_file_count = store
            .file_count()
            .map_err(|error| IndexError::Other(error.to_string()))?;
        let extractor_contract_rebuild_required =
            !extractor_contract_current && indexed_file_count > 0;
        let concept_contract_rebuild_required = !concept_contract_current && indexed_file_count > 0;
        let force_full = force_full || !extractor_contract_current || !concept_contract_current;
        let contracts_need_finalization = force_full;
        if force_full {
            // A full rebuild can replace graph rows incrementally. Persist the
            // derived-corpus dirty marker before the first replacement so an
            // interruption can never leave a partial corpus stamped current.
            store
                .mark_concept_contract_dirty()
                .map_err(|error| IndexError::Other(error.to_string()))?;
        }
        let root_capability = if limits == IndexLimits::MANUAL {
            None
        } else {
            Some(RootCapability::open(&self.root).map_err(index_error_from_read)?)
        };

        // Phase 1: walk filesystem
        let candidates = {
            let interrupted = || store.work_interrupted();
            let control = ReadControl {
                deadline: store.request_deadline(),
                interrupted: Some(&interrupted),
            };
            match (root_capability.as_ref(), limits.source_candidates) {
                (Some(root), Some(limit)) => source_candidates_bounded(root, limit, control)?,
                _ => source_candidates_controlled(&self.root, None, control)?,
            }
        };
        ensure_indexing_active(store)?;

        let mut stats = IndexStats {
            files_scanned: candidates.len() as u32,
            ..Default::default()
        };

        // Phase 2: classify candidates serially (cheap — one lookup per file).
        // Builds (to_parse, current_paths) for phases 3-5.
        let mut to_parse: Vec<PathBuf> = Vec::new();
        let mut current_paths: std::collections::HashSet<String> =
            std::collections::HashSet::with_capacity(candidates.len());
        let mut admitted_identities: HashMap<PathBuf, StableFileIdentity> = HashMap::new();
        let mut declared_source_bytes = 0_u64;

        for path in &candidates {
            ensure_indexing_active(store)?;
            let rel = path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            if extractor_for_path(path).is_none() {
                stats.files_skipped += 1;
                push_path_sample(&mut stats.skipped_paths, &rel);
                continue;
            }
            let admission = {
                let interrupted = || store.work_interrupted();
                let control = ReadControl {
                    deadline: store.request_deadline(),
                    interrupted: Some(&interrupted),
                };
                match root_capability.as_ref() {
                    Some(root) => source_admission_with_capability(root, path, control),
                    None => source_admission_controlled(&self.root, path, control),
                }
            };
            let admitted = match admission {
                Ok(admitted) => admitted,
                Err(IndexError::Skipped(IndexSkipReason::Binary)) => {
                    stats.files_skipped_binary += 1;
                    push_path_sample(&mut stats.skipped_binary_paths, &rel);
                    continue;
                }
                Err(IndexError::Skipped(IndexSkipReason::TooLarge { .. })) => {
                    stats.files_skipped_too_large += 1;
                    push_path_sample(&mut stats.skipped_too_large_paths, &rel);
                    continue;
                }
                Err(error @ (IndexError::Cancelled | IndexError::DeadlineExceeded))
                | Err(error @ IndexError::LimitExceeded { .. }) => return Err(error),
                Err(_) => {
                    stats.files_failed += 1;
                    continue;
                }
            };
            declared_source_bytes = declared_source_bytes
                .checked_add(admitted.declared_len)
                .ok_or(IndexError::LimitExceeded {
                    dimension: "source_declared_bytes",
                    cap: limits.source_declared_bytes.unwrap_or(u64::MAX),
                })?;
            if limits
                .source_declared_bytes
                .is_some_and(|cap| declared_source_bytes > cap)
            {
                return Err(IndexError::LimitExceeded {
                    dimension: "source_declared_bytes",
                    cap: limits.source_declared_bytes.unwrap_or(u64::MAX),
                });
            }
            current_paths.insert(rel.clone());

            if !force_full {
                let fs_mtime = Some(admitted.modified_millis);

                if let (Some(fs_mt), Ok(Some(stored_mt))) = (fs_mtime, store.file_mtime(&rel)) {
                    if fs_mt <= stored_mt {
                        stats.files_unchanged += 1;
                        continue;
                    }
                }
            }

            if limits != IndexLimits::MANUAL {
                admitted_identities.insert(path.clone(), admitted.identity);
            }
            to_parse.push(path.clone());
        }

        // Phases 3-4: parse a bounded batch in parallel, then commit it through
        // SQLite's single writer before parsing the next batch. The old all-at-once
        // collection retained every PendingFile until parsing the whole repository.
        for batch in to_parse.chunks(PARSE_BATCH_SIZE) {
            ensure_indexing_active(store)?;
            let parsed: Vec<(PathBuf, Result<IndexedPendingFile, IndexError>)> = if limits
                == IndexLimits::MANUAL
            {
                batch
                    .par_iter()
                    .filter_map(|path| {
                        let extractor = extractor_for_path(path)?;
                        Some((
                            path.clone(),
                            parse_one_with_concepts(path, &self.root, extractor.as_ref()),
                        ))
                    })
                    .collect()
            } else {
                batch
                    .iter()
                    .filter_map(|path| {
                        let extractor = extractor_for_path(path)?;
                        let interrupted = || store.work_interrupted();
                        let Some(expected_identity) = admitted_identities.get(path).copied() else {
                            return Some((path.clone(), Err(IndexError::SnapshotChanged)));
                        };
                        Some((
                            path.clone(),
                            parse_one_with_concepts_controlled(
                                path,
                                &self.root,
                                root_capability
                                    .as_ref()
                                    .expect("automatic refresh pins its root"),
                                expected_identity,
                                extractor.as_ref(),
                                ReadControl {
                                    deadline: store.request_deadline(),
                                    interrupted: Some(&interrupted),
                                },
                            ),
                        ))
                    })
                    .collect()
            };
            ensure_indexing_active(store)?;

            for (path, outcome) in parsed {
                ensure_indexing_active(store)?;
                let rel = path
                    .strip_prefix(&self.root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                match outcome {
                    Ok(indexed) => {
                        stats.symbols_total += indexed.pending.symbols.len() as u32;
                        stats.edges_total += indexed.pending.edges.len() as u32;
                        if let Some(lang) = guess_language_for(&indexed.pending.path) {
                            *stats.by_language.entry(lang.to_string()).or_insert(0) += 1;
                        }
                        match store
                            .commit_file_with_concepts(indexed.pending, indexed.documentation)
                        {
                            Ok(()) => {
                                stats.files_indexed += 1;
                                #[cfg(test)]
                                if let Some(callback) = &self.after_file_commit {
                                    callback();
                                }
                            }
                            Err(_) => stats.files_failed += 1,
                        }
                    }
                    Err(IndexError::Skipped(IndexSkipReason::Binary)) => {
                        stats.files_skipped_binary += 1;
                        push_path_sample(&mut stats.skipped_binary_paths, &rel);
                    }
                    Err(IndexError::Skipped(IndexSkipReason::TooLarge { .. })) => {
                        stats.files_skipped_too_large += 1;
                        push_path_sample(&mut stats.skipped_too_large_paths, &rel);
                    }
                    Err(error @ (IndexError::Cancelled | IndexError::DeadlineExceeded))
                    | Err(error @ IndexError::LimitExceeded { .. }) => return Err(error),
                    Err(_) => stats.files_failed += 1,
                }
            }
        }

        // Phase 5: purge orphans (in index, no longer on disk). Safe only after a
        // FULL root scan — a partial scan would wrongly purge the unscanned subtree.
        ensure_indexing_active(store)?;
        let indexed = store
            .indexed_paths()
            .map_err(|error| IndexError::Other(error.to_string()))?;
        for path in indexed {
            ensure_indexing_active(store)?;
            if !current_paths.contains(&path) {
                store
                    .purge_file(&path)
                    .map_err(|error| IndexError::Other(error.to_string()))?;
                stats.files_purged += 1;
            }
        }

        // Phase 6: refresh the task-spec FTS5 corpus. Scan
        // `.mastermind/tasks/<NNN>-<name>/spec.md` (each task its own folder;
        // top-level `_*.md` are shared assets and bare `*.md` is legacy 0.6.x
        // layout, both excluded). Whole-corpus replace — spec sets are small
        // (<100 files), so atomic replace beats delta tracking and avoids stale
        // entries on rename/delete.
        ensure_indexing_active(store)?;
        if let Ok(count) = self.index_task_specs(store) {
            stats.task_specs_indexed = count;
        }
        ensure_indexing_active(store)?;
        let history = self.index_project_history(store)?;
        stats.history_entries_indexed = history.indexed;
        stats.history_entries_skipped = history.skipped;
        stats.history_entries_truncated = history.truncated;
        if stats.files_failed == 0 && contracts_need_finalization {
            ensure_indexing_active(store)?;
            let finalized = store
                .finalize_index_contracts_current()
                .map_err(|error| IndexError::Other(error.to_string()))?;
            stats.concept_orphans_purged = finalized.orphans_purged;
            stats.concept_rows_indexed = finalized.rows;
            stats.extractor_contract_rebuilt = extractor_contract_rebuild_required;
            stats.concept_contract_rebuilt = concept_contract_rebuild_required;
        } else {
            stats.concept_orphans_purged = store
                .purge_orphan_concepts()
                .map_err(|error| IndexError::Other(error.to_string()))?;
            stats.concept_rows_indexed = store
                .concept_count()
                .map_err(|error| IndexError::Other(error.to_string()))?;
        }
        let documentation = store
            .concept_documentation_stats()
            .map_err(|error| IndexError::Other(error.to_string()))?;
        stats.concept_documentation_supported_languages =
            crate::store::CONCEPT_DOCUMENTATION_SUPPORTED_LANGUAGES.to_string();
        stats.concept_documentation_indexed = documentation.indexed_documents;
        stats.concept_documentation_secret_omitted = documentation.secret_omitted;
        stats.concept_documentation_size_omitted = documentation.size_omitted;
        stats.concept_documentation_unsupported_language_files =
            documentation.unsupported_language_files;

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
        ensure_indexing_active(store)?;
        let tasks_dir = self.root.join(".mastermind").join("tasks");
        if !std::fs::symlink_metadata(&tasks_dir).is_ok_and(|metadata| {
            metadata.file_type().is_dir() && !metadata.file_type().is_symlink()
        }) {
            // No `.mastermind/tasks/` — clear any stale entries from a prior run too.
            store
                .replace_task_specs(&[])
                .map_err(|e| IndexError::Other(e.to_string()))?;
            return Ok(0);
        }

        let mut entries: Vec<crate::store::TaskSpecEntry> = Vec::new();
        let interrupted = || store.work_interrupted();
        let control = ReadControl {
            deadline: store.request_deadline(),
            interrupted: Some(&interrupted),
        };
        let names = read_directory_names(&self.root, &tasks_dir, MAX_HISTORY_ENTRIES, control)
            .map_err(index_error_from_read)?;
        for name in names {
            ensure_indexing_active(store)?;
            let Some(name) = name.to_str() else {
                continue;
            };
            let path = tasks_dir.join(name);
            // Per-task folders only. Bare top-level `.md` (legacy 0.6.x) and
            // `_`-prefixed names (shared assets, private scratch) are excluded.
            if name.starts_with('_')
                || name.starts_with('.')
                || !std::fs::symlink_metadata(&path).is_ok_and(|metadata| {
                    metadata.file_type().is_dir() && !metadata.file_type().is_symlink()
                })
            {
                continue;
            }
            let spec_path = path.join("spec.md");
            let Ok(read) = read_regular_file(
                &self.root,
                &spec_path,
                MAX_HISTORY_ARTIFACT_SIZE,
                MAX_HISTORY_ARTIFACT_SIZE,
                control,
            ) else {
                continue;
            };
            let Ok(body) = String::from_utf8(read.bytes) else {
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
        ensure_indexing_active(store)?;
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
        ensure_indexing_active(store)?;
        self.bind_or_validate_index_root(store)?;
        let snapshot = {
            let interrupted = || store.work_interrupted();
            self.project_history_snapshot(ReadControl {
                deadline: store.request_deadline(),
                interrupted: Some(&interrupted),
            })?
        };
        ensure_indexing_active(store)?;
        store
            .replace_project_history_snapshot(
                &snapshot.entries,
                snapshot.stats.skipped,
                snapshot.stats.truncated,
                &snapshot.inventory_token,
            )
            .map_err(|error| IndexError::Other(error.to_string()))?;
        Ok(snapshot.stats)
    }

    pub(crate) fn project_history_freshness(
        &self,
        store: &Store,
    ) -> Result<ProjectHistoryFreshness, IndexError> {
        self.live_project_history_inventory(store)
            .map(|(_, freshness)| freshness)
    }

    pub(crate) fn live_project_history_inventory(
        &self,
        store: &Store,
    ) -> Result<(String, ProjectHistoryFreshness), IndexError> {
        let interrupted = || store.work_interrupted();
        let snapshot = match self.project_history_snapshot(ReadControl {
            deadline: store.request_deadline(),
            interrupted: Some(&interrupted),
        }) {
            Ok(snapshot) => snapshot,
            Err(IndexError::SnapshotChanged) => {
                return Ok((String::new(), ProjectHistoryFreshness::SnapshotChanged))
            }
            Err(error) => return Err(error),
        };
        let stored = store
            .meta_value("project_history_inventory_token")
            .map_err(|error| IndexError::Other(error.to_string()))?;
        let freshness = if stored.as_deref() != Some(snapshot.inventory_token.as_str()) {
            ProjectHistoryFreshness::Stale
        } else if snapshot.stats.skipped > 0 || snapshot.stats.truncated {
            ProjectHistoryFreshness::Incomplete
        } else {
            ProjectHistoryFreshness::Fresh
        };
        Ok((snapshot.inventory_token, freshness))
    }

    fn project_history_snapshot(
        &self,
        control: ReadControl<'_>,
    ) -> Result<ProjectHistorySnapshot, IndexError> {
        let root = RootCapability::open(&self.root).map_err(index_error_from_read)?;
        let first = self.project_history_snapshot_once(&root, control)?;
        control.check().map_err(index_error_from_read)?;
        let second = self.project_history_snapshot_once(&root, control)?;
        if first.inventory_token != second.inventory_token
            || first.stats.indexed != second.stats.indexed
            || first.stats.skipped != second.stats.skipped
            || first.stats.truncated != second.stats.truncated
        {
            return Err(IndexError::SnapshotChanged);
        }
        root.verify().map_err(index_error_from_read)?;
        Ok(second)
    }

    fn project_history_snapshot_once(
        &self,
        root: &RootCapability,
        control: ReadControl<'_>,
    ) -> Result<ProjectHistorySnapshot, IndexError> {
        let (mut candidates, mut truncated) = collect_project_history_candidates(root, control)?;
        candidates
            .sort_by(|left, right| (left.0.as_path(), left.1).cmp(&(right.0.as_path(), right.1)));
        candidates.dedup_by(|left, right| left.0 == right.0 && left.1 == right.1);
        if candidates.len() > MAX_HISTORY_ENTRIES {
            truncated = true;
            candidates.truncate(MAX_HISTORY_ENTRIES);
        }

        let mut digest = Sha256::new();
        digest.update(b"mmcg-project-history-inventory-v1\0");
        let mut entries = Vec::new();
        let mut skipped = 0_u32;
        let mut aggregate_bytes = 0_u64;
        for (path, kind) in candidates {
            control.check().map_err(index_error_from_read)?;
            let relative = path
                .strip_prefix(root.canonical_root())
                .map_err(|_| IndexError::SnapshotChanged)?
                .to_string_lossy()
                .replace('\\', "/");
            digest.update(relative.as_bytes());
            digest.update([0]);
            digest.update(kind.as_bytes());
            digest.update([0]);

            let inspected = match read_regular_file_with_capability(
                root,
                &path,
                MAX_HISTORY_ARTIFACT_SIZE,
                0,
                control,
            ) {
                Ok(file) => file,
                Err(BoundedReadError::Interrupted) => return Err(IndexError::Cancelled),
                Err(BoundedReadError::DeadlineExceeded) => {
                    return Err(IndexError::DeadlineExceeded)
                }
                Err(BoundedReadError::SnapshotChanged) => return Err(IndexError::SnapshotChanged),
                Err(error) => {
                    skipped = skipped.saturating_add(1);
                    digest.update(history_error_class(&error));
                    digest.update([0]);
                    continue;
                }
            };
            let Some(next_aggregate) = aggregate_bytes.checked_add(inspected.declared_len) else {
                truncated = true;
                digest.update(b"aggregate_overflow\0");
                continue;
            };
            if next_aggregate > MAX_HISTORY_AGGREGATE_BYTES {
                truncated = true;
                digest.update(b"aggregate_omitted\0");
                continue;
            }
            let read = match read_regular_file_expected(
                root,
                &path,
                MAX_HISTORY_ARTIFACT_SIZE,
                MAX_HISTORY_ARTIFACT_SIZE,
                control,
                Some(inspected.identity),
            ) {
                Ok(file) => file,
                Err(BoundedReadError::Interrupted) => return Err(IndexError::Cancelled),
                Err(BoundedReadError::DeadlineExceeded) => {
                    return Err(IndexError::DeadlineExceeded)
                }
                Err(BoundedReadError::SnapshotChanged) => return Err(IndexError::SnapshotChanged),
                Err(error) => {
                    skipped = skipped.saturating_add(1);
                    digest.update(history_error_class(&error));
                    digest.update([0]);
                    continue;
                }
            };
            let content_digest = Sha256::digest(&read.bytes);
            digest.update(read.declared_len.to_le_bytes());
            digest.update(content_digest);
            digest.update([0]);
            let body = match String::from_utf8(read.bytes) {
                Ok(body) => body,
                Err(_) => {
                    skipped = skipped.saturating_add(1);
                    digest.update(b"invalid_utf8\0");
                    continue;
                }
            };
            aggregate_bytes = next_aggregate;
            let fallback = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or(kind);
            let title = extract_spec_title(&body, fallback);
            entries.push(crate::store::ProjectHistoryEntry {
                path: relative,
                kind: kind.to_string(),
                title,
                body,
            });
        }
        digest.update(skipped.to_le_bytes());
        digest.update([u8::from(truncated)]);
        digest.update(aggregate_bytes.to_le_bytes());
        let stats = ProjectHistoryIndexStats {
            indexed: entries.len() as u32,
            skipped,
            truncated,
        };
        Ok(ProjectHistorySnapshot {
            entries,
            stats,
            inventory_token: crate::hex::encode(&digest.finalize()),
        })
    }

    /// Re-index a single file. Used by the watcher.
    pub fn index_one(&self, store: &mut Store, path: &Path) -> Result<(), IndexError> {
        ensure_indexing_active(store)?;
        let extractor = extractor_for_path(path)
            .ok_or_else(|| IndexError::Parse(format!("no extractor for {path:?}")))?;
        let indexed = parse_one_with_concepts(path, &self.root, extractor.as_ref())?;
        ensure_indexing_active(store)?;
        store
            .commit_file_with_concepts(indexed.pending, indexed.documentation)
            .map_err(|e| IndexError::Other(e.to_string()))
    }
}

fn history_error_class(error: &BoundedReadError) -> &'static [u8] {
    match error {
        BoundedReadError::InvalidPath => b"invalid_path",
        BoundedReadError::OutsideRoot => b"outside_root",
        BoundedReadError::NotRegular => b"not_regular",
        BoundedReadError::TooLarge { .. } => b"too_large",
        BoundedReadError::SnapshotChanged => b"snapshot_changed",
        BoundedReadError::Interrupted => b"cancelled",
        BoundedReadError::DeadlineExceeded => b"deadline",
        BoundedReadError::Io(_) => b"io",
    }
}

fn history_directory_names(
    root: &RootCapability,
    directory: &Path,
    remaining_entries: usize,
    control: ReadControl<'_>,
) -> Result<Option<Vec<std::ffi::OsString>>, IndexError> {
    match read_directory_names_with_capability(root, directory, remaining_entries, control) {
        Ok(names) => Ok(Some(names)),
        Err(BoundedReadError::Interrupted) => Err(IndexError::Cancelled),
        Err(BoundedReadError::DeadlineExceeded) => Err(IndexError::DeadlineExceeded),
        Err(BoundedReadError::SnapshotChanged) => Err(IndexError::SnapshotChanged),
        Err(BoundedReadError::TooLarge { .. }) => Ok(None),
        Err(_) => Ok(None),
    }
}

fn history_path_kind(
    root: &RootCapability,
    path: &Path,
    control: ReadControl<'_>,
) -> Result<Option<BoundedPathKind>, IndexError> {
    match inspect_path_kind_with_capability(root, path, control) {
        Ok(kind) => Ok(Some(kind)),
        Err(BoundedReadError::Interrupted) => Err(IndexError::Cancelled),
        Err(BoundedReadError::DeadlineExceeded) => Err(IndexError::DeadlineExceeded),
        Err(BoundedReadError::SnapshotChanged) => Err(IndexError::SnapshotChanged),
        Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(_) => Ok(Some(BoundedPathKind::Other)),
    }
}

fn add_history_candidate(
    root: &RootCapability,
    candidates: &mut Vec<HistoryCandidate>,
    path: PathBuf,
    kind: &'static str,
    control: ReadControl<'_>,
) -> Result<(), IndexError> {
    if history_path_kind(root, &path, control)? == Some(BoundedPathKind::RegularFile) {
        candidates.push((path, kind));
    }
    Ok(())
}

fn collect_project_history_candidates(
    root: &RootCapability,
    control: ReadControl<'_>,
) -> Result<HistoryCandidateInventory, IndexError> {
    let root_path = root.canonical_root();
    let mut candidates = Vec::new();
    let mut truncated = false;
    let mut directory_entries = 0usize;

    add_history_candidate(
        root,
        &mut candidates,
        root_path.join("CONTEXT.md"),
        "context",
        control,
    )?;
    match history_directory_names(
        root,
        root_path,
        MAX_HISTORY_DIRECTORY_ENTRIES.saturating_sub(directory_entries),
        control,
    )? {
        Some(names) => {
            directory_entries = directory_entries.saturating_add(names.len());
            for name in names {
                control.check().map_err(index_error_from_read)?;
                let Some(name) = name.to_str() else { continue };
                if name.starts_with("CONTEXT-archive-") && name.ends_with(".md") {
                    add_history_candidate(
                        root,
                        &mut candidates,
                        root_path.join(name),
                        "context",
                        control,
                    )?;
                }
            }
        }
        None => truncated = true,
    }

    let tasks_dir = root_path.join(".mastermind").join("tasks");
    add_history_candidate(
        root,
        &mut candidates,
        tasks_dir.join("_lessons.md"),
        "lesson",
        control,
    )?;
    if history_path_kind(root, &tasks_dir, control)? == Some(BoundedPathKind::Directory) {
        match history_directory_names(
            root,
            &tasks_dir,
            MAX_HISTORY_DIRECTORY_ENTRIES.saturating_sub(directory_entries),
            control,
        )? {
            Some(names) => {
                directory_entries = directory_entries.saturating_add(names.len());
                for name in names {
                    control.check().map_err(index_error_from_read)?;
                    let Some(name) = name.to_str() else { continue };
                    if name.starts_with('_') || name.starts_with('.') {
                        continue;
                    }
                    let path = tasks_dir.join(name);
                    if history_path_kind(root, &path, control)? != Some(BoundedPathKind::Directory)
                    {
                        continue;
                    }
                    add_history_candidate(
                        root,
                        &mut candidates,
                        path.join("spec.md"),
                        "task_spec",
                        control,
                    )?;
                    add_history_candidate(
                        root,
                        &mut candidates,
                        path.join("executor-report.md"),
                        "executor_report",
                        control,
                    )?;
                    add_history_candidate(
                        root,
                        &mut candidates,
                        path.join("audit.md"),
                        "audit",
                        control,
                    )?;
                    // Tasks created before 0.39 can keep release notes beside the spec.
                    add_history_candidate(
                        root,
                        &mut candidates,
                        path.join("release-notes.md"),
                        "release_notes",
                        control,
                    )?;
                }
            }
            None => truncated = true,
        }
    }

    let releases_dir = root_path.join(".mastermind").join("releases");
    if history_path_kind(root, &releases_dir, control)? == Some(BoundedPathKind::Directory) {
        match history_directory_names(
            root,
            &releases_dir,
            MAX_HISTORY_DIRECTORY_ENTRIES.saturating_sub(directory_entries),
            control,
        )? {
            Some(names) => {
                directory_entries = directory_entries.saturating_add(names.len());
                for name in names {
                    control.check().map_err(index_error_from_read)?;
                    let path = releases_dir.join(name);
                    if path
                        .extension()
                        .and_then(|value| value.to_str())
                        .is_some_and(|value| value.eq_ignore_ascii_case("md"))
                    {
                        add_history_candidate(
                            root,
                            &mut candidates,
                            path,
                            "release_notes",
                            control,
                        )?;
                    }
                }
            }
            None => truncated = true,
        }
    }

    let mut decision_roots = [
        "docs/adr",
        "docs/adrs",
        "docs/decisions",
        "adr",
        "adrs",
        ".mastermind/decisions",
    ]
    .map(|relative| root_path.join(relative));
    decision_roots.sort();
    let mut pending = decision_roots.to_vec();
    let mut visited_directories = 0usize;
    while let Some(directory) = pending.pop() {
        control.check().map_err(index_error_from_read)?;
        if candidates.len() > MAX_HISTORY_ENTRIES
            || visited_directories >= MAX_HISTORY_ENTRIES
            || directory_entries >= MAX_HISTORY_DIRECTORY_ENTRIES
        {
            truncated = true;
            break;
        }
        let exists = history_path_kind(root, &directory, control)?;
        if exists != Some(BoundedPathKind::Directory) {
            continue;
        }
        let Some(names) = history_directory_names(
            root,
            &directory,
            MAX_HISTORY_DIRECTORY_ENTRIES.saturating_sub(directory_entries),
            control,
        )?
        else {
            truncated = true;
            continue;
        };
        visited_directories = visited_directories.saturating_add(1);
        directory_entries = directory_entries.saturating_add(names.len());
        let mut child_directories = Vec::new();
        for name in names {
            let path = directory.join(name);
            let kind = history_path_kind(root, &path, control)?;
            if kind == Some(BoundedPathKind::Directory) {
                child_directories.push(path);
            } else if kind == Some(BoundedPathKind::RegularFile)
                && path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("md"))
            {
                candidates.push((path, "architecture_decision"));
            }
        }
        child_directories.sort();
        child_directories.reverse();
        pending.extend(child_directories);
    }
    Ok((candidates, truncated))
}

fn push_path_sample(samples: &mut Vec<String>, path: &str) {
    if samples.len() < SKIPPED_PATH_SAMPLE_LIMIT && !samples.iter().any(|item| item == path) {
        samples.push(path.to_string());
    }
}

pub(crate) fn is_skipped_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

fn has_skipped_component(path: &Path) -> bool {
    path.components()
        .any(|component| is_skipped_dir(component.as_os_str().to_str().unwrap_or("")))
}

/// Tracked files remain source-of-truth even when a later or overly broad
/// ignore rule matches them. Git itself applies ignore rules to untracked
/// discovery, not to entries already present in the index. Failure to query Git
/// is non-fatal: the ordinary ignore-aware filesystem walk remains available
/// outside repositories and when Git is unavailable.
fn git_tracked_relative_paths(root: &Path) -> Vec<PathBuf> {
    tracked_relative_paths(root).unwrap_or_default()
}

/// Existing tracked files, bounded by the same subprocess timeout and output
/// cap used during indexing. Indexing can fall back to the ignore-aware walk
/// when Git is unavailable; fail-closed readers such as Lens use the `Result`
/// to avoid treating an incomplete tracked-file inventory as fresh.
pub(crate) fn tracked_relative_paths(
    root: &Path,
) -> Result<Vec<PathBuf>, crate::diff::WorkingTreeDiffError> {
    tracked_relative_paths_controlled(root, ReadControl::default())
}

fn tracked_relative_paths_controlled(
    root: &Path,
    control: ReadControl<'_>,
) -> Result<Vec<PathBuf>, crate::diff::WorkingTreeDiffError> {
    let output = crate::diff::run_bounded_git_with_control(
        root,
        &["ls-files", "--cached", "-z", "--"],
        None,
        GIT_TRACKED_PATH_OUTPUT_LIMIT,
        control.deadline,
        control.interrupted,
    )?;
    if !output.success {
        return Err(crate::diff::WorkingTreeDiffError::GitUnavailable);
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .filter_map(|path| std::str::from_utf8(path).ok())
        .map(PathBuf::from)
        .filter(|path| !path.is_absolute() && !has_skipped_component(path))
        .filter(|path| {
            std::fs::symlink_metadata(root.join(path))
                .is_ok_and(|metadata| metadata.file_type().is_file())
        })
        .collect())
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
    source_candidates_controlled(root, None, ReadControl::default()).unwrap_or_default()
}

pub(crate) fn source_candidates_bounded(
    root: &RootCapability,
    limit: usize,
    control: ReadControl<'_>,
) -> Result<Vec<PathBuf>, IndexError> {
    control.check().map_err(index_error_from_read)?;
    root.verify().map_err(index_error_from_read)?;
    let output = crate::diff::run_bounded_git_with_control(
        root.canonical_root(),
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
        ],
        None,
        GIT_TRACKED_PATH_OUTPUT_LIMIT,
        control.deadline,
        control.interrupted,
    )
    .map_err(|error| match error {
        crate::diff::WorkingTreeDiffError::GitTimeout
            if control.interrupted.is_some_and(|check| check()) =>
        {
            IndexError::Cancelled
        }
        crate::diff::WorkingTreeDiffError::GitTimeout => IndexError::DeadlineExceeded,
        crate::diff::WorkingTreeDiffError::GitOutputLimit => IndexError::LimitExceeded {
            dimension: "source_candidates",
            cap: limit as u64,
        },
        error => IndexError::Other(error.to_string()),
    })?;
    if !output.success {
        return Err(IndexError::Other("git source inventory unavailable".into()));
    }
    let mut paths = BTreeSet::new();
    for raw in output.stdout.split(|byte| *byte == 0) {
        control.check().map_err(index_error_from_read)?;
        if raw.is_empty() {
            continue;
        }
        let relative = std::str::from_utf8(raw)
            .map(PathBuf::from)
            .map_err(|_| IndexError::SnapshotChanged)?;
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
            || has_skipped_component(&relative)
        {
            continue;
        }
        paths.insert(root.canonical_root().join(relative));
        if paths.len() > limit {
            return Err(IndexError::LimitExceeded {
                dimension: "source_candidates",
                cap: limit as u64,
            });
        }
    }
    root.verify().map_err(index_error_from_read)?;
    Ok(paths.into_iter().collect())
}

fn source_candidates_controlled(
    root: &Path,
    limit: Option<usize>,
    control: ReadControl<'_>,
) -> Result<Vec<PathBuf>, IndexError> {
    if let Some(limit) = limit {
        let root = RootCapability::open(root).map_err(index_error_from_read)?;
        return source_candidates_bounded(&root, limit, control);
    }
    control.check().map_err(index_error_from_read)?;
    let tracked = match tracked_relative_paths_controlled(root, control) {
        Ok(paths) => paths,
        Err(_) if limit.is_none() => Vec::new(),
        Err(crate::diff::WorkingTreeDiffError::GitTimeout)
            if control.interrupted.is_some_and(|check| check()) =>
        {
            return Err(IndexError::Cancelled);
        }
        Err(crate::diff::WorkingTreeDiffError::GitTimeout) => {
            return Err(IndexError::DeadlineExceeded);
        }
        Err(error) => return Err(IndexError::Other(error.to_string())),
    };
    let mut paths = BTreeSet::new();
    for relative in tracked {
        control.check().map_err(index_error_from_read)?;
        paths.insert(root.join(relative));
        if limit.is_some_and(|cap| paths.len() > cap) {
            return Err(IndexError::LimitExceeded {
                dimension: "source_candidates",
                cap: limit.unwrap_or(usize::MAX) as u64,
            });
        }
    }
    for entry in source_walk_builder(root).build() {
        control.check().map_err(index_error_from_read)?;
        let Ok(entry) = entry else {
            continue;
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        paths.insert(entry.into_path());
        if limit.is_some_and(|cap| paths.len() > cap) {
            return Err(IndexError::LimitExceeded {
                dimension: "source_candidates",
                cap: limit.unwrap_or(usize::MAX) as u64,
            });
        }
    }
    Ok(paths.into_iter().collect())
}

/// Stateful ignore matcher for watcher events. It applies the same gitignore
/// and global-ignore rules as the initial walk, including nested `.gitignore`s.
pub(crate) struct SourceMatcher {
    root: PathBuf,
    matcher: IncrementalIgnore,
    tracked: HashSet<PathBuf>,
}

impl SourceMatcher {
    pub(crate) fn new(root: &Path) -> Self {
        let matcher = source_walk_builder(root)
            .build_matchers()
            .into_iter()
            .next()
            .expect("one ignore matcher for one source root");
        let mut tracked = HashSet::new();
        for relative in git_tracked_relative_paths(root) {
            let absolute = root.join(&relative);
            tracked.insert(relative);
            tracked.insert(absolute.canonicalize().unwrap_or(absolute));
        }
        Self {
            root: root.to_path_buf(),
            matcher,
            tracked,
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
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if self.tracked.contains(&relative) || self.tracked.contains(&canonical) {
            return false;
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

pub(crate) fn guess_language_for(rel_path: &str) -> Option<&'static str> {
    let extension = Path::new(rel_path)
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "py" | "pyi") {
        Some("python")
    } else if extension == "vue" {
        Some("vue")
    } else if extension == "tsx" {
        Some("tsx")
    } else if extension == "ts" {
        Some("typescript")
    } else if matches!(extension.as_str(), "jsx" | "js" | "mjs" | "cjs") {
        // `.jsx` is a JavaScript dialect — store as "javascript", not a distinct
        // "jsx". The MCP `language` enum and `lang_from_ext` already treat it as
        // javascript; "jsx" made `.jsx` symbols invisible to every
        // `language: "javascript"` filter.
        Some("javascript")
    } else if extension == "rs" {
        Some("rust")
    } else if extension == "cs" {
        Some("csharp")
    } else if extension == "go" {
        Some("go")
    } else if extension == "java" {
        Some("java")
    } else if matches!(extension.as_str(), "php" | "phtml") {
        Some("php")
    } else if matches!(
        extension.as_str(),
        "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hh" | "hxx" | "ipp" | "tpp"
    ) {
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
    let source = read_source_bounded(path, root, ReadControl::default())?;
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    parse_blob(&rel, &source.bytes, source.modified_millis, extractor)
}

fn parse_one_with_concepts(
    path: &Path,
    root: &Path,
    extractor: &dyn LanguageExtractor,
) -> Result<IndexedPendingFile, IndexError> {
    let source = read_source_bounded(path, root, ReadControl::default())?;
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    parse_blob_with_concepts(&rel, &source.bytes, source.modified_millis, extractor)
}

#[cfg(test)]
fn parse_one_controlled(
    path: &Path,
    root: &Path,
    root_capability: &RootCapability,
    expected_identity: StableFileIdentity,
    extractor: &dyn LanguageExtractor,
    control: ReadControl<'_>,
) -> Result<PendingFile, IndexError> {
    let source = read_regular_file_expected(
        root_capability,
        path,
        MAX_INDEXABLE_FILE_SIZE,
        MAX_INDEXABLE_FILE_SIZE,
        control,
        Some(expected_identity),
    )
    .map_err(index_error_from_read)?;
    if is_binary_content(&source.bytes[..source.bytes.len().min(BINARY_SNIFF_BYTES as usize)]) {
        return Err(IndexError::Skipped(IndexSkipReason::Binary));
    }

    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");

    parse_blob(&rel, &source.bytes, source.modified_millis, extractor)
}

fn parse_one_with_concepts_controlled(
    path: &Path,
    root: &Path,
    root_capability: &RootCapability,
    expected_identity: StableFileIdentity,
    extractor: &dyn LanguageExtractor,
    control: ReadControl<'_>,
) -> Result<IndexedPendingFile, IndexError> {
    let source = read_regular_file_expected(
        root_capability,
        path,
        MAX_INDEXABLE_FILE_SIZE,
        MAX_INDEXABLE_FILE_SIZE,
        control,
        Some(expected_identity),
    )
    .map_err(index_error_from_read)?;
    if is_binary_content(&source.bytes[..source.bytes.len().min(BINARY_SNIFF_BYTES as usize)]) {
        return Err(IndexError::Skipped(IndexSkipReason::Binary));
    }
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    parse_blob_with_concepts(&rel, &source.bytes, source.modified_millis, extractor)
}

pub(crate) fn source_admission_mtime(root: &Path, path: &Path) -> Result<i64, IndexError> {
    source_admission_controlled(root, path, ReadControl::default()).map(|file| file.modified_millis)
}

pub(crate) fn source_admission_controlled(
    root: &Path,
    path: &Path,
    control: ReadControl<'_>,
) -> Result<crate::bounded_fs::BoundedFile, IndexError> {
    let root = RootCapability::open(root).map_err(index_error_from_read)?;
    source_admission_with_capability(&root, path, control)
}

pub(crate) fn source_admission_with_capability(
    root: &RootCapability,
    path: &Path,
    control: ReadControl<'_>,
) -> Result<crate::bounded_fs::BoundedFile, IndexError> {
    let prefix = read_regular_file_with_capability(
        root,
        path,
        MAX_INDEXABLE_FILE_SIZE,
        BINARY_SNIFF_BYTES,
        control,
    )
    .map_err(index_error_from_read)?;
    if is_binary_content(&prefix.bytes) {
        return Err(IndexError::Skipped(IndexSkipReason::Binary));
    }
    Ok(prefix)
}

fn read_source_bounded(
    path: &Path,
    root: &Path,
    control: ReadControl<'_>,
) -> Result<crate::bounded_fs::BoundedFile, IndexError> {
    let source = read_regular_file(
        root,
        path,
        MAX_INDEXABLE_FILE_SIZE,
        MAX_INDEXABLE_FILE_SIZE,
        control,
    )
    .map_err(index_error_from_read)?;
    if is_binary_content(&source.bytes[..source.bytes.len().min(BINARY_SNIFF_BYTES as usize)]) {
        return Err(IndexError::Skipped(IndexSkipReason::Binary));
    }
    Ok(source)
}

pub(crate) fn index_error_from_read(error: BoundedReadError) -> IndexError {
    match error {
        BoundedReadError::TooLarge { size, .. } => {
            IndexError::Skipped(IndexSkipReason::TooLarge { size })
        }
        BoundedReadError::Interrupted => IndexError::Cancelled,
        BoundedReadError::DeadlineExceeded => IndexError::DeadlineExceeded,
        BoundedReadError::SnapshotChanged
        | BoundedReadError::OutsideRoot
        | BoundedReadError::InvalidPath
        | BoundedReadError::NotRegular => IndexError::SnapshotChanged,
        BoundedReadError::Io(error) => IndexError::Io(error.to_string()),
    }
}

pub(crate) fn is_binary_content(content: &[u8]) -> bool {
    !matches!(content, [0xff, 0xfe, ..] | [0xfe, 0xff, ..]) && content.contains(&0)
}

fn source_for_parser(source: &[u8]) -> Result<Cow<'_, [u8]>, IndexError> {
    let (little_endian, body) = match source {
        [0xff, 0xfe, rest @ ..] => (true, rest),
        [0xfe, 0xff, rest @ ..] => (false, rest),
        _ if is_binary_content(source) => {
            return Err(IndexError::Skipped(IndexSkipReason::Binary));
        }
        _ => return Ok(Cow::Borrowed(source)),
    };
    if body.len() % 2 != 0 {
        return Err(IndexError::Parse("odd-length UTF-16 source".to_string()));
    }
    let (pairs, remainder) = body.as_chunks::<2>();
    debug_assert!(remainder.is_empty());
    let units = pairs
        .iter()
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        })
        .collect::<Vec<_>>();
    let decoded = String::from_utf16(&units)
        .map_err(|_| IndexError::Parse("invalid UTF-16 source".to_string()))?;
    Ok(Cow::Owned(decoded.into_bytes()))
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
    parse_blob_internal(rel_path, source, mtime, extractor, false).map(|parsed| parsed.pending)
}

fn parse_blob_with_concepts(
    rel_path: &str,
    source: &[u8],
    mtime: i64,
    extractor: &dyn LanguageExtractor,
) -> Result<IndexedPendingFile, IndexError> {
    parse_blob_internal(rel_path, source, mtime, extractor, true)
}

fn parse_blob_internal(
    rel_path: &str,
    source: &[u8],
    mtime: i64,
    extractor: &dyn LanguageExtractor,
    collect_documentation: bool,
) -> Result<IndexedPendingFile, IndexError> {
    let parser_source = source_for_parser(source)?;
    let mut parser = Parser::new();
    let language = extractor.language();
    parser
        .set_language(&language)
        .map_err(|e| IndexError::Parse(e.to_string()))?;
    let tree = parser
        .parse(parser_source.as_ref(), None)
        .ok_or_else(|| IndexError::Parse("tree-sitter parse returned None".to_string()))?;

    let line_count = parser_source.iter().filter(|&&b| b == b'\n').count() as u32 + 1;
    let language = guess_language_for(rel_path).unwrap_or("").to_string();
    let mut pending = PendingFile {
        path: rel_path.to_string(),
        mtime,
        content_sha256: crate::hex::encode(&Sha256::digest(source)),
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

    extractor.extract(&tree, parser_source.as_ref(), &mut pending, module_index);
    let candidates = if collect_documentation {
        match pending.language.as_str() {
            "rust" => rust_lang::collect_concept_documentation(
                &tree,
                parser_source.as_ref(),
                &pending,
                module_index,
            ),
            "python" => python::collect_concept_documentation(
                &tree,
                parser_source.as_ref(),
                &pending,
                module_index,
            ),
            "typescript" | "tsx" => {
                typescript::collect_concept_documentation(&tree, parser_source.as_ref(), &pending)
            }
            "javascript" => {
                javascript::collect_concept_documentation(&tree, parser_source.as_ref(), &pending)
            }
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };
    let documentation = build_concept_documentation_corpus(&pending.language, candidates);
    Ok(IndexedPendingFile {
        pending,
        documentation,
    })
}

#[derive(Debug)]
pub enum IndexError {
    Io(String),
    Parse(String),
    Other(String),
    Skipped(IndexSkipReason),
    SnapshotChanged,
    Cancelled,
    DeadlineExceeded,
    LimitExceeded { dimension: &'static str, cap: u64 },
}

fn ensure_indexing_active(store: &Store) -> Result<(), IndexError> {
    if store.work_interrupted() {
        Err(match store.interrupt_source() {
            Some(crate::store::InterruptSource::Cancel) => IndexError::Cancelled,
            Some(crate::store::InterruptSource::Budget) => IndexError::DeadlineExceeded,
            None => IndexError::Other("indexing interrupted".to_string()),
        })
    } else {
        Ok(())
    }
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
            IndexError::SnapshotChanged => write!(f, "snapshot changed during bounded read"),
            IndexError::Cancelled => write!(f, "indexing cancelled"),
            IndexError::DeadlineExceeded => write!(f, "indexing deadline exceeded"),
            IndexError::LimitExceeded { dimension, cap } => {
                write!(f, "refresh limit exceeded: {dimension} > {cap}")
            }
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
    use std::process::Command;
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

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn documentation_matches(store: &Store, term: &str) -> Vec<String> {
        store
            .search_concepts(&format!("\"{term}\""), 50)
            .unwrap()
            .into_iter()
            .filter(|hit| hit.documentation_matched)
            .map(|hit| hit.name)
            .collect()
    }

    #[test]
    fn rust_concept_documentation_attaches_outer_and_module_docs_only() {
        let (dir, db) = setup("rust_concept_documentation");
        fs::write(
            dir.join("owned.rs"),
            "//! crate atlasquartz routing\n\
             // license ordinaryquartz\n\
             /// retryquartz handler\n\
             #[inline]\n\
             fn accepted() {}\n\
             /** ledgerquartz model */\n\
             struct Ledger;\n\
             /// outermodulequartz docs\n\
             mod nested {\n\
                 //! innermodulequartz docs\n\
             }\n\
             /// blankgapquartz ignored\n\
             \n\
             fn blank_gap() {}\n\
             // linecommentquartz ignored\n\
             fn ordinary() {}\n\
             fn trailing() {} /// trailingquartz ignored\n",
        )
        .unwrap();
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, true).unwrap();

        assert_eq!(documentation_matches(&store, "atlasquartz"), ["<module>"]);
        assert_eq!(documentation_matches(&store, "retryquartz"), ["accepted"]);
        assert_eq!(documentation_matches(&store, "ledgerquartz"), ["Ledger"]);
        assert_eq!(
            documentation_matches(&store, "outermodulequartz"),
            ["nested"]
        );
        assert_eq!(
            documentation_matches(&store, "innermodulequartz"),
            ["nested"]
        );
        for rejected in [
            "ordinaryquartz",
            "blankgapquartz",
            "linecommentquartz",
            "trailingquartz",
        ] {
            assert!(
                documentation_matches(&store, rejected).is_empty(),
                "unexpected Rust documentation match for {rejected}"
            );
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn python_concept_documentation_attaches_only_owned_plain_docstrings() {
        let (dir, db) = setup("python_concept_documentation");
        fs::write(
            dir.join("owned.py"),
            r#"# comment before module docs
"""modulequartz compass"""

def marker(fn):
    return fn

@marker
async def accepted():
    """asyncquartz orbit"""
    return 1

class Owned:
    """classquartz prism"""
    def method(self):
        """methodquartz amber"""
        return 1

def fstring():
    f"""fstringquartz {1}"""

def later():
    value = 1
    """laterquartz"""

def assigned():
    value = """assignedquartz"""

def concatenated(value):
    "concatquartz" + value
"#,
        )
        .unwrap();
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, true).unwrap();

        assert_eq!(documentation_matches(&store, "modulequartz"), ["<module>"]);
        assert_eq!(documentation_matches(&store, "asyncquartz"), ["accepted"]);
        assert_eq!(documentation_matches(&store, "classquartz"), ["Owned"]);
        assert_eq!(documentation_matches(&store, "methodquartz"), ["method"]);
        for rejected in [
            "fstringquartz",
            "laterquartz",
            "assignedquartz",
            "concatquartz",
        ] {
            assert!(
                documentation_matches(&store, rejected).is_empty(),
                "unexpected Python documentation match for {rejected}"
            );
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn js_ts_concept_documentation_attaches_immediate_jsdoc_through_export() {
        let (dir, db) = setup("js_ts_concept_documentation");
        fs::write(
            dir.join("owned.ts"),
            "/** @file headerquartz */\n\
             function headerTarget() {}\n\
             /** Copyright licensequartz */\n\
             function licenseTarget() {}\n\
             /** exportquartz handler */\n\
             export function accepted() {}\n\
             /** classquartz service */\n\
             export class Service {\n\
                 /** methodquartz action */\n\
                 run() {}\n\
             }\n\
             /** variablequartz callback */\n\
             export const Handler = () => 1;\n\
             /** blankgapquartz ignored */\n\
             \n\
             function blankGap() {}\n\
             /* ordinaryquartz ignored */\n\
             function ordinary() {}\n\
             function trailing() {} /** trailingquartz ignored */\n\
             /** stalequartz ignored */\n\
             const value = 1;\n\
             function afterValue() {}\n",
        )
        .unwrap();
        fs::write(
            dir.join("owned.js"),
            "/** javascriptquartz export */\nexport function jsAccepted() {}\n",
        )
        .unwrap();
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, true).unwrap();

        assert_eq!(documentation_matches(&store, "exportquartz"), ["accepted"]);
        assert_eq!(documentation_matches(&store, "classquartz"), ["Service"]);
        assert_eq!(documentation_matches(&store, "methodquartz"), ["run"]);
        assert_eq!(documentation_matches(&store, "variablequartz"), ["Handler"]);
        assert_eq!(
            documentation_matches(&store, "javascriptquartz"),
            ["jsAccepted"]
        );
        for rejected in [
            "blankgapquartz",
            "ordinaryquartz",
            "trailingquartz",
            "stalequartz",
            "headerquartz",
            "licensequartz",
        ] {
            assert!(
                documentation_matches(&store, rejected).is_empty(),
                "unexpected JS/TS documentation match for {rejected}"
            );
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn concept_documentation_secret_patterns_omit_whole_candidates() {
        let (dir, db) = setup("concept_documentation_secrets");
        fs::write(
            dir.join("secrets.py"),
            r#"def private_material():
    """privatecanary -----BEGIN PRIVATE KEY----- material"""

def aws_material():
    """awscanary AKIAIOSFODNN7EXAMPLE"""

def bearer_material():
    """bearercanary Authorization: Bearer livecredential123"""

def json_bearer_material():
    """jsonbearercanary {"Authorization": "Bearer jsoncredential789"}"""

def password_material():
    """passwordcanary password = livecredential456"""

def api_key_material():
    """apiassignmentcanary stripe_api_key = liveexamplecredential"""

def prose():
    """rotationquartz explains token buckets and password rotation"""

def bearer_prose():
    """bearerwordquartz Authorization: Bearerish explanation"""

def placeholder():
    """placeholderquartz token = placeholder"""
"#,
        )
        .unwrap();
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, true).unwrap();

        for omitted in [
            "privatecanary",
            "awscanary",
            "bearercanary",
            "jsonbearercanary",
            "passwordcanary",
            "apiassignmentcanary",
            "livecredential123",
            "livecredential456",
            "liveexamplecredential",
            "jsoncredential789",
        ] {
            assert!(
                documentation_matches(&store, omitted).is_empty(),
                "secret-like candidate leaked {omitted}"
            );
        }
        assert_eq!(documentation_matches(&store, "rotationquartz"), ["prose"]);
        assert_eq!(
            documentation_matches(&store, "bearerwordquartz"),
            ["bearer_prose"]
        );
        assert_eq!(
            documentation_matches(&store, "placeholderquartz"),
            ["placeholder"]
        );
        let stats = store.concept_documentation_stats().unwrap();
        assert_eq!(stats.indexed_documents, 3);
        assert_eq!(stats.secret_omitted, 6);
        assert_eq!(
            store
                .meta_value(crate::store::CONCEPT_DOCUMENTATION_SECRET_OMITTED_META_KEY)
                .unwrap()
                .as_deref(),
            Some("6")
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn concept_documentation_utf8_and_file_caps_are_stable() {
        let (utf8_dir, utf8_db) = setup("concept_documentation_utf8");
        let long_doc = format!(
            "def bounded():\n    \"\"\"utf8quartz {}\"\"\"\n",
            "界".repeat(900)
        );
        fs::write(utf8_dir.join("bounded.py"), long_doc).unwrap();
        let mut utf8_store = Store::open(&utf8_db).unwrap();
        Indexer::new(&utf8_dir)
            .index_all(&mut utf8_store, true)
            .unwrap();
        assert_eq!(
            documentation_matches(&utf8_store, "utf8quartz"),
            ["bounded"]
        );
        assert_eq!(
            utf8_store
                .concept_documentation_stats()
                .unwrap()
                .size_omitted,
            1
        );
        let utf8_connection = rusqlite::Connection::open(&utf8_db).unwrap();
        let document_bytes: i64 = utf8_connection
            .query_row(
                "SELECT length(CAST(documentation_search AS BLOB))
                 FROM symbol_concepts WHERE documentation_search <> ''",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(document_bytes <= 512);
        drop(utf8_connection);
        fs::remove_dir_all(&utf8_dir).ok();

        let (aggregate_dir, aggregate_db) = setup("concept_documentation_aggregate");
        let alpha_word = |mut value: usize| {
            let mut word = String::from("w");
            for _ in 0..7 {
                word.push((b'a' + (value % 26) as u8) as char);
                value /= 26;
            }
            word
        };
        let mut source = String::new();
        for symbol in 0..150usize {
            source.push_str(&format!("/** aggregatequartz{symbol} "));
            for term in 0..48usize {
                source.push_str(&alpha_word(symbol * 64 + term));
                source.push(' ');
            }
            source.push_str(&format!("*/\nfunction item_{symbol}() {{}}\n"));
        }
        fs::write(aggregate_dir.join("aggregate.js"), source).unwrap();
        let mut aggregate_store = Store::open(&aggregate_db).unwrap();
        Indexer::new(&aggregate_dir)
            .index_all(&mut aggregate_store, true)
            .unwrap();
        let stats = aggregate_store.concept_documentation_stats().unwrap();
        assert!(stats.indexed_documents > 0 && stats.indexed_documents < 150);
        assert_eq!(
            stats.size_omitted,
            150u32.saturating_sub(stats.indexed_documents)
        );
        assert_eq!(
            documentation_matches(&aggregate_store, "aggregatequartz0"),
            ["item_0"]
        );
        assert!(documentation_matches(&aggregate_store, "aggregatequartz149").is_empty());
        fs::remove_dir_all(&aggregate_dir).ok();
    }

    #[test]
    fn concept_documentation_comment_edit_rename_delete_and_failed_commit_are_atomic() {
        let (dir, db) = setup("concept_documentation_lifecycle");
        let original = dir.join("original.py");
        fs::write(
            &original,
            "def handler():\n    \"\"\"oldquartz behavior\"\"\"\n",
        )
        .unwrap();
        let mut store = Store::open(&db).unwrap();
        let indexer = Indexer::new(&dir);
        indexer.index_all(&mut store, true).unwrap();
        assert_eq!(documentation_matches(&store, "oldquartz"), ["handler"]);

        fs::write(
            &original,
            "def handler():\n    \"\"\"watchquartz behavior\"\"\"\n",
        )
        .unwrap();
        indexer.index_one(&mut store, &original).unwrap();
        assert!(documentation_matches(&store, "oldquartz").is_empty());
        assert_eq!(documentation_matches(&store, "watchquartz"), ["handler"]);

        let renamed = dir.join("renamed.py");
        fs::rename(&original, &renamed).unwrap();
        indexer.index_all(&mut store, false).unwrap();
        let renamed_hits = store.search_concepts("\"watchquartz\"", 10).unwrap();
        assert_eq!(renamed_hits.len(), 1);
        assert_eq!(renamed_hits[0].path, "renamed.py");

        fs::remove_file(&renamed).unwrap();
        indexer.index_all(&mut store, false).unwrap();
        assert!(documentation_matches(&store, "watchquartz").is_empty());

        let failed = dir.join("failed.py");
        fs::write(
            &failed,
            "def failed():\n    \"\"\"stablequartz behavior\"\"\"\n",
        )
        .unwrap();
        indexer.index_all(&mut store, true).unwrap();
        fs::write(
            &failed,
            "def failed():\n    \"\"\"unstablequartz behavior\"\"\"\n",
        )
        .unwrap();
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_concept_documentation_commit
                 BEFORE INSERT ON symbols
                 WHEN new.file_path = 'failed.py' BEGIN
                     SELECT RAISE(ABORT, 'forced documentation commit failure');
                 END;",
            )
            .unwrap();
        drop(connection);
        assert!(indexer.index_one(&mut store, &failed).is_err());
        assert!(!store.concept_contract_current().unwrap());
        assert_eq!(
            crate::mcp::build_concept_current(&mut store, "stablequartz", 10),
            Err(crate::queries::ConceptError::IndexStale)
        );
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch("DROP TRIGGER fail_concept_documentation_commit")
            .unwrap();
        drop(connection);
        indexer.index_all(&mut store, true).unwrap();
        assert!(documentation_matches(&store, "stablequartz").is_empty());
        assert_eq!(documentation_matches(&store, "unstablequartz"), ["failed"]);
        fs::remove_dir_all(&dir).ok();
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
    fn cancellation_stops_indexing_before_derived_state_changes() {
        let (dir, db) = setup("cancel_before_index");
        fs::write(dir.join("a.py"), "def foo(): pass\n").unwrap();

        let mut store = Store::open(&db).unwrap();
        let indexer = Indexer::new(&dir);
        indexer.index_all(&mut store, false).unwrap();
        fs::write(dir.join("b.py"), "def bar(): pass\n").unwrap();

        store.cancel_handle().cancel();
        let error = indexer.index_all(&mut store, false).unwrap_err();
        assert!(matches!(error, IndexError::Cancelled));
        assert_eq!(
            store.take_interrupt_source(),
            Some(crate::store::InterruptSource::Cancel)
        );
        assert_eq!(store.file_count().unwrap(), 1);

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
    fn cancelled_forced_rebuild_leaves_concept_contract_dirty() {
        let (dir, db) = setup("cancel_forced_concept_rebuild");
        fs::write(dir.join("a.py"), "def alpha_handler(): pass\n").unwrap();
        fs::write(dir.join("b.py"), "def beta_handler(): pass\n").unwrap();

        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();
        assert!(store.concept_contract_current().unwrap());
        drop(store);

        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute(
                "UPDATE symbol_concepts SET name_search = 'corrupted'
                 WHERE symbol_id = (SELECT MIN(symbol_id) FROM symbol_concepts)",
                [],
            )
            .unwrap();
        drop(connection);

        let mut store = Store::open(&db).unwrap();
        assert!(store.concept_contract_current().unwrap());
        let cancel = store.cancel_handle();
        let indexer = Indexer::new(&dir).with_after_file_commit(move || cancel.cancel());
        let error = indexer.index_all(&mut store, true).unwrap_err();
        assert!(matches!(error, IndexError::Cancelled));
        assert_eq!(
            store.take_interrupt_source(),
            Some(crate::store::InterruptSource::Cancel)
        );
        assert!(!store.concept_contract_current().unwrap());
        drop(store);

        let mut custom = Store::open_for_serve(&db, None).unwrap();
        assert_eq!(
            crate::mcp::build_concept_current(&mut custom, "handler", 10),
            Err(crate::queries::ConceptError::IndexStale)
        );

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
        let adr_dir = dir.join("docs/adr/accepted");
        fs::create_dir_all(&adr_dir).unwrap();
        fs::write(
            adr_dir.join("004-storage.md"),
            "# Storage boundary\n\nThe ledger owns durable payment state.\n",
        )
        .unwrap();
        fs::write(adr_dir.join("draft.txt"), "not a durable Markdown ADR\n").unwrap();
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
        assert_eq!(stats.history_entries_indexed, 8);
        assert_eq!(stats.history_entries_skipped, 1);
        assert!(!stats.history_entries_truncated);
        assert_eq!(store.project_history_count().unwrap(), 8);
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
        assert_eq!(
            store
                .search_project_history("durable payment state", Some("architecture_decision"), 10,)
                .unwrap()
                .len(),
            1
        );

        fs::remove_file(task_dir.join("audit.md")).unwrap();
        let stats = indexer.index_all(&mut store, false).unwrap();
        assert_eq!(stats.history_entries_indexed, 7);
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
            Some(crate::hex::encode(&Sha256::digest(bytes)).as_str())
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn index_all_refuses_to_retarget_an_existing_database() {
        let (indexed_root, db) = setup("root_identity_source");
        fs::write(indexed_root.join("app.py"), "def source(): pass\n").unwrap();
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&indexed_root)
            .index_all(&mut store, false)
            .unwrap();

        let (requested_root, _) = setup("root_identity_target");
        fs::write(requested_root.join("app.py"), "def target(): pass\n").unwrap();
        let error = Indexer::new(&requested_root)
            .index_all(&mut store, false)
            .unwrap_err();
        assert!(error.to_string().contains("index belongs to"));
        assert_eq!(
            store.meta_value("index_root").unwrap().as_deref(),
            Some(
                indexed_root
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .as_ref()
            )
        );

        fs::remove_dir_all(indexed_root).ok();
        fs::remove_dir_all(requested_root).ok();
    }

    #[test]
    fn history_index_refuses_to_bind_an_unbound_scratchpad_database() {
        let (original_root, db) = setup("scratchpad_root_identity_source");
        let mut store = Store::open(&db).unwrap();
        store
            .scratchpad_append("planner", "handoff", "belongs to original repo")
            .unwrap();

        let (requested_root, _) = setup("scratchpad_root_identity_target");
        let error = Indexer::new(&requested_root)
            .index_project_history(&mut store)
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("existing index has no repository identity"));
        assert!(store.meta_value("index_root").unwrap().is_none());
        assert_eq!(
            store
                .scratchpad_read(None, None, None, 10)
                .unwrap()
                .first()
                .map(|entry| entry.body.as_str()),
            Some("belongs to original repo")
        );

        fs::remove_dir_all(original_root).ok();
        fs::remove_dir_all(requested_root).ok();
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
    fn frozen_v3_partial_and_successful_writes_cannot_leave_concepts_falsely_current() {
        for old_run_completed in [false, true] {
            let suffix = if old_run_completed {
                "complete"
            } else {
                "cancelled"
            };
            let (dir, db) = setup(&format!("concept_old_writer_{suffix}"));
            fs::write(dir.join("app.py"), "def current_handler(): pass\n").unwrap();
            let mut store = Store::open(&db).unwrap();
            let indexer = Indexer::new(&dir);
            indexer.index_all(&mut store, false).unwrap();
            assert!(store.concept_contract_current().unwrap());
            drop(store);

            // Frozen schema-v7/v3 behavior: replace one file's graph rows but
            // do not know about symbol_concepts. A cancelled run leaves the
            // future extractor marker untouched; a successful old run stamps
            // v3. Persistent schema-v7 triggers must dirty concepts in both.
            let connection = rusqlite::Connection::open(&db).unwrap();
            connection
                .execute_batch("PRAGMA foreign_keys = ON; BEGIN")
                .unwrap();
            connection
                .execute("DELETE FROM symbols WHERE file_path = 'app.py'", [])
                .unwrap();
            connection
                .execute(
                    "INSERT INTO symbols(name, kind, file_path, line_start, line_end)
                     VALUES ('legacy_secret', 'function', 'app.py', 1, 1)",
                    [],
                )
                .unwrap();
            if old_run_completed {
                connection
                    .execute(
                        "INSERT INTO meta(key, value) VALUES (?1, 'mmcg-extractors-v3')
                         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                        [EXTRACTOR_CONTRACT_META_KEY],
                    )
                    .unwrap();
            }
            connection.execute_batch("COMMIT").unwrap();
            drop(connection);

            let mut store = Store::open(&db).unwrap();
            assert!(!store.concept_contract_current().unwrap());
            assert_eq!(
                store.extractor_contract_current().unwrap(),
                !old_run_completed
            );
            let stats = indexer.index_all(&mut store, false).unwrap();
            assert!(stats.concept_contract_rebuilt);
            assert_eq!(stats.extractor_contract_rebuilt, old_run_completed);
            assert!(store.concept_contract_current().unwrap());
            assert!(store.extractor_contract_current().unwrap());
            assert_eq!(
                store.concept_count().unwrap(),
                store.symbol_count().unwrap()
            );
            assert!(store.search_concepts("\"legacy\"", 10).unwrap().is_empty());
            assert!(!store.search_concepts("\"current\"", 10).unwrap().is_empty());

            fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn deleted_path_purge_failure_is_fatal_and_cannot_stamp_concepts_current() {
        let (dir, db) = setup("concept_purge_failure");
        fs::write(dir.join("keep.py"), "def keep(): pass\n").unwrap();
        fs::write(dir.join("deleted.py"), "def stale_secret(): pass\n").unwrap();
        let mut store = Store::open(&db).unwrap();
        let indexer = Indexer::new(&dir);
        indexer.index_all(&mut store, false).unwrap();
        drop(store);

        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_deleted_file_purge
                 BEFORE DELETE ON files
                 WHEN old.path = 'deleted.py' BEGIN
                     INSERT INTO missing_concept_purge_sink(value)
                     VALUES (old.path);
                 END;",
            )
            .unwrap();
        drop(connection);
        fs::remove_file(dir.join("deleted.py")).unwrap();

        let mut store = Store::open(&db).unwrap();
        assert!(store.concept_schema_objects_current().unwrap());
        assert!(store.concept_contract_current().unwrap());
        let error = indexer.index_all(&mut store, false).unwrap_err();
        assert!(error.to_string().contains("missing_concept_purge_sink"));
        assert!(!store.concept_contract_current().unwrap());
        assert!(store
            .indexed_paths()
            .unwrap()
            .contains(&"deleted.py".to_string()));
        assert!(!store.search_concepts("\"stale\"", 10).unwrap().is_empty());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn schema_rebuild_allows_in_place_reindex_with_retained_scratchpad() {
        let (dir, db) = setup("schema_rebuild_in_place");
        fs::write(dir.join("app.py"), "def current(): pass\n").unwrap();
        {
            let mut store = Store::open(&db).unwrap();
            Indexer::new(&dir).index_all(&mut store, false).unwrap();
            store
                .scratchpad_append("planner", "handoff", "retain me")
                .unwrap();
            store.set_meta("schema_version", "6").unwrap();
        }

        let mut store = Store::open(&db).unwrap();
        assert_eq!(store.file_count().unwrap(), 0);
        let stats = Indexer::new(&dir).index_all(&mut store, false).unwrap();
        assert_eq!(stats.files_indexed, 1);
        assert_eq!(store.file_count().unwrap(), 1);
        assert_eq!(
            store
                .scratchpad_read(None, None, None, 10)
                .unwrap()
                .first()
                .map(|entry| entry.body.as_str()),
            Some("retain me")
        );
        fs::remove_dir_all(dir).ok();
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
        assert_eq!(stats.skipped_binary_paths, vec!["binary.rs"]);
        assert_eq!(stats.skipped_too_large_paths, vec!["large.rs"]);
        assert_eq!(stats.files_failed, 0);
        assert_eq!(store.indexed_paths().unwrap(), vec!["good.rs"]);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tracked_ignored_source_is_indexed_but_untracked_ignored_source_is_not() {
        let (dir, db) = setup("tracked_ignored_source");
        fs::write(dir.join(".gitignore"), "*.rs\n").unwrap();
        fs::write(dir.join("tracked.rs"), "pub fn tracked() {}\n").unwrap();
        fs::write(dir.join("untracked.rs"), "pub fn untracked() {}\n").unwrap();
        git(&dir, &["init", "-q", "--initial-branch=main"]);
        git(&dir, &["add", "-f", ".gitignore", "tracked.rs"]);

        let mut store = Store::open(&db).unwrap();
        let stats = Indexer::new(&dir).index_all(&mut store, false).unwrap();

        assert_eq!(stats.files_indexed, 1);
        assert_eq!(store.indexed_paths().unwrap(), vec!["tracked.rs"]);
        let mut matcher = SourceMatcher::new(&dir);
        assert!(!matcher.is_ignored(&dir.join("tracked.rs"), false));
        assert!(matcher.is_ignored(&dir.join("untracked.rs"), false));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn utf16le_source_with_bom_is_decoded_and_indexed() {
        let (dir, db) = setup("utf16le_source");
        let mut bytes = vec![0xff, 0xfe];
        for unit in "int unicode_source() { return 1; }\r\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        fs::write(dir.join("unicode.cpp"), bytes).unwrap();

        let mut store = Store::open(&db).unwrap();
        let stats = Indexer::new(&dir).index_all(&mut store, false).unwrap();

        assert_eq!(stats.files_skipped_binary, 0);
        assert_eq!(stats.files_indexed, 1);
        assert!(store
            .search_symbols("unicode_source", Some("function"), Some("cpp"))
            .unwrap()
            .iter()
            .any(|symbol| symbol.file_path == "unicode.cpp"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn source_extensions_are_matched_case_insensitively() {
        let (dir, db) = setup("case_insensitive_extensions");
        fs::write(dir.join("Legacy.CPP"), "int LegacyEntry() { return 0; }\n").unwrap();

        let mut store = Store::open(&db).unwrap();
        let stats = Indexer::new(&dir).index_all(&mut store, false).unwrap();

        assert_eq!(stats.files_indexed, 1);
        assert_eq!(store.indexed_paths().unwrap(), vec!["Legacy.CPP"]);
        assert_eq!(
            store
                .search_symbols("LegacyEntry", Some("function"), Some("cpp"))
                .unwrap()
                .len(),
            1
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn python_type_stubs_are_indexed_as_python() {
        let (dir, db) = setup("python_type_stub");
        fs::write(
            dir.join("contracts.pyi"),
            "class Service:\n    def execute(self, value: int) -> str: ...\n",
        )
        .unwrap();

        let mut store = Store::open(&db).unwrap();
        let stats = Indexer::new(&dir).index_all(&mut store, false).unwrap();
        assert_eq!(stats.files_indexed, 1);
        assert!(store
            .search_symbols("Service", Some("class"), Some("python"))
            .unwrap()
            .iter()
            .any(|symbol| symbol.file_path == "contracts.pyi"));
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

    #[cfg(unix)]
    #[test]
    fn source_admission_rejects_symlink_and_fifo_without_following() {
        use std::os::unix::fs::symlink;

        let (dir, _db) = setup("source_admission_nofollow");
        let outside = env::temp_dir().join(format!(
            "mmcg-source-admission-outside-{}",
            std::process::id()
        ));
        fs::write(&outside, "fn secret() {}\n").unwrap();
        let linked = dir.join("linked.rs");
        symlink(&outside, &linked).unwrap();
        assert!(source_admission_controlled(&dir, &linked, ReadControl::default()).is_err());

        let fifo = dir.join("pipe.rs");
        let status = Command::new("mkfifo").arg(&fifo).status().unwrap();
        assert!(status.success());
        assert!(source_admission_controlled(&dir, &fifo, ReadControl::default()).is_err());
        fs::remove_file(outside).ok();
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn auto_refresh_candidate_limit_is_fail_closed() {
        let (dir, _db) = setup("auto_refresh_candidate_limit");
        git(&dir, &["init"]);
        fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
        fs::write(dir.join("b.rs"), "fn b() {}\n").unwrap();
        git(&dir, &["add", "a.rs", "b.rs"]);
        let db = dir.join(".mastermind/mmcg.db");
        let mut store = Store::open(&db).unwrap();
        let error = Indexer::new(&dir)
            .index_all_with_limits(
                &mut store,
                false,
                IndexLimits {
                    source_candidates: Some(1),
                    source_declared_bytes: Some(u64::MAX),
                },
            )
            .unwrap_err();
        assert!(matches!(
            error,
            IndexError::LimitExceeded {
                dimension: "source_candidates",
                cap: 1
            }
        ));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn auto_refresh_aggregate_limit_is_fail_closed() {
        let (dir, _db) = setup("auto_refresh_aggregate_limit");
        git(&dir, &["init"]);
        fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
        git(&dir, &["add", "a.rs"]);
        let db = dir.join(".mastermind/mmcg.db");
        let mut store = Store::open(&db).unwrap();
        let error = Indexer::new(&dir)
            .index_all_with_limits(
                &mut store,
                false,
                IndexLimits {
                    source_candidates: Some(100),
                    source_declared_bytes: Some(1),
                },
            )
            .unwrap_err();
        assert!(matches!(
            error,
            IndexError::LimitExceeded {
                dimension: "source_declared_bytes",
                cap: 1
            }
        ));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn automatic_parse_rejects_a_file_replaced_after_aggregate_admission() {
        let (dir, _db) = setup("aggregate_admission_identity");
        let path = dir.join("source.rs");
        fs::write(&path, "pub fn admitted() {}\n").unwrap();
        let root = RootCapability::open(&dir).unwrap();
        let admitted =
            source_admission_with_capability(&root, &path, ReadControl::default()).unwrap();

        fs::write(&path, "pub fn replacement() {\n    let expanded = 1;\n}\n").unwrap();
        let extractor = extractor_for_path(&path).unwrap();
        assert!(matches!(
            parse_one_controlled(
                &path,
                &dir,
                &root,
                admitted.identity,
                extractor.as_ref(),
                ReadControl::default(),
            ),
            Err(IndexError::SnapshotChanged)
        ));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn history_freshness_detects_markdown_only_edit_add_rename_and_delete() {
        let (dir, _db) = setup("history_freshness");
        fs::create_dir_all(dir.join(".mastermind/tasks")).unwrap();
        let context = dir.join("CONTEXT.md");
        fs::write(&context, "# Context\ninitial\n").unwrap();
        let db = dir.join(".mastermind/mmcg.db");
        let mut store = Store::open(&db).unwrap();
        let indexer = Indexer::new(&dir);
        indexer.index_project_history(&mut store).unwrap();
        assert_eq!(
            indexer.project_history_freshness(&store).unwrap(),
            ProjectHistoryFreshness::Fresh
        );

        fs::write(&context, "# Context\nedited\n").unwrap();
        assert_eq!(
            indexer.project_history_freshness(&store).unwrap(),
            ProjectHistoryFreshness::Stale
        );
        indexer.index_project_history(&mut store).unwrap();
        let archive = dir.join("CONTEXT-archive-1.md");
        fs::write(&archive, "# Archive\nadded\n").unwrap();
        assert_eq!(
            indexer.project_history_freshness(&store).unwrap(),
            ProjectHistoryFreshness::Stale
        );
        indexer.index_project_history(&mut store).unwrap();
        let renamed = dir.join("CONTEXT-archive-2.md");
        fs::rename(&archive, &renamed).unwrap();
        assert_eq!(
            indexer.project_history_freshness(&store).unwrap(),
            ProjectHistoryFreshness::Stale
        );
        indexer.index_project_history(&mut store).unwrap();
        fs::remove_file(renamed).unwrap();
        assert_eq!(
            indexer.project_history_freshness(&store).unwrap(),
            ProjectHistoryFreshness::Stale
        );
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
    /// Declaration text from the node start up to its body — the signature as
    /// written, minus the opening brace. Languages whose declarations can end
    /// in something other than a body (Rust `;`, C# `=>`) specialise this.
    pub fn signature_until_body(node: &Node, source: &[u8]) -> Option<String> {
        let body = node.child_by_field_name("body")?;
        let header_end = body.start_byte();
        let start = node.start_byte();
        if header_end <= start {
            return None;
        }
        let text = std::str::from_utf8(&source[start..header_end]).ok()?;
        let trimmed = text
            .trim_end_matches(|c: char| c == '{' || c.is_whitespace())
            .to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }

    /// Per-language temp file for extractor tests. `lang` keeps concurrently
    /// running language suites from sharing a directory within one test binary.
    #[cfg(test)]
    pub fn write_tmp(lang: &str, name: &str, content: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("mmcg-{lang}-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }
}
