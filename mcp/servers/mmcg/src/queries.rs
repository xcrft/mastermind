//! High-level query layer over the Store.
//!
//! Wraps raw store methods with name-based lookup, structured response types,
//! and JSON serialization for the MCP layer.

use crate::store::{FileEntry, MapBoundaryMatch, MapBoundaryScope, Store, Symbol, TaskSpecHit};
use serde::Serialize;
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct SymbolHit {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Extra locations when this hit collapses several declarations of one
    /// symbol (e.g. C# partial classes across files). `file`/`line` still point
    /// to the canonical (lex-first) declaration; this list includes every
    /// declaration, canonical one included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<SymbolLocation>>,
    /// Decorators / attributes / modifiers from source (e.g. `",Fact,"`,
    /// `",partial,sealed,"`). Skipped from output when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decorators: Option<String>,
    /// Graph-edge precision for this symbol's language. Present on `mmcg_search`
    /// results; absent on sub-lists (callers, callees), where the parent response
    /// carries a single `edge_precision`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precision: Option<EdgePrecision>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolLocation {
    pub file: String,
    pub line: u32,
}

impl From<Symbol> for SymbolHit {
    fn from(s: Symbol) -> Self {
        Self {
            name: s.name,
            kind: s.kind,
            file: s.file_path,
            line: s.line_start,
            signature: s.signature,
            locations: None,
            decorators: s.decorators,
            precision: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub results: Vec<SymbolHit>,
}

#[derive(Debug, Serialize)]
pub struct CallersResponse {
    pub target: String,
    pub count: u32,
    /// How many definitions share `target`'s name. Edges resolve by name, so
    /// > 1 means these callers pool across several same-named symbols.
    pub name_collision: u32,
    pub callers: Vec<SymbolHit>,
}

/// Confidence and resolution metadata for a set of graph edges.
///
/// Precision depends on language: Rust and Go are syntactic, high-confidence;
/// Python and JavaScript heuristic (leaf-name only, no type inference). C/C++ is
/// syntactic but inherently low-confidence — macros unexpanded, includes unfollowed.
#[derive(Debug, Clone, Serialize)]
pub struct EdgePrecision {
    /// `"high"`, `"medium"`, or `"low"`.
    pub confidence: &'static str,
    /// `"syntactic"` — straight from AST; `"heuristic"` — leaf-name guessing
    /// without type resolution.
    pub resolution: &'static str,
    /// Known gaps for this language's edge extraction.
    pub limitations: Vec<&'static str>,
}

/// Edge precision derived from a file path's extension.
pub fn lang_precision(file_path: &str) -> EdgePrecision {
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "rs" => EdgePrecision {
            confidence: "high",
            resolution: "syntactic",
            limitations: vec!["trait-object dynamic dispatch not resolved"],
        },
        "go" => EdgePrecision {
            confidence: "high",
            resolution: "syntactic",
            limitations: vec![],
        },
        "java" => EdgePrecision {
            confidence: "high",
            resolution: "syntactic",
            limitations: vec!["reflection not tracked", "generics erased at call sites"],
        },
        "cs" => EdgePrecision {
            confidence: "high",
            resolution: "syntactic",
            limitations: vec!["reflection not tracked"],
        },
        "py" => EdgePrecision {
            confidence: "medium",
            resolution: "heuristic",
            limitations: vec![
                "obj.method() matched by leaf name only — no type inference",
                "dynamic attributes not tracked",
            ],
        },
        "ts" | "tsx" => EdgePrecision {
            confidence: "medium",
            resolution: "syntactic",
            limitations: vec![
                "no type-based dispatch resolution",
                "dynamic imports not tracked",
            ],
        },
        "js" | "jsx" | "mjs" | "cjs" => EdgePrecision {
            confidence: "medium",
            resolution: "heuristic",
            limitations: vec!["no type resolution", "dynamic calls not tracked"],
        },
        "php" | "phtml" => EdgePrecision {
            confidence: "medium",
            resolution: "syntactic",
            limitations: vec!["dynamic dispatch not tracked"],
        },
        "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hh" | "hxx" | "ipp" | "tpp" => EdgePrecision {
            confidence: "low",
            resolution: "syntactic",
            limitations: vec![
                "macros not expanded",
                "includes not followed",
                "overload resolution absent",
            ],
        },
        _ => EdgePrecision {
            confidence: "unknown",
            resolution: "unknown",
            limitations: vec!["unsupported or unrecognized language"],
        },
    }
}

fn lang_from_ext(ext: &str) -> &'static str {
    match ext {
        "rs" => "rust",
        "go" => "go",
        "java" => "java",
        "cs" => "csharp",
        "py" => "python",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "php" | "phtml" => "php",
        "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hh" | "hxx" | "ipp" | "tpp" => "cpp",
        _ => "unknown",
    }
}

#[derive(Debug, Serialize)]
pub struct CalleesEntry {
    pub name: String,
    pub line: u32,
}

#[derive(Debug, Serialize)]
pub struct CalleesResponse {
    pub symbol: String,
    pub matched: Option<SymbolHit>,
    pub count: u32,
    pub callees: Vec<CalleesEntry>,
    /// Edge precision for calls made by this symbol, from its language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_precision: Option<EdgePrecision>,
}

/// A matched symbol with debug metadata for `mmcg query explain`.
#[derive(Debug, Serialize)]
pub struct ExplainSymbol {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub language: &'static str,
}

/// Full debug output for `mmcg query explain <name>`: matched symbol IDs, files,
/// edge counts, source-language precision, and known limitations.
#[derive(Debug, Serialize)]
pub struct ExplainResponse {
    pub query: String,
    /// Every raw symbol row matching the query (before partial-class collapse).
    pub matched: Vec<ExplainSymbol>,
    /// Direct callers of the first match.
    pub caller_count: u32,
    /// Direct callees of the first match.
    pub callee_count: u32,
    /// Edge precision from the first matched symbol's language.
    pub edge_precision: EdgePrecision,
    /// Present when multiple partial-class rows share the name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collapse_note: Option<String>,
    /// Human-readable limitations — same content as `edge_precision.limitations`.
    pub limitations: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ImpactEntry {
    pub symbol: SymbolHit,
    pub depth: u32,
}

#[derive(Debug, Serialize)]
pub struct ImpactResponse {
    pub target: String,
    pub max_depth: u32,
    pub count: u32,
    /// How many definitions share `target`'s name (same caveat as
    /// `CallersResponse`): > 1 means the blast radius pools across same-named
    /// symbols and over-approximates real reach.
    pub name_collision: u32,
    pub impact: Vec<ImpactEntry>,
}

#[derive(Debug, Serialize)]
pub struct FilesResponse {
    pub prefix: Option<String>,
    pub count: u32,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub db_path: String,
    pub symbol_count: u32,
    pub file_count: u32,
    /// Indexable source paths that are added, deleted, or newer than the indexed
    /// snapshot (capped at 100). A non-zero count means structural answers must
    /// not be trusted before re-indexing.
    pub stale_files: usize,
    /// False when extractor semantics changed after the stored index was built.
    pub extractor_contract_current: bool,
}

pub fn search(
    store: &Store,
    name: &str,
    kind: Option<&str>,
    language: Option<&str>,
    collapse_partials: bool,
) -> rusqlite::Result<SearchResponse> {
    let raw = store.search_symbols(name, kind, language)?;
    let mut results: Vec<SymbolHit> = if collapse_partials {
        collapse_partial_hits(raw)
    } else {
        raw.into_iter().map(SymbolHit::from).collect()
    };
    for hit in &mut results {
        hit.precision = Some(lang_precision(&hit.file));
    }
    Ok(SearchResponse {
        query: name.to_string(),
        results,
    })
}

/// Collapse multiple Symbol rows for the same partial-class declaration into one
/// hit. A row is "partial" when its decorators contain `,partial,` (set by the
/// C# extractor for `partial class` / `partial record`).
///
/// Non-partial rows pass through unchanged, even when several share a name: two
/// non-partial same-named classes (unusual but possible across namespaces)
/// deserve to be distinct hits.
///
/// Canonical hit is lex-first by file path; its `locations` lists every
/// declaration (including itself).
fn collapse_partial_hits(symbols: Vec<Symbol>) -> Vec<SymbolHit> {
    use std::collections::HashMap;

    // Group key: (name, kind). Language omitted — partials are C#-only and SQL
    // filters by language upstream.
    let mut groups: HashMap<(String, String), Vec<Symbol>> = HashMap::new();
    let mut order: Vec<(String, String)> = Vec::new();
    let mut passthrough: Vec<SymbolHit> = Vec::new();

    for sym in symbols {
        let is_partial = sym
            .decorators
            .as_deref()
            .map(|d| d.contains(",partial,"))
            .unwrap_or(false);
        if !is_partial {
            passthrough.push(SymbolHit::from(sym));
            continue;
        }
        let key = (sym.name.clone(), sym.kind.clone());
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(sym);
    }

    let mut out: Vec<SymbolHit> = Vec::with_capacity(passthrough.len() + order.len());
    out.extend(passthrough);
    for key in order {
        let mut rows = groups.remove(&key).unwrap();
        rows.sort_by(|a, b| {
            a.file_path
                .cmp(&b.file_path)
                .then(a.line_start.cmp(&b.line_start))
        });
        let canonical = rows[0].clone();
        let locations: Vec<SymbolLocation> = rows
            .iter()
            .map(|s| SymbolLocation {
                file: s.file_path.clone(),
                line: s.line_start,
            })
            .collect();
        let mut hit = SymbolHit::from(canonical);
        hit.locations = Some(locations);
        out.push(hit);
    }
    out
}

#[derive(Debug, Serialize)]
pub struct CentralityHit {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub in_degree: u32,
    pub name_collision: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decorators: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CentralityResponse {
    pub count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    pub top: u32,
    pub results: Vec<CentralityHit>,
}

#[derive(Debug, Serialize)]
pub struct TaskSearchResponse {
    pub query: String,
    pub count: u32,
    pub results: Vec<TaskSpecHit>,
}

pub fn tasks(store: &Store, query: &str, top: u32) -> rusqlite::Result<TaskSearchResponse> {
    let results = store.search_task_specs(query, top)?;
    Ok(TaskSearchResponse {
        query: query.to_string(),
        count: results.len() as u32,
        results,
    })
}

#[derive(Debug, Serialize)]
pub struct DependencyCyclesResponse {
    pub count: u32,
    pub min_size: u32,
    /// Each entry is one cycle (SCC) — file paths in lex order.
    pub cycles: Vec<Vec<String>>,
}

pub fn symbols_changed_since(
    store: &Store,
    repo_root: &std::path::Path,
    git_ref: &str,
) -> Result<crate::diff::SymbolDiff, crate::diff::DiffError> {
    crate::diff::symbols_changed_since(store, repo_root, git_ref)
}

pub const CHANGE_SEED_LIMIT: usize = 200;
pub const IMPACT_WORK_LIMIT: usize = 5_001;

#[derive(Debug, Clone, Serialize)]
pub struct Collection<T> {
    pub total: Option<u32>,
    pub returned: u32,
    pub truncated: bool,
    pub truncation_reason: Option<String>,
    pub items: Vec<T>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangeImpactResponse {
    pub schema_version: u32,
    pub baseline: ImpactBaseline,
    pub scope: ImpactScope,
    pub changes: ImpactChanges,
    pub affected_components: Collection<ComponentImpact>,
    pub impact: Collection<ImpactedSymbol>,
    pub api_crossings: Collection<ApiCrossing>,
    pub tests: Collection<TestCandidate>,
    pub limits: ImpactLimits,
    pub precision_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactBaseline {
    pub requested_ref: String,
    pub baseline_oid: String,
    pub head_oid: String,
    pub includes_worktree: bool,
    pub includes_untracked: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactScope {
    pub repository_relative_root: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactChanges {
    pub files: Collection<ChangedFile>,
    pub symbols: Collection<ChangedSymbol>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChangedSymbol {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub change: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComponentImpact {
    pub component: String,
    pub changed_symbols: u32,
    pub impacted_symbols: u32,
    pub candidate_tests: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SymbolEvidence {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SeedEvidence {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub change: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactedSymbol {
    pub symbol: SymbolEvidence,
    pub minimum_depth: u32,
    pub seeds: Vec<SeedEvidence>,
    pub name_collision_count: u32,
    pub edge_precision: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiCrossing {
    pub seed: SeedEvidence,
    pub changed_component: String,
    pub impacted: SymbolEvidence,
    pub impacted_component: String,
    pub minimum_depth: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestCandidate {
    pub symbol: SymbolEvidence,
    pub classification: String,
    pub minimum_depth: Option<u32>,
    pub confidence: String,
    pub evidence: Vec<TestEvidence>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct TestEvidence {
    pub kind: String,
    pub seed: Option<SeedEvidence>,
    pub component: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactLimits {
    pub changed_files: u32,
    pub changed_seeds: u32,
    pub graph_rows: u32,
    pub impact: u32,
    pub tests: u32,
    pub crossings: u32,
    pub heuristic_paths: u32,
    pub max_depth: u32,
}

type ImpactGroupKey = (String, u32, String, String, i64);
type ImpactGroupValue = (Symbol, u32, BTreeSet<SeedEvidence>);

pub type ImpactEngine<'a> =
    dyn Fn(&Store, &Path, &str, u32, usize) -> Result<ChangeImpactResponse, ChangeImpactError> + 'a;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeImpactError {
    InvalidRef,
    RootMismatch,
    IndexStale,
    SnapshotChanged,
    GitTimeout,
    GitOutputLimit,
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImpactTestStage {
    BeforeDataVersionRecheck,
    BeforeGitSnapshotRecheck,
}

#[cfg(test)]
type ImpactTestHook = (ImpactTestStage, Box<dyn FnOnce()>);

#[cfg(test)]
type ImpactTestHookCell = RefCell<Option<ImpactTestHook>>;

#[cfg(test)]
thread_local! {
    static IMPACT_TEST_HOOK: ImpactTestHookCell = RefCell::new(None);
}

#[cfg(test)]
pub(crate) struct ImpactTestHookGuard;

#[cfg(test)]
impl Drop for ImpactTestHookGuard {
    fn drop(&mut self) {
        IMPACT_TEST_HOOK.with(|hook| hook.borrow_mut().take());
    }
}

#[cfg(test)]
pub(crate) fn install_impact_test_hook(
    stage: ImpactTestStage,
    hook: impl FnOnce() + 'static,
) -> ImpactTestHookGuard {
    IMPACT_TEST_HOOK.with(|slot| *slot.borrow_mut() = Some((stage, Box::new(hook))));
    ImpactTestHookGuard
}

#[cfg(test)]
fn run_impact_test_hook(stage: ImpactTestStage) {
    let hook = IMPACT_TEST_HOOK.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot
            .as_ref()
            .is_some_and(|(candidate, _)| *candidate == stage)
        {
            slot.take().map(|(_, hook)| hook)
        } else {
            None
        }
    });
    if let Some(hook) = hook {
        hook();
    }
}

impl ChangeImpactError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidRef => "invalid_ref",
            Self::RootMismatch => "root_mismatch",
            Self::IndexStale => "index_stale",
            Self::SnapshotChanged => "snapshot_changed",
            Self::GitTimeout => "git_timeout",
            Self::GitOutputLimit => "git_output_limit",
        }
    }
}

impl std::fmt::Display for ChangeImpactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for ChangeImpactError {}

impl From<crate::diff::WorkingTreeDiffError> for ChangeImpactError {
    fn from(error: crate::diff::WorkingTreeDiffError) -> Self {
        match error {
            crate::diff::WorkingTreeDiffError::InvalidRef
            | crate::diff::WorkingTreeDiffError::GitUnavailable => Self::InvalidRef,
            crate::diff::WorkingTreeDiffError::SnapshotChanged => Self::SnapshotChanged,
            crate::diff::WorkingTreeDiffError::GitTimeout => Self::GitTimeout,
            crate::diff::WorkingTreeDiffError::GitOutputLimit => Self::GitOutputLimit,
            crate::diff::WorkingTreeDiffError::IndexStale => Self::IndexStale,
        }
    }
}

fn owning_repository(path: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    canonical
        .ancestors()
        .find(|candidate| candidate.join(".git").exists())
        .map(Path::to_path_buf)
}

fn component_for(path: &str) -> String {
    path.split_once('/')
        .map(|(component, _)| component.to_string())
        .unwrap_or_else(|| ".".to_string())
}

fn visible_precision(path: &str) -> Vec<String> {
    let precision = lang_precision(path);
    let mut values = vec![format!("{}:{}", precision.confidence, precision.resolution)];
    values.extend(
        precision
            .limitations
            .into_iter()
            .map(|value| value.to_string()),
    );
    values.sort();
    values.dedup();
    values
}

fn test_like_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let basename = lower.rsplit('/').next().unwrap_or(&lower);
    lower
        .split('/')
        .any(|part| part == "test" || part == "tests" || part == "spec")
        || basename.starts_with("test_")
        || basename.ends_with("_test.rs")
        || basename.contains(".test.")
        || basename.contains(".spec.")
        || basename.ends_with("tests.rs")
}

fn test_symbol(symbol: &Symbol) -> bool {
    if !test_like_path(&symbol.file_path) {
        return false;
    }
    let lower_name = symbol.name.to_ascii_lowercase();
    let decorators = symbol.decorators.as_deref().unwrap_or("");
    let lifecycle = [
        "setup",
        "teardown",
        "setup_method",
        "teardown_method",
        "beforeeach",
        "aftereach",
        "beforeall",
        "afterall",
        "testinitialize",
        "testcleanup",
    ];
    if lifecycle.contains(&lower_name.as_str())
        || decorators.contains(",fixture,")
        || decorators.contains(",pytest.fixture,")
        || decorators.contains(",SetUp,")
        || decorators.contains(",TearDown,")
    {
        return false;
    }
    lower_name.starts_with("test")
        || matches!(lower_name.as_str(), "it" | "spec")
        || [
            ",test,",
            ",tokio::test,",
            ",async_std::test,",
            ",Fact,",
            ",Theory,",
            ",TestMethod,",
            ",TestCase,",
            ",ParameterizedTest,",
        ]
        .iter()
        .any(|marker| decorators.contains(marker))
}

fn symbol_evidence(symbol: &Symbol) -> SymbolEvidence {
    SymbolEvidence {
        file: symbol.file_path.clone(),
        name: symbol.name.clone(),
        kind: symbol.kind.clone(),
        line: symbol.line_start,
    }
}

fn bounded_collection<T>(mut items: Vec<T>, limit: usize, reason: &str) -> Collection<T> {
    let total = items.len();
    let truncated = total > limit;
    if truncated {
        items.truncate(limit);
    }
    Collection {
        total: Some(total as u32),
        returned: items.len() as u32,
        truncated,
        truncation_reason: truncated.then(|| reason.to_string()),
        items,
    }
}

fn work_limited_collection<T>(items: Vec<T>) -> Collection<T> {
    Collection {
        total: None,
        returned: items.len() as u32,
        truncated: true,
        truncation_reason: Some("work_limit".to_string()),
        items,
    }
}

pub fn change_impact(
    store: &Store,
    requested_root: &Path,
    git_ref: &str,
    max_depth: u32,
    top: usize,
) -> Result<ChangeImpactResponse, ChangeImpactError> {
    if !(1..=5).contains(&max_depth) || !(1..=500).contains(&top) {
        return Err(ChangeImpactError::InvalidRef);
    }
    let requested_root = requested_root
        .canonicalize()
        .map_err(|_| ChangeImpactError::RootMismatch)?;
    let repository_root =
        owning_repository(&requested_root).ok_or(ChangeImpactError::RootMismatch)?;
    let stored_root = store
        .meta_value("index_root")
        .map_err(|_| ChangeImpactError::IndexStale)?
        .ok_or(ChangeImpactError::IndexStale)?;
    let stored_root = PathBuf::from(stored_root)
        .canonicalize()
        .map_err(|_| ChangeImpactError::RootMismatch)?;
    if stored_root != repository_root {
        return Err(ChangeImpactError::RootMismatch);
    }
    let scope = requested_root
        .strip_prefix(&repository_root)
        .ok()
        .filter(|value| !value.as_os_str().is_empty())
        .map(|value| value.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| ".".to_string());

    let working = crate::diff::symbols_changed_in_worktree(store, &repository_root, git_ref)?;
    for file in &working.files {
        if file.status == "deleted"
            || crate::indexer::extractor_for_path(Path::new(&file.path)).is_none()
        {
            continue;
        }
        let bytes = std::fs::read(repository_root.join(&file.path))
            .map_err(|_| ChangeImpactError::SnapshotChanged)?;
        let digest = format!("{:x}", Sha256::digest(bytes));
        let stored = store
            .file_content_sha256(&file.path)
            .map_err(|_| ChangeImpactError::IndexStale)?;
        if stored.as_deref().filter(|value| !value.is_empty()) != Some(digest.as_str()) {
            return Err(ChangeImpactError::IndexStale);
        }
    }

    let mut changed_symbols = Vec::new();
    changed_symbols.extend(working.diff.added.iter().map(|symbol| ChangedSymbol {
        file: symbol.file.clone(),
        name: symbol.name.clone(),
        kind: symbol.kind.clone(),
        line: symbol.line,
        change: "added".to_string(),
    }));
    changed_symbols.extend(working.diff.removed.iter().map(|symbol| ChangedSymbol {
        file: symbol.file.clone(),
        name: symbol.name.clone(),
        kind: symbol.kind.clone(),
        line: symbol.line,
        change: "removed".to_string(),
    }));
    changed_symbols.extend(
        working
            .diff
            .signature_changed
            .iter()
            .map(|symbol| ChangedSymbol {
                file: symbol.file.clone(),
                name: symbol.name.clone(),
                kind: symbol.kind.clone(),
                line: symbol.new_line,
                change: "signature_changed".to_string(),
            }),
    );
    changed_symbols.extend(working.body_changed.iter().map(|symbol| ChangedSymbol {
        file: symbol.file.clone(),
        name: symbol.name.clone(),
        kind: symbol.kind.clone(),
        line: symbol.line,
        change: "body_changed".to_string(),
    }));
    changed_symbols.sort();
    changed_symbols.dedup();
    let seed_evidence: Vec<SeedEvidence> = changed_symbols
        .iter()
        .map(|symbol| SeedEvidence {
            file: symbol.file.clone(),
            name: symbol.name.clone(),
            kind: symbol.kind.clone(),
            line: symbol.line,
            change: symbol.change.clone(),
        })
        .collect();
    let seed_names: Vec<String> = seed_evidence
        .iter()
        .map(|seed| seed.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let data_version_before = store
        .data_version()
        .map_err(|_| ChangeImpactError::SnapshotChanged)?;
    store
        .begin_read_snapshot()
        .map_err(|_| ChangeImpactError::SnapshotChanged)?;

    let mut precision_notes = vec!["focused_tests_do_not_replace_full_gate".to_string()];
    let graph_seed_overflow = seed_names.len() > CHANGE_SEED_LIMIT;
    let graph_rows = if seed_names.is_empty() || graph_seed_overflow {
        Vec::new()
    } else {
        match store.impact_of_many(&seed_names, max_depth, IMPACT_WORK_LIMIT) {
            Ok(rows) => rows,
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::OperationInterrupted =>
            {
                precision_notes.push("graph_work_limit".to_string());
                Vec::new()
            }
            Err(_) => {
                let _ = store.end_read_snapshot();
                return Err(ChangeImpactError::SnapshotChanged);
            }
        }
    };
    let graph_overflow = graph_seed_overflow || graph_rows.len() >= IMPACT_WORK_LIMIT;
    if graph_seed_overflow {
        precision_notes.push("changed_seed_work_limit".to_string());
    } else if graph_rows.len() >= IMPACT_WORK_LIMIT {
        precision_notes.push("graph_work_limit".to_string());
    }

    let evidence_by_name: BTreeMap<String, Vec<SeedEvidence>> = {
        let mut grouped: BTreeMap<String, Vec<SeedEvidence>> = BTreeMap::new();
        for seed in &seed_evidence {
            grouped
                .entry(seed.name.clone())
                .or_default()
                .push(seed.clone());
        }
        grouped
    };
    let mut impact_grouped: BTreeMap<ImpactGroupKey, ImpactGroupValue> = BTreeMap::new();
    if !graph_overflow {
        for row in &graph_rows {
            let key = (
                row.symbol.file_path.clone(),
                row.symbol.line_start,
                row.symbol.name.clone(),
                row.symbol.kind.clone(),
                row.symbol.id,
            );
            let entry = impact_grouped
                .entry(key)
                .or_insert_with(|| (row.symbol.clone(), row.depth, BTreeSet::new()));
            entry.1 = entry.1.min(row.depth);
            if let Some(evidence) = evidence_by_name.get(&row.seed) {
                entry.2.extend(evidence.iter().cloned());
            }
        }
    }
    let mut impacts = Vec::new();
    for (_, (symbol, minimum_depth, seeds)) in impact_grouped {
        impacts.push(ImpactedSymbol {
            name_collision_count: store.definition_count(&symbol.name).unwrap_or(0),
            edge_precision: visible_precision(&symbol.file_path),
            symbol: symbol_evidence(&symbol),
            minimum_depth,
            seeds: seeds.into_iter().collect(),
        });
    }
    impacts.sort_by(|a, b| {
        (
            a.minimum_depth,
            &a.symbol.file,
            a.symbol.line,
            &a.symbol.name,
            &a.symbol.kind,
        )
            .cmp(&(
                b.minimum_depth,
                &b.symbol.file,
                b.symbol.line,
                &b.symbol.name,
                &b.symbol.kind,
            ))
    });

    let mut crossings = Vec::new();
    for impact in &impacts {
        let impacted_component = component_for(&impact.symbol.file);
        for seed in &impact.seeds {
            let changed_component = component_for(&seed.file);
            if changed_component != impacted_component {
                crossings.push(ApiCrossing {
                    seed: seed.clone(),
                    changed_component,
                    impacted: impact.symbol.clone(),
                    impacted_component: impacted_component.clone(),
                    minimum_depth: impact.minimum_depth,
                });
            }
        }
    }
    crossings.sort_by(|a, b| {
        (
            &a.seed.file,
            a.seed.line,
            &a.impacted.file,
            a.impacted.line,
            a.minimum_depth,
        )
            .cmp(&(
                &b.seed.file,
                b.seed.line,
                &b.impacted.file,
                b.impacted.line,
                b.minimum_depth,
            ))
    });

    let mut tests_by_symbol: BTreeMap<SymbolEvidence, TestCandidate> = BTreeMap::new();
    for changed in &changed_symbols {
        let indexed = store.symbols_in_file(&changed.file).unwrap_or_default();
        if let Some(symbol) = indexed.into_iter().find(|symbol| {
            symbol.name == changed.name && symbol.kind == changed.kind && test_symbol(symbol)
        }) {
            let evidence = SeedEvidence {
                file: changed.file.clone(),
                name: changed.name.clone(),
                kind: changed.kind.clone(),
                line: changed.line,
                change: changed.change.clone(),
            };
            let symbol = symbol_evidence(&symbol);
            tests_by_symbol.insert(
                symbol.clone(),
                TestCandidate {
                    symbol,
                    classification: "direct".to_string(),
                    minimum_depth: Some(0),
                    confidence: "high".to_string(),
                    evidence: vec![TestEvidence {
                        kind: "changed_test_symbol".to_string(),
                        seed: Some(evidence),
                        component: None,
                    }],
                },
            );
        }
    }
    if !graph_overflow {
        for row in &graph_rows {
            if !test_symbol(&row.symbol) {
                continue;
            }
            let symbol = symbol_evidence(&row.symbol);
            let candidate = tests_by_symbol
                .entry(symbol.clone())
                .or_insert(TestCandidate {
                    symbol,
                    classification: if row.depth == 1 {
                        "direct"
                    } else {
                        "transitive"
                    }
                    .to_string(),
                    minimum_depth: Some(row.depth),
                    confidence: if row.depth == 1 { "high" } else { "medium" }.to_string(),
                    evidence: Vec::new(),
                });
            if candidate.minimum_depth != Some(0) {
                candidate.minimum_depth =
                    Some(candidate.minimum_depth.unwrap_or(row.depth).min(row.depth));
                if candidate.minimum_depth == Some(1) {
                    candidate.classification = "direct".to_string();
                    candidate.confidence = "high".to_string();
                }
            }
            if let Some(seeds) = evidence_by_name.get(&row.seed) {
                candidate
                    .evidence
                    .extend(seeds.iter().cloned().map(|seed| TestEvidence {
                        kind: "graph_seed".to_string(),
                        seed: Some(seed),
                        component: None,
                    }));
            }
        }
    }

    let affected_component_names: Vec<String> = changed_symbols
        .iter()
        .map(|symbol| component_for(&symbol.file))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let heuristic_paths = store
        .scoped_paths_in_components(&affected_component_names, 50_001)
        .map_err(|_| ChangeImpactError::SnapshotChanged)?;
    let path_probe_overflow = heuristic_paths.len() > 50_000;
    let heuristic_rows = if path_probe_overflow {
        Vec::new()
    } else {
        store
            .test_symbols_in_components(&affected_component_names, 501)
            .map_err(|_| ChangeImpactError::SnapshotChanged)?
    };
    let heuristic_overflow = path_probe_overflow || heuristic_rows.len() > 500;
    if heuristic_overflow {
        precision_notes.push("heuristic_work_limit".to_string());
    } else {
        for symbol in heuristic_rows.into_iter().filter(test_symbol) {
            let evidence = symbol_evidence(&symbol);
            let heuristic_evidence = TestEvidence {
                kind: "same_component_test_filename".to_string(),
                seed: None,
                component: Some(component_for(&symbol.file_path)),
            };
            match tests_by_symbol.entry(evidence.clone()) {
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().evidence.push(heuristic_evidence);
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(TestCandidate {
                        symbol: evidence,
                        classification: "heuristic".to_string(),
                        minimum_depth: None,
                        confidence: "low".to_string(),
                        evidence: vec![heuristic_evidence],
                    });
                }
            }
        }
    }
    let mut tests: Vec<TestCandidate> = tests_by_symbol.into_values().collect();
    for test in &mut tests {
        test.evidence.sort();
        test.evidence.dedup();
    }
    tests.sort_by(|a, b| {
        let rank = |value: &str| match value {
            "direct" => 0,
            "transitive" => 1,
            _ => 2,
        };
        (
            rank(&a.classification),
            a.minimum_depth.unwrap_or(u32::MAX),
            &a.symbol.file,
            a.symbol.line,
            &a.symbol.name,
        )
            .cmp(&(
                rank(&b.classification),
                b.minimum_depth.unwrap_or(u32::MAX),
                &b.symbol.file,
                b.symbol.line,
                &b.symbol.name,
            ))
    });

    let mut component_counts: BTreeMap<String, (u32, u32, u32)> = BTreeMap::new();
    for symbol in &changed_symbols {
        component_counts
            .entry(component_for(&symbol.file))
            .or_default()
            .0 += 1;
    }
    for impact in &impacts {
        component_counts
            .entry(component_for(&impact.symbol.file))
            .or_default()
            .1 += 1;
    }
    for test in &tests {
        component_counts
            .entry(component_for(&test.symbol.file))
            .or_default()
            .2 += 1;
    }
    let components = component_counts
        .into_iter()
        .map(|(component, counts)| ComponentImpact {
            component,
            changed_symbols: counts.0,
            impacted_symbols: counts.1,
            candidate_tests: counts.2,
        })
        .collect::<Vec<_>>();

    #[cfg(test)]
    run_impact_test_hook(ImpactTestStage::BeforeDataVersionRecheck);
    store
        .end_read_snapshot()
        .map_err(|_| ChangeImpactError::SnapshotChanged)?;
    let data_version_after = store
        .data_version()
        .map_err(|_| ChangeImpactError::SnapshotChanged)?;
    if data_version_before != data_version_after {
        return Err(ChangeImpactError::SnapshotChanged);
    }
    #[cfg(test)]
    run_impact_test_hook(ImpactTestStage::BeforeGitSnapshotRecheck);
    if crate::diff::current_head_oid(&repository_root)? != working.head_oid {
        return Err(ChangeImpactError::SnapshotChanged);
    }
    let rechecked =
        crate::diff::symbols_changed_in_worktree(store, &repository_root, &working.baseline_oid)?;
    if rechecked.snapshot_token != working.snapshot_token || rechecked.files != working.files {
        return Err(ChangeImpactError::SnapshotChanged);
    }

    precision_notes.sort();
    precision_notes.dedup();
    let files = working
        .files
        .iter()
        .map(|file| ChangedFile {
            path: file.path.clone(),
            status: file.status.clone(),
        })
        .collect::<Vec<_>>();
    let files_collection = Collection {
        total: working.files_total,
        returned: files.len() as u32,
        truncated: working.files_truncated,
        truncation_reason: working.files_truncated.then(|| "file_limit".to_string()),
        items: files,
    };
    let impact_collection = if graph_overflow {
        work_limited_collection(Vec::new())
    } else {
        bounded_collection(impacts, top, "top_limit")
    };
    let crossing_collection = if graph_overflow {
        work_limited_collection(Vec::new())
    } else {
        bounded_collection(crossings, 500, "crossing_limit")
    };
    let tests_collection = if heuristic_overflow {
        let mut retained = tests;
        retained.truncate(500);
        work_limited_collection(retained)
    } else {
        bounded_collection(tests, 500, "test_limit")
    };
    Ok(ChangeImpactResponse {
        schema_version: 1,
        baseline: ImpactBaseline {
            requested_ref: git_ref.to_string(),
            baseline_oid: working.baseline_oid,
            head_oid: working.head_oid,
            includes_worktree: true,
            includes_untracked: true,
        },
        scope: ImpactScope {
            repository_relative_root: scope,
        },
        changes: ImpactChanges {
            files: files_collection,
            symbols: bounded_collection(changed_symbols, 50_000, "symbol_limit"),
        },
        affected_components: bounded_collection(components, 500, "component_limit"),
        impact: impact_collection,
        api_crossings: crossing_collection,
        tests: tests_collection,
        limits: ImpactLimits {
            changed_files: crate::diff::CHANGE_FILE_LIMIT as u32,
            changed_seeds: CHANGE_SEED_LIMIT as u32,
            graph_rows: (IMPACT_WORK_LIMIT - 1) as u32,
            impact: top as u32,
            tests: 500,
            crossings: 500,
            heuristic_paths: 50_000,
            max_depth,
        },
        precision_notes,
    })
}

pub fn dependency_cycles(
    store: &Store,
    language: Option<&str>,
    min_size: u32,
) -> rusqlite::Result<DependencyCyclesResponse> {
    let cycles = store.dependency_cycles(language, min_size as usize)?;
    Ok(DependencyCyclesResponse {
        count: cycles.len() as u32,
        min_size,
        cycles,
    })
}

pub fn centrality(
    store: &Store,
    prefix: Option<&str>,
    language: Option<&str>,
    kind: Option<&str>,
    top: u32,
) -> rusqlite::Result<CentralityResponse> {
    let raw = store.centrality(prefix, language, kind, top)?;
    let results: Vec<CentralityHit> = raw
        .into_iter()
        .map(|(s, in_degree, name_collision)| CentralityHit {
            name: s.name,
            kind: s.kind,
            file: s.file_path,
            line: s.line_start,
            in_degree,
            name_collision,
            signature: s.signature,
            decorators: s.decorators,
        })
        .collect();
    Ok(CentralityResponse {
        count: results.len() as u32,
        prefix: prefix.map(String::from),
        top,
        results,
    })
}

pub fn callers(
    store: &Store,
    name: &str,
    language: Option<&str>,
    edge_kind: Option<&str>,
) -> rusqlite::Result<CallersResponse> {
    let callers: Vec<SymbolHit> = store
        .callers_of(name, language, edge_kind)?
        .into_iter()
        .map(SymbolHit::from)
        .collect();
    Ok(CallersResponse {
        target: name.to_string(),
        count: callers.len() as u32,
        name_collision: store.definition_count(name)?,
        callers,
    })
}

pub fn callees(
    store: &Store,
    name: &str,
    language: Option<&str>,
    edge_kind: Option<&str>,
) -> rusqlite::Result<CalleesResponse> {
    let matched = store
        .search_symbols(name, None, language)?
        .into_iter()
        .next();
    let edge_precision = matched.as_ref().map(|s| lang_precision(&s.file_path));
    let callees: Vec<CalleesEntry> = if let Some(ref sym) = matched {
        store
            .callees_of(sym.id, edge_kind)?
            .into_iter()
            .map(|(n, l)| CalleesEntry { name: n, line: l })
            .collect()
    } else {
        Vec::new()
    };
    Ok(CalleesResponse {
        symbol: name.to_string(),
        matched: matched.map(SymbolHit::from),
        count: callees.len() as u32,
        callees,
        edge_precision,
    })
}

pub fn explain(
    store: &Store,
    name: &str,
    language: Option<&str>,
) -> rusqlite::Result<ExplainResponse> {
    let symbols = store.search_symbols(name, None, language)?;

    let first_file = symbols.first().map(|s| s.file_path.as_str()).unwrap_or("");
    let precision = lang_precision(first_file);

    let caller_count = store.callers_of(name, language, None)?.len() as u32;
    let callee_count = symbols
        .first()
        .and_then(|s| store.callees_of(s.id, None).ok())
        .map(|v| v.len() as u32)
        .unwrap_or(0);

    let partial_count = symbols
        .iter()
        .filter(|s| {
            s.decorators
                .as_deref()
                .map(|d| d.contains(",partial,"))
                .unwrap_or(false)
        })
        .count();
    let collapse_note = if partial_count > 1 {
        Some(format!(
            "{partial_count} partial-class declarations — collapsed by default; \
             use query search --no-collapse-partials to see all"
        ))
    } else {
        None
    };

    let matched = symbols
        .iter()
        .map(|s| {
            let ext = std::path::Path::new(&s.file_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            ExplainSymbol {
                id: s.id,
                name: s.name.clone(),
                kind: s.kind.clone(),
                file: s.file_path.clone(),
                line: s.line_start,
                language: lang_from_ext(ext),
            }
        })
        .collect();

    let limitations = precision
        .limitations
        .iter()
        .map(|l| l.to_string())
        .collect();

    Ok(ExplainResponse {
        query: name.to_string(),
        matched,
        caller_count,
        callee_count,
        edge_precision: precision,
        collapse_note,
        limitations,
    })
}

pub fn impact(
    store: &Store,
    name: &str,
    max_depth: u32,
    language: Option<&str>,
) -> rusqlite::Result<ImpactResponse> {
    let depth = max_depth.clamp(1, 10);
    let impact: Vec<ImpactEntry> = store
        .impact_of(name, depth, language)?
        .into_iter()
        .map(|(s, d)| ImpactEntry {
            symbol: SymbolHit::from(s),
            depth: d,
        })
        .collect();
    Ok(ImpactResponse {
        target: name.to_string(),
        max_depth: depth,
        count: impact.len() as u32,
        name_collision: store.definition_count(name)?,
        impact,
    })
}

pub fn files(
    store: &Store,
    prefix: Option<&str>,
    language: Option<&str>,
) -> rusqlite::Result<FilesResponse> {
    // SQL LIKE pattern — match anything starting with prefix
    let pattern = prefix.map(|p| {
        if p.ends_with('%') {
            p.to_string()
        } else {
            format!("{p}%")
        }
    });
    let files = store.files_under(pattern.as_deref(), language)?;
    Ok(FilesResponse {
        prefix: prefix.map(String::from),
        count: files.len() as u32,
        files,
    })
}

/// Parse a duration like "30s" / "10m" / "2h" / "1d" into seconds.
/// Errors on missing suffix, unknown suffix, or non-numeric prefix.
pub fn parse_duration(s: &str) -> Result<u64, String> {
    if s.len() < 2 {
        return Err(format!("duration too short: {s:?}"));
    }
    let (num_part, suffix) = s.split_at(s.len() - 1);
    let n: u64 = num_part
        .parse()
        .map_err(|_| format!("duration prefix not a non-negative integer: {num_part:?}"))?;
    let multiplier: u64 = match suffix {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        other => {
            return Err(format!(
                "unknown duration suffix {other:?}; expected s/m/h/d"
            ))
        }
    };
    Ok(n * multiplier)
}

#[derive(Debug, Serialize)]
pub struct RecentChangesResponse {
    pub since: String,
    pub window_secs: u64,
    pub count: u32,
    pub files: Vec<FileEntry>,
}

/// Files re-indexed within the last `since` window (e.g. "2h"). Useful for
/// incident-response Phase 3 ("what's been touched recently?") and debugging
/// stale-index symptoms.
///
/// `indexed_at` is stored in **milliseconds** by the indexer (see `indexer.rs` —
/// `as_millis() as i64`), so the threshold is computed in ms too.
pub fn recent_changes(store: &Store, since: &str) -> Result<RecentChangesResponse, String> {
    let window_secs = parse_duration(since)?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis() as i64;
    let threshold_ms = now_ms - (window_secs as i64) * 1000;
    let files = store
        .files_indexed_since(threshold_ms)
        .map_err(|e| e.to_string())?;
    Ok(RecentChangesResponse {
        since: since.to_string(),
        window_secs,
        count: files.len() as u32,
        files,
    })
}

#[derive(Debug, Serialize)]
pub struct UnreferencedResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub count: u32,
    pub symbols: Vec<SymbolHit>,
}

/// Symbols nothing references. See `Store::unreferenced` for false-positive caveats.
pub fn unreferenced(
    store: &Store,
    kind: Option<&str>,
    language: Option<&str>,
) -> rusqlite::Result<UnreferencedResponse> {
    let syms: Vec<SymbolHit> = store
        .unreferenced(kind, language)?
        .into_iter()
        .map(SymbolHit::from)
        .collect();
    Ok(UnreferencedResponse {
        kind: kind.map(String::from),
        language: language.map(String::from),
        count: syms.len() as u32,
        symbols: syms,
    })
}

#[derive(Debug, Serialize)]
pub struct ApiSurfaceResponse {
    pub prefix: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub count: u32,
    pub symbols: Vec<SymbolHit>,
}

/// Symbols under `prefix` that are referenced from outside `prefix`.
pub fn api_surface(
    store: &Store,
    prefix: &str,
    language: Option<&str>,
) -> rusqlite::Result<ApiSurfaceResponse> {
    let syms: Vec<SymbolHit> = store
        .api_surface(prefix, language)?
        .into_iter()
        .map(SymbolHit::from)
        .collect();
    Ok(ApiSurfaceResponse {
        prefix: prefix.to_string(),
        language: language.map(String::from),
        count: syms.len() as u32,
        symbols: syms,
    })
}

pub fn status(store: &Store) -> rusqlite::Result<StatusResponse> {
    let db_path = store.db_path();
    Ok(StatusResponse {
        db_path: db_path.to_string_lossy().to_string(),
        symbol_count: store.symbol_count()?,
        file_count: store.file_count()?,
        stale_files: stale_count(db_path),
        extractor_contract_current: store.extractor_contract_current()?,
    })
}

/// Best-effort count of source files that differ from their stored index mtime
/// (capped). Returns 1 when freshness cannot be read so `status` does not claim
/// that a damaged index is current.
/// The db path may be relative, so canonicalize before climbing to project root.
fn stale_count(db_path: &std::path::Path) -> usize {
    let Ok(db_abs) = db_path.canonicalize() else {
        return 1;
    };
    let Some(root) = db_abs.parent().and_then(|d| d.parent()) else {
        return 1;
    };
    crate::workflow_status::stale_paths(root, &db_abs, 100)
        .map(|paths| paths.len())
        .unwrap_or(1)
}

#[derive(Debug, Serialize)]
pub struct ImportEntry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub line: u32,
}

#[derive(Debug, Serialize)]
pub struct ImportsResponse {
    pub file: String,
    pub count: u32,
    pub imports: Vec<ImportEntry>,
}

#[derive(Debug, Serialize)]
pub struct ImportedByResponse {
    pub name: String,
    pub count: u32,
    pub files: Vec<String>,
}

pub fn imports(store: &Store, file: &str) -> rusqlite::Result<ImportsResponse> {
    let triples = store.imports_of(file)?;
    let imports: Vec<ImportEntry> = triples
        .into_iter()
        .map(|(name, path, line)| ImportEntry { name, path, line })
        .collect();
    Ok(ImportsResponse {
        file: file.to_string(),
        count: imports.len() as u32,
        imports,
    })
}

/// `match_kind`: "name" (default) matches the leaf binding;
/// "path" matches the fully-qualified import path exactly.
#[derive(Debug, Serialize)]
pub struct SymbolsInFileResponse {
    pub file: String,
    pub count: u32,
    pub symbols: Vec<SymbolHit>,
}

pub fn symbols_in_file(store: &Store, file: &str) -> rusqlite::Result<SymbolsInFileResponse> {
    let syms: Vec<SymbolHit> = store
        .symbols_in_file(file)?
        .into_iter()
        .map(SymbolHit::from)
        .collect();
    Ok(SymbolsInFileResponse {
        file: file.to_string(),
        count: syms.len() as u32,
        symbols: syms,
    })
}

#[derive(Debug, Serialize)]
pub struct OutlineNode {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub line_start: u32,
    pub line_end: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<OutlineNode>,
}

#[derive(Debug, Serialize)]
pub struct OutlineResponse {
    pub file: String,
    pub count: u32,
    pub nodes: Vec<OutlineNode>,
}

/// Build a tree of a file's symbols via `parent_id` chains. Returns top-level
/// nodes (parent_id IS NULL); each node holds its children sorted by line.
/// Single SELECT, in-memory tree construction.
pub fn outline(store: &Store, file: &str) -> rusqlite::Result<OutlineResponse> {
    use std::collections::HashMap;
    let flat = store.symbols_in_file(file)?;
    let total = flat.len() as u32;

    // Child lists keyed by parent id (None = root).
    let mut children_of: HashMap<Option<i64>, Vec<crate::store::Symbol>> = HashMap::new();
    for sym in flat {
        children_of.entry(sym.parent_id).or_default().push(sym);
    }

    fn build(
        parent_id: Option<i64>,
        children_of: &mut HashMap<Option<i64>, Vec<crate::store::Symbol>>,
    ) -> Vec<OutlineNode> {
        let mut nodes = children_of.remove(&parent_id).unwrap_or_default();
        nodes.sort_by_key(|s| s.line_start);
        nodes
            .into_iter()
            .map(|s| OutlineNode {
                id: s.id,
                name: s.name,
                kind: s.kind,
                line_start: s.line_start,
                line_end: s.line_end,
                signature: s.signature,
                children: build(Some(s.id), children_of),
            })
            .collect()
    }

    let nodes = build(None, &mut children_of);
    Ok(OutlineResponse {
        file: file.to_string(),
        count: total,
        nodes,
    })
}

/// `match_kind`: "name" (default) matches the leaf binding;
/// "path" matches the fully-qualified import path exactly.
/// `language` scopes to a single language (defends against monorepo name collisions).
pub fn imported_by(
    store: &Store,
    query: &str,
    match_kind: &str,
    language: Option<&str>,
) -> rusqlite::Result<ImportedByResponse> {
    let files = match match_kind {
        "path" => store.imported_by_path(query, language)?,
        _ => store.imported_by_name(query, language)?,
    };
    Ok(ImportedByResponse {
        name: query.to_string(),
        count: files.len() as u32,
        files,
    })
}

/// Outcome of comparing a file's current structural shape against the
/// fingerprint stored in the index.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeClass {
    /// No stored fingerprint — never indexed (or indexed pre-0.28 before the
    /// column existed, not yet re-indexed).
    FirstSeen,
    /// Fingerprint matches — only line numbers / whitespace / comments differ.
    Cosmetic,
    /// Fingerprint differs — signatures, edges, or imports changed.
    Structural,
}

/// Structured response for `mmcg_change_class`. Both fingerprints are surfaced
/// so consumers can sanity-check against `sqlite3` or persist across sessions.
#[derive(Debug, serde::Serialize)]
pub struct ChangeClassReport {
    pub file: String,
    pub class: ChangeClass,
    pub stored_fingerprint: Option<String>,
    pub current_fingerprint: String,
}

/// Classify a file's current state against its last-indexed fingerprint.
/// `rel_path` is relative to `root`. Errors if the file can't be parsed or no
/// extractor supports the extension.
pub fn classify_change(
    store: &crate::store::Store,
    root: &std::path::Path,
    rel_path: &str,
) -> Result<ChangeClassReport, String> {
    let full_path = root.join(rel_path);
    let extractor = crate::indexer::extractor_for_path(&full_path)
        .ok_or_else(|| format!("no extractor for {rel_path}"))?;
    let pending = crate::indexer::parse_one(&full_path, root, extractor.as_ref())
        .map_err(|e| format!("parse {rel_path}: {e:?}"))?;
    let current = crate::fingerprint::compute_structural_fingerprint(&pending);
    let stored = store
        .file_fingerprint(rel_path)
        .map_err(|e| format!("read fingerprint for {rel_path}: {e}"))?;
    let class = match stored.as_deref() {
        None => ChangeClass::FirstSeen,
        Some("") => ChangeClass::FirstSeen,
        Some(s) if s == current => ChangeClass::Cosmetic,
        Some(_) => ChangeClass::Structural,
    };
    Ok(ChangeClassReport {
        file: rel_path.to_string(),
        class,
        stored_fingerprint: stored,
        current_fingerprint: current,
    })
}

const MAP_PATH_LIMIT: usize = 50_000;
const MAP_LANGUAGE_LIMIT: usize = 20;
const MAP_COMPONENT_LIMIT: usize = 20;
const MAP_BOUNDARY_LIMIT: usize = 20;
const MAP_BOUNDARY_GLOBAL_LIMIT: usize = 400;
const MAP_ENTRY_LIMIT: usize = 50;
const MAP_CYCLE_EDGE_LIMIT: usize = 50_000;
const MAP_CYCLE_LIMIT: usize = 50;
const MAP_CYCLE_MEMBERSHIP_LIMIT: usize = 500;
fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Serialize)]
pub struct MapSection<T> {
    pub total: Option<u32>,
    pub returned: u32,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation_reason: Option<&'static str>,
    pub items: Vec<T>,
}

#[derive(Debug, Serialize)]
pub struct MapCount {
    pub total: Option<u32>,
    pub returned: u32,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation_reason: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct MapScope {
    pub path: String,
    pub kind: String,
    pub depth: u8,
    pub aggregation_paths_truncated: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub production_only: bool,
}

#[derive(Debug, Serialize)]
pub struct MapLanguage {
    pub language: String,
    pub file_count: u32,
}

#[derive(Debug, Serialize)]
pub struct MapComponent {
    pub path: String,
    pub file_count: u32,
    pub languages: Vec<MapLanguage>,
    pub boundaries: MapSection<SymbolHit>,
}

#[derive(Debug, Serialize)]
pub struct MapEntryPoint {
    pub file: String,
    pub classification: &'static str,
    pub evidence: MapEvidence,
}

#[derive(Debug, Serialize)]
pub struct MapEvidence {
    pub kind: &'static str,
    pub matched: String,
}

#[derive(Debug, Serialize)]
pub struct MapNote {
    pub code: &'static str,
    pub message: &'static str,
}

#[derive(Debug, Serialize)]
pub struct MapHotspot {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub in_degree: u32,
    pub name_collision: u32,
    pub edge_precision: EdgePrecision,
}

#[derive(Debug, Serialize)]
pub struct MapLimits {
    pub paths: u32,
    pub languages: u32,
    pub components: u32,
    pub boundaries_per_component: u32,
    pub boundaries_global: u32,
    pub entry_points: u32,
    pub hotspots: u32,
    pub cycle_edges: u32,
    pub cycles: u32,
    pub cycle_memberships: u32,
}

#[derive(Debug, Serialize)]
pub struct ProjectMapResponse {
    pub schema_version: u32,
    pub scope: MapScope,
    pub files: MapCount,
    pub languages: MapSection<MapLanguage>,
    pub components: MapSection<MapComponent>,
    pub entry_points: MapSection<MapEntryPoint>,
    pub hotspots: MapSection<MapHotspot>,
    pub cycles: MapSection<Vec<String>>,
    pub limits: MapLimits,
    pub precision_notes: Vec<MapNote>,
}

pub fn normalize_map_path(input: &str) -> Result<String, String> {
    let replaced = input.replace('\\', "/");
    if replaced.starts_with('/')
        || replaced.starts_with("//")
        || replaced.as_bytes().get(1) == Some(&b':')
    {
        return Err("map path must be repository-relative".into());
    }
    let mut parts = Vec::new();
    for part in replaced.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err("map path cannot contain parent traversal".into());
        }
        if part.chars().any(|ch| ch == '\0' || ch.is_control()) {
            return Err("map path cannot contain control characters".into());
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

fn map_language(path: &str) -> String {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    lang_from_ext(ext).to_string()
}

fn component_for_file(scope: &str, kind: &str, file: &str, depth: u8) -> String {
    if kind == "file" {
        return file.to_string();
    }
    let prefix = if scope.is_empty() {
        String::new()
    } else {
        format!("{scope}/")
    };
    let relative = file.strip_prefix(&prefix).unwrap_or(file);
    let parent = std::path::Path::new(relative)
        .parent()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if parent.is_empty() {
        return ".".into();
    }
    let local = parent
        .split('/')
        .take(depth as usize)
        .collect::<Vec<_>>()
        .join("/");
    local
}

fn language_items(counts: std::collections::BTreeMap<String, u32>) -> Vec<MapLanguage> {
    let mut items = counts
        .into_iter()
        .map(|(language, file_count)| MapLanguage {
            language,
            file_count,
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| {
        b.file_count
            .cmp(&a.file_count)
            .then_with(|| a.language.cmp(&b.language))
    });
    items
}

fn boundary_scope(scope: &str, kind: &str, component: &str) -> Option<MapBoundaryScope> {
    if kind == "file" {
        None
    } else if component == "." {
        Some(MapBoundaryScope {
            label: component.to_string(),
            path: scope.to_string(),
            match_mode: MapBoundaryMatch::Direct,
        })
    } else {
        Some(MapBoundaryScope {
            label: component.to_string(),
            path: if scope.is_empty() {
                component.to_string()
            } else {
                format!("{scope}/{component}")
            },
            match_mode: MapBoundaryMatch::Recursive,
        })
    }
}

fn map_cycle_components(edges: &[(String, String)]) -> Vec<Vec<String>> {
    let mut graph: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    let mut reverse: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (from, to) in edges {
        graph.entry(from.clone()).or_default().push(to.clone());
        graph.entry(to.clone()).or_default();
        reverse.entry(to.clone()).or_default().push(from.clone());
        reverse.entry(from.clone()).or_default();
    }
    for neighbors in graph.values_mut().chain(reverse.values_mut()) {
        neighbors.sort();
        neighbors.dedup();
    }

    let mut visited = std::collections::BTreeSet::new();
    let mut order = Vec::new();
    for start in graph.keys() {
        if !visited.insert(start.clone()) {
            continue;
        }
        let mut stack = vec![(start.clone(), false)];
        while let Some((node, expanded)) = stack.pop() {
            if expanded {
                order.push(node);
                continue;
            }
            stack.push((node.clone(), true));
            if let Some(neighbors) = graph.get(&node) {
                for neighbor in neighbors.iter().rev() {
                    if visited.insert(neighbor.clone()) {
                        stack.push((neighbor.clone(), false));
                    }
                }
            }
        }
    }

    visited.clear();
    let mut components = Vec::new();
    for start in order.into_iter().rev() {
        if !visited.insert(start.clone()) {
            continue;
        }
        let mut component = Vec::new();
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            component.push(node.clone());
            if let Some(neighbors) = reverse.get(&node) {
                for neighbor in neighbors.iter().rev() {
                    if visited.insert(neighbor.clone()) {
                        stack.push(neighbor.clone());
                    }
                }
            }
        }
        if component.len() >= 2 {
            component.sort();
            components.push(component);
        }
    }
    components.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    components
}

pub fn project_map(
    store: &Store,
    path: &str,
    depth: u8,
    top: u32,
) -> Result<ProjectMapResponse, String> {
    project_map_with_options(store, path, depth, top, false)
}

pub fn project_map_with_options(
    store: &Store,
    path: &str,
    depth: u8,
    top: u32,
    production_only: bool,
) -> Result<ProjectMapResponse, String> {
    let depth = depth.clamp(1, 6);
    let top = top.clamp(1, 100);
    let normalized = normalize_map_path(path)?;
    let kind = if normalized.is_empty() {
        "root"
    } else if store
        .file_mtime(&normalized)
        .map_err(|error| error.to_string())?
        .is_some()
    {
        "file"
    } else {
        "directory"
    };
    let mut selected = store
        .map_paths_filtered(&normalized, kind, MAP_PATH_LIMIT + 1, production_only)
        .map_err(|error| error.to_string())?;
    if selected.is_empty() {
        return Err(if production_only {
            "map production scope has no indexed files"
        } else {
            "map scope has no indexed files"
        }
        .into());
    }
    let paths_truncated = selected.len() > MAP_PATH_LIMIT;
    selected.truncate(MAP_PATH_LIMIT);

    let mut language_counts = std::collections::BTreeMap::new();
    let mut component_counts: std::collections::BTreeMap<
        String,
        (u32, std::collections::BTreeMap<String, u32>),
    > = std::collections::BTreeMap::new();
    for file in &selected {
        let language = map_language(file);
        *language_counts.entry(language.clone()).or_insert(0) += 1;
        let component = component_for_file(&normalized, kind, file, depth);
        let entry = component_counts.entry(component).or_default();
        entry.0 += 1;
        *entry.1.entry(language).or_insert(0) += 1;
    }
    let language_total = language_counts.len();
    let mut languages = language_items(language_counts);
    languages.truncate(MAP_LANGUAGE_LIMIT);

    let mut components_raw = component_counts.into_iter().collect::<Vec<_>>();
    components_raw.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then_with(|| a.0.cmp(&b.0)));
    let component_total = components_raw.len();
    components_raw.truncate((top as usize).min(MAP_COMPONENT_LIMIT));
    let boundary_scopes = components_raw
        .iter()
        .filter_map(|(component, _)| boundary_scope(&normalized, kind, component))
        .collect::<Vec<_>>();
    let boundary_rows = store
        .map_boundaries_filtered(
            &boundary_scopes,
            MAP_BOUNDARY_LIMIT + 1,
            MAP_BOUNDARY_GLOBAL_LIMIT + 1,
            production_only,
        )
        .map_err(|error| error.to_string())?;
    let boundary_global_probe = (boundary_rows.len() > MAP_BOUNDARY_GLOBAL_LIMIT)
        .then(|| boundary_rows[MAP_BOUNDARY_GLOBAL_LIMIT].component.clone());
    let mut boundaries_by_component: std::collections::BTreeMap<String, Vec<SymbolHit>> =
        Default::default();
    for row in boundary_rows.into_iter().take(MAP_BOUNDARY_GLOBAL_LIMIT) {
        boundaries_by_component
            .entry(row.component)
            .or_default()
            .push(SymbolHit::from(row.symbol));
    }
    let mut components = Vec::with_capacity(components_raw.len());
    for (component_path, (file_count, counts)) in components_raw {
        let mut boundary_items = boundaries_by_component
            .remove(&component_path)
            .unwrap_or_default();
        let boundary_global_may_have_cut = boundary_global_probe
            .as_ref()
            .is_some_and(|first_uncertain| &component_path >= first_uncertain);
        let boundary_cap_exceeded = boundary_items.len() > MAP_BOUNDARY_LIMIT;
        let (boundary_truncated, boundary_reason) = if paths_truncated {
            (true, Some("path_work_limit"))
        } else if boundary_global_may_have_cut {
            (true, Some("global_probe_limit"))
        } else if boundary_cap_exceeded {
            (true, Some("top_probe"))
        } else {
            (false, None)
        };
        boundary_items.truncate(MAP_BOUNDARY_LIMIT);
        components.push(MapComponent {
            path: component_path,
            file_count,
            languages: language_items(counts),
            boundaries: MapSection {
                total: if boundary_truncated {
                    None
                } else {
                    Some(boundary_items.len() as u32)
                },
                returned: boundary_items.len() as u32,
                truncated: boundary_truncated,
                truncation_reason: boundary_reason,
                items: boundary_items,
            },
        });
    }

    const ENTRY_NAMES: &[(&str, u8)] = &[
        ("main.rs", 1),
        ("main.go", 1),
        ("main.py", 1),
        ("__main__.py", 1),
        ("Program.cs", 2),
        ("Main.java", 2),
        ("index.php", 2),
        ("app.py", 3),
        ("index.ts", 3),
        ("index.tsx", 3),
        ("index.js", 3),
        ("index.jsx", 3),
        ("lib.rs", 4),
        ("mod.rs", 4),
    ];
    let mut entry_points = Vec::new();
    for file in &selected {
        let basename = std::path::Path::new(file)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if let Some((_, priority)) = ENTRY_NAMES.iter().find(|(name, _)| *name == basename) {
            entry_points.push((
                *priority,
                MapEntryPoint {
                    file: file.clone(),
                    classification: "heuristic",
                    evidence: MapEvidence {
                        kind: "filename",
                        matched: basename.to_string(),
                    },
                },
            ));
        }
    }
    entry_points.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.file.cmp(&b.1.file)));
    let entry_total = entry_points.len();
    let entry_points = entry_points
        .into_iter()
        .take(MAP_ENTRY_LIMIT)
        .map(|(_, entry)| entry)
        .collect::<Vec<_>>();

    let hotspot_probe = top as usize + 1;
    let hotspot_rows = store
        .map_centrality_filtered(&normalized, kind, hotspot_probe, production_only)
        .map_err(|error| error.to_string())?;
    let mut hotspot_items = hotspot_rows
        .into_iter()
        .map(|row| MapHotspot {
            name: row.symbol.name,
            kind: row.symbol.kind,
            file: row.symbol.file_path.clone(),
            line: row.symbol.line_start,
            in_degree: row.in_degree,
            name_collision: row.name_collision,
            edge_precision: lang_precision(&row.symbol.file_path),
        })
        .collect::<Vec<_>>();
    let hotspots_truncated = hotspot_items.len() > top as usize;
    if hotspots_truncated {
        hotspot_items.truncate(top as usize);
    }

    let import_edges = store
        .map_import_edges_filtered(&normalized, kind, MAP_CYCLE_EDGE_LIMIT + 1, production_only)
        .map_err(|error| error.to_string())?;
    let cycle_work_truncated = import_edges.len() > MAP_CYCLE_EDGE_LIMIT;
    let (cycle_total, cycle_items) = if cycle_work_truncated {
        (None, Vec::new())
    } else {
        let all_cycles = map_cycle_components(&import_edges);
        let total = all_cycles.len();
        let mut memberships = 0usize;
        let mut retained = Vec::new();
        for cycle in all_cycles {
            if retained.len() >= MAP_CYCLE_LIMIT {
                break;
            }
            if memberships + cycle.len() <= MAP_CYCLE_MEMBERSHIP_LIMIT {
                memberships += cycle.len();
                retained.push(cycle);
            }
        }
        (Some(total as u32), retained)
    };

    let mut precision_notes = vec![
        MapNote {
            code: "syntactic_graph",
            message: "Call and import edges are syntactic and may miss dynamic dispatch or reflection.",
        },
        MapNote {
            code: "heuristic_entry_points",
            message: "Entry points are filename heuristics, not runtime reachability claims.",
        },
        MapNote {
            code: "name_resolution",
            message: "Boundary and cycle edges resolve names syntactically and may over-approximate collisions.",
        },
    ];
    if paths_truncated {
        precision_notes.push(MapNote {
            code: "path_work_limit",
            message: "Language and component counts are based on the first 50000 indexed paths in lexical order.",
        });
    }
    if hotspot_items.iter().any(|item| item.name_collision > 1) {
        precision_notes.push(MapNote {
            code: "name_collision",
            message: "One or more hotspots pool callers across same-named definitions.",
        });
    }
    if cycle_work_truncated {
        precision_notes.push(MapNote {
            code: "work_limit",
            message:
                "Cycle analysis was skipped because the scoped import graph exceeded 50000 edges.",
        });
    }

    Ok(ProjectMapResponse {
        schema_version: 1,
        scope: MapScope {
            path: if normalized.is_empty() {
                ".".into()
            } else {
                normalized
            },
            kind: kind.into(),
            depth,
            aggregation_paths_truncated: paths_truncated,
            production_only,
        },
        files: MapCount {
            total: (!paths_truncated).then_some(selected.len() as u32),
            returned: selected.len() as u32,
            truncated: paths_truncated,
            truncation_reason: paths_truncated.then_some("path_work_limit"),
        },
        languages: MapSection {
            total: (!paths_truncated).then_some(language_total as u32),
            returned: languages.len() as u32,
            truncated: paths_truncated || language_total > languages.len(),
            truncation_reason: paths_truncated.then_some("path_work_limit"),
            items: languages,
        },
        components: MapSection {
            total: (!paths_truncated).then_some(component_total as u32),
            returned: components.len() as u32,
            truncated: paths_truncated || component_total > components.len(),
            truncation_reason: paths_truncated.then_some("path_work_limit"),
            items: components,
        },
        entry_points: MapSection {
            total: (!paths_truncated).then_some(entry_total as u32),
            returned: entry_points.len() as u32,
            truncated: paths_truncated || entry_total > entry_points.len(),
            truncation_reason: paths_truncated.then_some("path_work_limit"),
            items: entry_points,
        },
        hotspots: MapSection {
            total: if hotspots_truncated {
                None
            } else {
                Some(hotspot_items.len() as u32)
            },
            returned: hotspot_items.len() as u32,
            truncated: hotspots_truncated,
            truncation_reason: hotspots_truncated.then_some("top_probe"),
            items: hotspot_items,
        },
        cycles: MapSection {
            total: cycle_total,
            returned: cycle_items.len() as u32,
            truncated: cycle_work_truncated
                || cycle_total.is_some_and(|total| total > cycle_items.len() as u32),
            truncation_reason: cycle_work_truncated.then_some("work_limit"),
            items: cycle_items,
        },
        limits: MapLimits {
            paths: MAP_PATH_LIMIT as u32,
            languages: MAP_LANGUAGE_LIMIT as u32,
            components: MAP_COMPONENT_LIMIT as u32,
            boundaries_per_component: MAP_BOUNDARY_LIMIT as u32,
            boundaries_global: MAP_BOUNDARY_GLOBAL_LIMIT as u32,
            entry_points: MAP_ENTRY_LIMIT as u32,
            hotspots: 100,
            cycle_edges: MAP_CYCLE_EDGE_LIMIT as u32,
            cycles: MAP_CYCLE_LIMIT as u32,
            cycle_memberships: MAP_CYCLE_MEMBERSHIP_LIMIT as u32,
        },
        precision_notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::path::PathBuf;

    fn tmp_db(name: &str) -> PathBuf {
        let mut p = env::temp_dir();
        p.push(format!("mmcg-queries-{}-{}.db", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn project_map_scope_is_lexical_and_exact() {
        assert_eq!(normalize_map_path("./src//app").unwrap(), "src/app");
        assert_eq!(normalize_map_path("src\\app").unwrap(), "src/app");
        assert!(normalize_map_path("../src").is_err());
        assert!(normalize_map_path("/src").is_err());
        assert!(normalize_map_path("C:\\src").is_err());
        assert!(normalize_map_path("src/\u{1b}bad").is_err());

        let path = tmp_db("project_map_literal_scope");
        let store = Store::open(&path).unwrap();
        for file in [
            "src/%dir/a.rs",
            "src/%directory/b.rs",
            "src/_dir/c.rs",
            "src/xdir/d.rs",
            "outside.rs",
        ] {
            store.upsert_file(file, 1, 1).unwrap();
        }
        let wanted = store
            .insert_symbol(
                "wanted",
                "function",
                "src/%dir/a.rs",
                1,
                3,
                Some("fn wanted()"),
                None,
            )
            .unwrap();
        let wrong = store
            .insert_symbol("wrong", "function", "src/%directory/b.rs", 1, 3, None, None)
            .unwrap();
        let caller_a = store
            .insert_symbol("caller_a", "function", "outside.rs", 1, 3, None, None)
            .unwrap();
        let caller_b = store
            .insert_symbol("caller_b", "function", "outside.rs", 5, 7, None, None)
            .unwrap();
        store
            .insert_edge(caller_a, Some(wanted), "wanted", "calls", 2)
            .unwrap();
        store
            .insert_edge(caller_a, Some(wrong), "wrong", "calls", 3)
            .unwrap();
        store
            .insert_edge(caller_b, Some(wrong), "wrong", "calls", 6)
            .unwrap();

        let percent = serde_json::to_value(project_map(&store, "src/%dir", 2, 1).unwrap()).unwrap();
        assert_eq!(percent["files"]["total"], 1);
        assert_eq!(percent["hotspots"]["items"][0]["name"], "wanted");
        assert_eq!(
            percent["components"]["items"][0]["boundaries"]["items"][0]["name"],
            "wanted"
        );
        let underscore =
            serde_json::to_value(project_map(&store, "src/_dir", 2, 1).unwrap()).unwrap();
        assert_eq!(underscore["files"]["total"], 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn project_map_is_deterministic_and_labels_entry_points() {
        let path = tmp_db("project_map");
        let store = Store::open(&path).unwrap();
        store.upsert_file("src/main.rs", 1, 1).unwrap();
        store.upsert_file("src/lib.rs", 1, 1).unwrap();
        store.upsert_file("src/app/mod.rs", 1, 1).unwrap();
        store.upsert_file("src/application/main.rs", 1, 1).unwrap();

        let first = serde_json::to_value(project_map(&store, "src/app", 2, 20).unwrap()).unwrap();
        let second = serde_json::to_value(project_map(&store, "src/app", 2, 20).unwrap()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first["scope"]["kind"], "directory");
        assert_eq!(first["files"]["total"], 1);
        assert_eq!(
            first["entry_points"]["items"][0]["classification"],
            "heuristic"
        );
        assert_eq!(first["entry_points"]["items"][0]["file"], "src/app/mod.rs");

        let file =
            serde_json::to_value(project_map(&store, "src/main.rs", 2, 20).unwrap()).unwrap();
        assert_eq!(file["scope"]["kind"], "file");
        assert_eq!(file["components"]["items"][0]["path"], "src/main.rs");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn project_map_production_only_excludes_non_production_paths() {
        let path = tmp_db("project_map_production_only");
        let store = Store::open(&path).unwrap();
        for file in [
            "src/main.rs",
            "src/core/service.rs",
            "tests/fixture/main.rs",
            "examples/demo.rs",
            "evals/runner.py",
        ] {
            store.upsert_file(file, 1, 1).unwrap();
        }
        let production = store
            .insert_symbol(
                "production_target",
                "function",
                "src/core/service.rs",
                1,
                3,
                None,
                None,
            )
            .unwrap();
        let fixture = store
            .insert_symbol(
                "fixture_target",
                "function",
                "tests/fixture/main.rs",
                1,
                3,
                None,
                None,
            )
            .unwrap();
        let caller = store
            .insert_symbol("caller", "function", "src/main.rs", 1, 3, None, None)
            .unwrap();
        store
            .insert_edge(caller, Some(production), "production_target", "calls", 2)
            .unwrap();
        store
            .insert_edge(caller, Some(fixture), "fixture_target", "calls", 3)
            .unwrap();

        let value =
            serde_json::to_value(project_map_with_options(&store, ".", 2, 20, true).unwrap())
                .unwrap();
        assert_eq!(value["scope"]["production_only"], true);
        assert_eq!(value["files"]["total"], 2);
        let rendered = serde_json::to_string(&value).unwrap();
        assert!(!rendered.contains("tests/fixture"));
        assert!(!rendered.contains("examples/demo"));
        assert!(!rendered.contains("evals/runner"));
        assert!(rendered.contains("production_target"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn project_map_schema_v1_is_exact_and_components_are_relative() {
        let path = tmp_db("project_map_schema");
        let store = Store::open(&path).unwrap();
        for file in ["src/app/main.rs", "src/app/nested/lib.rs", "outside.rs"] {
            store.upsert_file(file, 1, 1).unwrap();
        }
        let target = store
            .insert_symbol(
                "target",
                "function",
                "src/app/main.rs",
                10,
                12,
                Some("fn target()"),
                None,
            )
            .unwrap();
        let caller = store
            .insert_symbol("caller", "function", "outside.rs", 1, 3, None, None)
            .unwrap();
        store
            .insert_edge(caller, Some(target), "target", "calls", 2)
            .unwrap();

        let value = serde_json::to_value(project_map(&store, "src", 1, 20).unwrap()).unwrap();
        let top_keys = value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            top_keys,
            [
                "components",
                "cycles",
                "entry_points",
                "files",
                "hotspots",
                "languages",
                "limits",
                "precision_notes",
                "schema_version",
                "scope",
            ]
            .into_iter()
            .map(String::from)
            .collect()
        );
        let limits = value["limits"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            limits,
            [
                "boundaries_global",
                "boundaries_per_component",
                "components",
                "cycle_edges",
                "cycle_memberships",
                "cycles",
                "entry_points",
                "hotspots",
                "languages",
                "paths",
            ]
            .into_iter()
            .map(String::from)
            .collect()
        );
        assert_eq!(value["components"]["items"][0]["path"], "app");
        let hotspot_keys = value["hotspots"]["items"][0]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            hotspot_keys,
            [
                "edge_precision",
                "file",
                "in_degree",
                "kind",
                "line",
                "name",
                "name_collision",
            ]
            .into_iter()
            .map(String::from)
            .collect()
        );
        let file =
            serde_json::to_value(project_map(&store, "src/app/main.rs", 2, 20).unwrap()).unwrap();
        assert_eq!(file["components"]["items"][0]["path"], "src/app/main.rs");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn project_map_reports_exact_top_and_top_probe_truthfully() {
        let path = tmp_db("project_map_top_probe");
        let store = Store::open(&path).unwrap();
        store.upsert_file("src/lib.rs", 1, 1).unwrap();
        store.upsert_file("outside.rs", 1, 1).unwrap();
        let caller = store
            .insert_symbol("caller", "function", "outside.rs", 1, 3, None, None)
            .unwrap();
        for (index, name) in ["alpha", "beta", "gamma"].into_iter().enumerate() {
            let target = store
                .insert_symbol(
                    name,
                    "function",
                    "src/lib.rs",
                    index as u32 + 1,
                    index as u32 + 1,
                    None,
                    None,
                )
                .unwrap();
            store
                .insert_edge(caller, Some(target), name, "calls", index as u32 + 1)
                .unwrap();
        }

        let exact = serde_json::to_value(project_map(&store, "src", 2, 3).unwrap()).unwrap();
        assert_eq!(exact["hotspots"]["total"], 3);
        assert_eq!(exact["hotspots"]["truncated"], false);
        assert!(exact["hotspots"].get("truncation_reason").is_none());

        let probed = serde_json::to_value(project_map(&store, "src", 2, 2).unwrap()).unwrap();
        assert!(probed["hotspots"]["total"].is_null());
        assert_eq!(probed["hotspots"]["returned"], 2);
        assert_eq!(probed["hotspots"]["truncated"], true);
        assert_eq!(probed["hotspots"]["truncation_reason"], "top_probe");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn project_map_propagates_path_aggregation_truncation() {
        let path = tmp_db("project_map_path_limit");
        let store = Store::open(&path).unwrap();
        for index in 0..=MAP_PATH_LIMIT {
            store
                .upsert_file(&format!("src/f{index:05}.rs"), 1, 1)
                .unwrap();
        }

        let value = serde_json::to_value(project_map(&store, "src", 2, 20).unwrap()).unwrap();
        assert_eq!(value["scope"]["aggregation_paths_truncated"], true);
        for section in ["files", "languages", "components", "entry_points"] {
            assert!(value[section]["total"].is_null());
            assert_eq!(value[section]["truncated"], true);
            assert_eq!(value[section]["truncation_reason"], "path_work_limit");
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn project_map_skips_cycles_when_scoped_edge_probe_overflows() {
        let path = tmp_db("project_map_cycle_limit");
        let store = Store::open(&path).unwrap();
        let mut sources = Vec::new();
        for index in 0..225 {
            let file = format!("src/source{index:03}.rs");
            store.upsert_file(&file, 1, 1).unwrap();
            sources.push(
                store
                    .insert_symbol("<module>", "module", &file, 1, 2, None, None)
                    .unwrap(),
            );
        }
        for index in 0..225 {
            let file = format!("src/target{index:03}.rs");
            store.upsert_file(&file, 1, 1).unwrap();
            store
                .insert_symbol("target", "function", &file, 1, 2, None, None)
                .unwrap();
        }
        for source in sources {
            store
                .insert_edge(source, None, "target", "imports", 1)
                .unwrap();
        }

        let value = serde_json::to_value(project_map(&store, "src", 2, 20).unwrap()).unwrap();
        assert!(value["cycles"]["total"].is_null());
        assert_eq!(value["cycles"]["returned"], 0);
        assert_eq!(value["cycles"]["truncated"], true);
        assert_eq!(value["cycles"]["truncation_reason"], "work_limit");
        assert!(value["precision_notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|note| note["code"] == "work_limit"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn project_map_is_stable_across_reverse_insert_and_vacuum() {
        fn build(path: &std::path::Path, reverse: bool) -> serde_json::Value {
            let store = Store::open(path).unwrap();
            let mut files = vec!["src/a.rs", "src/b.rs"];
            if reverse {
                files.reverse();
            }
            store.upsert_file("outside.rs", 1, 1).unwrap();
            let caller = store
                .insert_symbol("caller", "function", "outside.rs", 1, 2, None, None)
                .unwrap();
            for file in files {
                store.upsert_file(file, 1, 1).unwrap();
                let name = if file.ends_with("a.rs") {
                    "alpha"
                } else {
                    "beta"
                };
                let target = store
                    .insert_symbol(name, "function", file, 10, 12, None, None)
                    .unwrap();
                store
                    .insert_edge(caller, Some(target), name, "calls", 1)
                    .unwrap();
            }
            serde_json::to_value(project_map(&store, "src", 2, 20).unwrap()).unwrap()
        }

        let first_path = tmp_db("project_map_stable_first");
        let second_path = tmp_db("project_map_stable_second");
        let first = build(&first_path, false);
        let second_before = build(&second_path, true);
        assert_eq!(first, second_before);
        rusqlite::Connection::open(&second_path)
            .unwrap()
            .execute_batch("VACUUM")
            .unwrap();
        let store = Store::open(&second_path).unwrap();
        let second_after =
            serde_json::to_value(project_map(&store, "src", 2, 20).unwrap()).unwrap();
        assert_eq!(first, second_after);
        std::fs::remove_file(&first_path).ok();
        std::fs::remove_file(&second_path).ok();
    }

    #[test]
    fn project_map_exact_semantic_ties_ignore_insertion_ids_and_vacuum() {
        fn build(path: &std::path::Path, reverse: bool) -> Vec<u8> {
            let store = Store::open(path).unwrap();
            store.upsert_file("src/lib.rs", 1, 1).unwrap();
            store.upsert_file("outside.rs", 1, 1).unwrap();
            let mut variants = vec![
                ("function", Some("fn tied()")),
                ("method", Some("fn tied(&self)")),
            ];
            if reverse {
                variants.reverse();
            }
            let mut target = None;
            for (kind, signature) in variants {
                let id = store
                    .insert_symbol("tied", kind, "src/lib.rs", 10, 12, signature, None)
                    .unwrap();
                target.get_or_insert(id);
            }
            let caller = store
                .insert_symbol("caller", "function", "outside.rs", 1, 2, None, None)
                .unwrap();
            store
                .insert_edge(caller, target, "tied", "calls", 1)
                .unwrap();
            serde_json::to_vec(&project_map(&store, "src", 2, 20).unwrap()).unwrap()
        }

        let first_path = tmp_db("project_map_semantic_ties_first");
        let second_path = tmp_db("project_map_semantic_ties_second");
        let first = build(&first_path, false);
        let second_before = build(&second_path, true);
        assert_eq!(first, second_before);
        rusqlite::Connection::open(&second_path)
            .unwrap()
            .execute_batch("VACUUM")
            .unwrap();
        let second_after = {
            let store = Store::open(&second_path).unwrap();
            serde_json::to_vec(&project_map(&store, "src", 2, 20).unwrap()).unwrap()
        };
        assert_eq!(first, second_after);
        std::fs::remove_file(&first_path).ok();
        std::fs::remove_file(&second_path).ok();
    }

    #[test]
    fn project_map_large_out_of_scope_fixture_stays_within_fetch_caps() {
        let path = tmp_db("project_map_out_of_scope");
        let store = Store::open(&path).unwrap();
        for index in 0..100_000 {
            store
                .upsert_file(&format!("vendor/f{index:06}.rs"), 1, 1)
                .unwrap();
        }
        store.upsert_file("src/a.rs", 1, 1).unwrap();
        store.upsert_file("src/b.rs", 1, 1).unwrap();

        let value = serde_json::to_value(project_map(&store, "src", 2, 20).unwrap()).unwrap();
        assert_eq!(value["files"]["total"], 2);
        assert_eq!(value["files"]["returned"], 2);
        assert_eq!(value["files"]["truncated"], false);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn project_map_dot_component_boundaries_exclude_nested_and_work_at_root() {
        let directory_path = tmp_db("project_map_dot_directory");
        let directory_store = Store::open(&directory_path).unwrap();
        for file in ["src/app/lib.rs", "src/app/nested/lib.rs", "outside.rs"] {
            directory_store.upsert_file(file, 1, 1).unwrap();
        }
        let direct_target = directory_store
            .insert_symbol(
                "direct_target",
                "function",
                "src/app/lib.rs",
                1,
                2,
                None,
                None,
            )
            .unwrap();
        let nested_target = directory_store
            .insert_symbol(
                "nested_target",
                "function",
                "src/app/nested/lib.rs",
                1,
                2,
                None,
                None,
            )
            .unwrap();
        let outside = directory_store
            .insert_symbol("outside", "function", "outside.rs", 1, 2, None, None)
            .unwrap();
        directory_store
            .insert_edge(outside, Some(direct_target), "direct_target", "calls", 1)
            .unwrap();
        directory_store
            .insert_edge(outside, Some(nested_target), "nested_target", "calls", 2)
            .unwrap();

        let directory =
            serde_json::to_value(project_map(&directory_store, "src/app", 1, 20).unwrap()).unwrap();
        let directory_components = directory["components"]["items"].as_array().unwrap();
        let dot = directory_components
            .iter()
            .find(|component| component["path"] == ".")
            .unwrap();
        let nested = directory_components
            .iter()
            .find(|component| component["path"] == "nested")
            .unwrap();
        assert_eq!(dot["boundaries"]["returned"], 1);
        assert_eq!(dot["boundaries"]["items"][0]["name"], "direct_target");
        assert_eq!(nested["boundaries"]["returned"], 1);
        assert_eq!(nested["boundaries"]["items"][0]["name"], "nested_target");

        let root_path = tmp_db("project_map_dot_root");
        let root_store = Store::open(&root_path).unwrap();
        for file in ["root.rs", "root_caller.rs", "pkg/lib.rs", "pkg/caller.rs"] {
            root_store.upsert_file(file, 1, 1).unwrap();
        }
        let root_target = root_store
            .insert_symbol("root_target", "function", "root.rs", 1, 2, None, None)
            .unwrap();
        let nested_target = root_store
            .insert_symbol("nested_target", "function", "pkg/lib.rs", 1, 2, None, None)
            .unwrap();
        let nested_caller = root_store
            .insert_symbol(
                "nested_caller",
                "function",
                "pkg/caller.rs",
                1,
                2,
                None,
                None,
            )
            .unwrap();
        let root_caller = root_store
            .insert_symbol(
                "root_caller",
                "function",
                "root_caller.rs",
                1,
                2,
                None,
                None,
            )
            .unwrap();
        root_store
            .insert_edge(nested_caller, Some(root_target), "root_target", "calls", 1)
            .unwrap();
        root_store
            .insert_edge(
                root_caller,
                Some(nested_target),
                "nested_target",
                "calls",
                1,
            )
            .unwrap();

        let root = serde_json::to_value(project_map(&root_store, ".", 1, 20).unwrap()).unwrap();
        let root_components = root["components"]["items"].as_array().unwrap();
        let dot = root_components
            .iter()
            .find(|component| component["path"] == ".")
            .unwrap();
        let package = root_components
            .iter()
            .find(|component| component["path"] == "pkg")
            .unwrap();
        assert_eq!(dot["boundaries"]["returned"], 1);
        assert_eq!(dot["boundaries"]["items"][0]["name"], "root_target");
        assert_eq!(package["boundaries"]["returned"], 1);
        assert_eq!(package["boundaries"]["items"][0]["name"], "nested_target");
        std::fs::remove_file(&directory_path).ok();
        std::fs::remove_file(&root_path).ok();
    }

    #[test]
    fn project_map_boundary_exact_cap_is_not_truncated() {
        let path = tmp_db("project_map_boundary_exact_cap");
        let store = Store::open(&path).unwrap();
        store.upsert_file("src/lib.rs", 1, 1).unwrap();
        store.upsert_file("outside.rs", 1, 1).unwrap();
        let caller = store
            .insert_symbol("caller", "function", "outside.rs", 1, 2, None, None)
            .unwrap();
        for index in 0..MAP_BOUNDARY_LIMIT {
            let name = format!("target_{index:02}");
            let target = store
                .insert_symbol(
                    &name,
                    "function",
                    "src/lib.rs",
                    index as u32 + 1,
                    index as u32 + 1,
                    None,
                    None,
                )
                .unwrap();
            store
                .insert_edge(caller, Some(target), &name, "calls", index as u32 + 1)
                .unwrap();
        }

        let value = serde_json::to_value(project_map(&store, "src", 2, 20).unwrap()).unwrap();
        let boundaries = &value["components"]["items"][0]["boundaries"];
        assert_eq!(boundaries["total"], MAP_BOUNDARY_LIMIT as u32);
        assert_eq!(boundaries["returned"], MAP_BOUNDARY_LIMIT as u32);
        assert_eq!(boundaries["truncated"], false);
        assert!(boundaries.get("truncation_reason").is_none());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn project_map_boundary_cap_plus_one_and_global_probe_are_truthful() {
        let cap_path = tmp_db("project_map_boundary_cap_plus_one");
        let cap_store = Store::open(&cap_path).unwrap();
        cap_store.upsert_file("src/lib.rs", 1, 1).unwrap();
        cap_store.upsert_file("outside.rs", 1, 1).unwrap();
        let cap_caller = cap_store
            .insert_symbol("caller", "function", "outside.rs", 1, 2, None, None)
            .unwrap();
        for index in 0..=MAP_BOUNDARY_LIMIT {
            let name = format!("target_{index:02}");
            let target = cap_store
                .insert_symbol(
                    &name,
                    "function",
                    "src/lib.rs",
                    index as u32 + 1,
                    index as u32 + 1,
                    None,
                    None,
                )
                .unwrap();
            cap_store
                .insert_edge(cap_caller, Some(target), &name, "calls", index as u32 + 1)
                .unwrap();
        }
        let cap = serde_json::to_value(project_map(&cap_store, "src", 2, 20).unwrap()).unwrap();
        let cap_boundaries = &cap["components"]["items"][0]["boundaries"];
        assert!(cap_boundaries["total"].is_null());
        assert_eq!(cap_boundaries["returned"], MAP_BOUNDARY_LIMIT as u32);
        assert_eq!(cap_boundaries["truncated"], true);
        assert_eq!(cap_boundaries["truncation_reason"], "top_probe");

        let global_path = tmp_db("project_map_boundary_global_probe");
        let global_store = Store::open(&global_path).unwrap();
        global_store.upsert_file("outside.rs", 1, 1).unwrap();
        let global_caller = global_store
            .insert_symbol("caller", "function", "outside.rs", 1, 2, None, None)
            .unwrap();
        for component in 0..MAP_COMPONENT_LIMIT {
            let file = format!("src/c{component:02}/lib.rs");
            global_store.upsert_file(&file, 1, 1).unwrap();
            for index in 0..=MAP_BOUNDARY_LIMIT {
                let name = format!("target_{component:02}_{index:02}");
                let target = global_store
                    .insert_symbol(
                        &name,
                        "function",
                        &file,
                        index as u32 + 1,
                        index as u32 + 1,
                        None,
                        None,
                    )
                    .unwrap();
                global_store
                    .insert_edge(
                        global_caller,
                        Some(target),
                        &name,
                        "calls",
                        index as u32 + 1,
                    )
                    .unwrap();
            }
        }
        let global =
            serde_json::to_value(project_map(&global_store, "src", 1, 20).unwrap()).unwrap();
        let components = global["components"]["items"].as_array().unwrap();
        let known_probe = components
            .iter()
            .find(|component| component["path"] == "c18")
            .unwrap();
        let global_probe = components
            .iter()
            .find(|component| component["path"] == "c19")
            .unwrap();
        assert_eq!(known_probe["boundaries"]["truncation_reason"], "top_probe");
        assert!(global_probe["boundaries"]["total"].is_null());
        assert_eq!(global_probe["boundaries"]["truncated"], true);
        assert_eq!(
            global_probe["boundaries"]["truncation_reason"],
            "global_probe_limit"
        );
        std::fs::remove_file(&cap_path).ok();
        std::fs::remove_file(&global_path).ok();
    }

    #[test]
    fn parse_duration_units() {
        assert_eq!(parse_duration("30s").unwrap(), 30);
        assert_eq!(parse_duration("10m").unwrap(), 600);
        assert_eq!(parse_duration("2h").unwrap(), 7200);
        assert_eq!(parse_duration("1d").unwrap(), 86400);
        assert_eq!(parse_duration("0s").unwrap(), 0);
        assert!(parse_duration("").is_err());
        assert!(parse_duration("h").is_err()); // too short to have a number
        assert!(parse_duration("5y").is_err()); // unknown suffix
        assert!(parse_duration("abc").is_err());
    }

    #[test]
    fn recent_changes_filters() {
        let path = tmp_db("recent_changes_filters");
        let store = Store::open(&path).unwrap();
        // indexer stores indexed_at in ms — match that convention
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        // file_a touched 30s ago, file_b 2h ago (ms)
        store.upsert_file("file_a.py", now_ms - 30_000, 5).unwrap();
        store
            .upsert_file("file_b.py", now_ms - 7_200_000, 3)
            .unwrap();

        // "1h" window catches only file_a
        let recent = recent_changes(&store, "1h").unwrap();
        assert_eq!(recent.count, 1);
        assert_eq!(recent.files[0].path, "file_a.py");

        // "3h" catches both
        let wider = recent_changes(&store, "3h").unwrap();
        assert_eq!(wider.count, 2);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn outline_tree() {
        let path = tmp_db("outline_tree");
        let store = Store::open(&path).unwrap();
        // Class Foo (line 1) with methods bar (5) and baz (10).
        let foo = store
            .insert_symbol("Foo", "class", "x.py", 1, 15, None, None)
            .unwrap();
        let _bar = store
            .insert_symbol(
                "bar",
                "method",
                "x.py",
                5,
                7,
                Some("def bar(self)"),
                Some(foo),
            )
            .unwrap();
        let _baz = store
            .insert_symbol(
                "baz",
                "method",
                "x.py",
                10,
                12,
                Some("def baz(self)"),
                Some(foo),
            )
            .unwrap();
        // Sibling top-level function (line 20).
        let _helper = store
            .insert_symbol("helper", "function", "x.py", 20, 22, None, None)
            .unwrap();

        let out = outline(&store, "x.py").unwrap();
        assert_eq!(out.count, 4);
        assert_eq!(out.nodes.len(), 2); // Foo + helper

        // Nodes ordered by line_start
        assert_eq!(out.nodes[0].name, "Foo");
        assert_eq!(out.nodes[0].children.len(), 2);
        assert_eq!(out.nodes[0].children[0].name, "bar");
        assert_eq!(out.nodes[0].children[1].name, "baz");

        assert_eq!(out.nodes[1].name, "helper");
        assert!(out.nodes[1].children.is_empty());

        std::fs::remove_file(&path).ok();
    }

    fn mk_sym(name: &str, kind: &str, file: &str, line: u32, decorators: Option<&str>) -> Symbol {
        Symbol {
            id: 0,
            name: name.to_string(),
            kind: kind.to_string(),
            file_path: file.to_string(),
            line_start: line,
            line_end: line,
            signature: None,
            parent_id: None,
            decorators: decorators.map(String::from),
        }
    }

    #[test]
    fn collapse_partials_groups_only_partial_rows() {
        let symbols = vec![
            mk_sym("User", "class", "User.B.cs", 3, Some(",partial,")),
            mk_sym("User", "class", "User.A.cs", 3, Some(",partial,")),
            mk_sym("User", "class", "User.C.cs", 3, Some(",partial,")),
            mk_sym("Service", "class", "Service.cs", 2, None),
        ];
        let hits = collapse_partial_hits(symbols);
        // 1 partial group (User) + 1 passthrough (Service) = 2
        assert_eq!(hits.len(), 2);

        let user = hits.iter().find(|h| h.name == "User").unwrap();
        // Canonical = lex-first file
        assert_eq!(user.file, "User.A.cs");
        let locs = user.locations.as_ref().expect("partial has locations");
        assert_eq!(locs.len(), 3);
        assert_eq!(locs[0].file, "User.A.cs");
        assert_eq!(locs[1].file, "User.B.cs");
        assert_eq!(locs[2].file, "User.C.cs");

        let service = hits.iter().find(|h| h.name == "Service").unwrap();
        assert!(service.locations.is_none());
    }

    #[test]
    fn collapse_partials_passes_non_partial_duplicates_unchanged() {
        // Two distinct non-partial `Foo` classes in different namespaces — NOT a
        // partial collapse target, must remain separate hits.
        let symbols = vec![
            mk_sym("Foo", "class", "A/Foo.cs", 1, None),
            mk_sym("Foo", "class", "B/Foo.cs", 1, None),
        ];
        let hits = collapse_partial_hits(symbols);
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| h.locations.is_none()));
    }

    fn impact_repo(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = env::temp_dir().join(format!("mmcg-impact-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
                .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init", "-q", "--initial-branch=main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        run(&["config", "commit.gpgsign", "false"]);
        for (path, content) in files {
            let target = dir.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(target, content).unwrap();
        }
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "baseline"]);
        dir
    }

    fn write_impact_file(root: &Path, path: &str, content: &str) {
        let target = root.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(target, content).unwrap();
    }

    fn index_impact(root: &Path, name: &str) -> Store {
        let db = env::temp_dir().join(format!("mmcg-impact-db-{}-{name}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        let mut store = Store::open(db).unwrap();
        crate::indexer::Indexer::new(root)
            .index_all(&mut store, true)
            .unwrap();
        store
    }

    #[test]
    fn change_impact_body_only_edit_seeds_callers() {
        let root = impact_repo(
            "body_edit",
            &[(
                "src/app.py",
                "def target():\n    return 1\n\ndef caller():\n    return target()\n",
            )],
        );
        write_impact_file(
            &root,
            "src/app.py",
            "def target():\n    return 2\n\ndef caller():\n    return target()\n",
        );
        let store = index_impact(&root, "body_edit");
        let response = change_impact(&store, &root, "HEAD", 3, 100).unwrap();
        assert!(response
            .changes
            .symbols
            .items
            .iter()
            .any(|symbol| symbol.name == "target" && symbol.change == "body_changed"));
        assert!(response
            .impact
            .items
            .iter()
            .any(|impact| impact.symbol.name == "caller"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn change_impact_returns_changed_test_at_depth_zero() {
        let root = impact_repo(
            "changed_test",
            &[(
                "tests/test_app.py",
                "def test_value():\n    assert 1 == 1\n",
            )],
        );
        write_impact_file(
            &root,
            "tests/test_app.py",
            "def test_value():\n    assert 2 == 2\n",
        );
        let store = index_impact(&root, "changed_test");
        let response = change_impact(&store, &root, "HEAD", 3, 100).unwrap();
        let candidate = response
            .tests
            .items
            .iter()
            .find(|test| test.symbol.name == "test_value")
            .unwrap();
        assert_eq!(candidate.classification, "direct");
        assert_eq!(candidate.minimum_depth, Some(0));
        assert!(candidate
            .evidence
            .iter()
            .any(|evidence| evidence.kind == "changed_test_symbol"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn change_impact_rejects_stale_or_wrong_root_index() {
        let root = impact_repo(
            "root_stale",
            &[("src/app.py", "def value():\n    return 1\n")],
        );
        let store = index_impact(&root, "root_stale");
        store.set_meta("index_root", "/definitely/wrong").unwrap();
        assert_eq!(
            change_impact(&store, &root, "HEAD", 3, 100).unwrap_err(),
            ChangeImpactError::RootMismatch
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn change_impact_rejects_same_mtime_changed_content_as_stale() {
        let root = impact_repo(
            "same_mtime",
            &[("src/app.py", "def value():\n    return 1\n")],
        );
        let store = index_impact(&root, "same_mtime");
        write_impact_file(&root, "src/app.py", "def value():\n    return 2\n");
        assert_eq!(
            change_impact(&store, &root, "HEAD", 3, 100).unwrap_err(),
            ChangeImpactError::IndexStale
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn change_impact_normalizes_subdirectory_root() {
        let root = impact_repo("subdir", &[("src/app.py", "def value():\n    return 1\n")]);
        write_impact_file(&root, "src/app.py", "def value():\n    return 2\n");
        let store = index_impact(&root, "subdir");
        let response = change_impact(&store, &root.join("src"), "HEAD", 3, 100).unwrap();
        assert_eq!(response.scope.repository_relative_root, "src");
        assert!(response
            .changes
            .files
            .items
            .iter()
            .any(|file| file.path == "src/app.py"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn change_impact_skips_graph_when_seed_limit_overflows() {
        let mut baseline = String::new();
        let mut current = String::new();
        for index in 0..=CHANGE_SEED_LIMIT {
            baseline.push_str(&format!("def seed_{index}():\n    return 1\n\n"));
            current.push_str(&format!("def seed_{index}():\n    return 2\n\n"));
        }
        let root = impact_repo("seed_overflow", &[("src/many.py", &baseline)]);
        write_impact_file(&root, "src/many.py", &current);
        let store = index_impact(&root, "seed_overflow");
        let response = change_impact(&store, &root, "HEAD", 3, 100).unwrap();
        assert!(response.impact.truncated);
        assert_eq!(
            response.impact.truncation_reason.as_deref(),
            Some("work_limit")
        );
        assert!(response.impact.items.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn change_impact_never_claims_focused_tests_replace_full_gate() {
        let root = impact_repo("caveat", &[("src/app.py", "def value():\n    return 1\n")]);
        write_impact_file(&root, "src/app.py", "def value():\n    return 2\n");
        let store = index_impact(&root, "caveat");
        let response = change_impact(&store, &root, "HEAD", 3, 100).unwrap();
        assert!(response
            .precision_notes
            .contains(&"focused_tests_do_not_replace_full_gate".to_string()));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn change_impact_schema_v1_matches_committed_non_empty_golden() {
        let root = impact_repo(
            "non_empty_golden",
            &[
                (
                    "core/app.py",
                    "def target():\n    return 1\n\ndef helper():\n    return target()\n",
                ),
                (
                    "core/test_core.py",
                    "from core.app import target, helper\n\ndef test_direct():\n    return target()\n\ndef test_transitive():\n    return helper()\n\ndef test_heuristic():\n    return True\n",
                ),
                (
                    "api/entry.py",
                    "from core.app import target\n\ndef external_caller():\n    return target()\n",
                ),
            ],
        );
        write_impact_file(
            &root,
            "core/app.py",
            "def target():\n    return 2\n\ndef helper():\n    return target()\n",
        );
        write_impact_file(
            &root,
            "core/test_core.py",
            "from core.app import target, helper\n\ndef test_direct():\n    assert target() == 2\n    return target()\n\ndef test_transitive():\n    return helper()\n\ndef test_heuristic():\n    return True\n",
        );
        let store = index_impact(&root, "non_empty_golden");
        let response = change_impact(&store, &root, "HEAD", 3, 100).unwrap();
        let mut full = serde_json::to_string_pretty(&response).unwrap();
        full.push('\n');
        assert_eq!(
            full,
            include_str!("../tests/fixtures/change-impact-schema-v1.json").replace("\r\n", "\n")
        );
        let value = serde_json::to_value(&response).unwrap();
        let projection = serde_json::json!({
            "schema_version": value["schema_version"],
            "baseline": value["baseline"],
            "scope": value["scope"],
            "changes": value["changes"],
            "tests": value["tests"],
            "limits": value["limits"],
            "precision_notes": value["precision_notes"],
        });
        let mut projection = serde_json::to_string_pretty(&projection).unwrap();
        projection.push('\n');
        assert_eq!(
            projection,
            include_str!("../tests/fixtures/change-impact-test-projection-v1.json")
                .replace("\r\n", "\n")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn change_impact_reports_name_collisions_and_edge_precision() {
        let precision = visible_precision("src/lib.rs");
        assert!(precision.iter().any(|value| value == "high:syntactic"));
        let path = tmp_db("impact_collision");
        let store = Store::open(&path).unwrap();
        store
            .insert_symbol("same", "function", "a.rs", 1, 2, None, None)
            .unwrap();
        store
            .insert_symbol("same", "function", "b.rs", 1, 2, None, None)
            .unwrap();
        assert_eq!(store.definition_count("same").unwrap(), 2);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn change_impact_aggregates_multiple_seeds_deterministically() {
        let mut seeds = [
            SeedEvidence {
                file: "b.rs".into(),
                name: "beta".into(),
                kind: "function".into(),
                line: 2,
                change: "body_changed".into(),
            },
            SeedEvidence {
                file: "a.rs".into(),
                name: "alpha".into(),
                kind: "function".into(),
                line: 1,
                change: "added".into(),
            },
        ];
        seeds.sort();
        assert_eq!(seeds[0].name, "alpha");
        assert_eq!(seeds[1].name, "beta");
    }

    #[test]
    fn change_impact_classifies_direct_transitive_and_heuristic_tests() {
        let root = impact_repo(
            "classifications",
            &[
                (
                    "src/app.py",
                    "def target():\n    return 1\n\ndef helper():\n    return target()\n",
                ),
                (
                    "src/test_impact.py",
                    "from src.app import target, helper\n\ndef test_direct():\n    return target()\n\ndef test_transitive():\n    return helper()\n\ndef test_heuristic():\n    return True\n",
                ),
            ],
        );
        write_impact_file(
            &root,
            "src/app.py",
            "def target():\n    return 2\n\ndef helper():\n    return target()\n",
        );
        let store = index_impact(&root, "classifications");
        let response = change_impact(&store, &root, "HEAD", 3, 100).unwrap();
        let classes = response
            .tests
            .items
            .iter()
            .map(|test| (test.symbol.name.as_str(), test.classification.as_str()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(classes.get("test_direct"), Some(&"direct"));
        assert_eq!(classes.get("test_transitive"), Some(&"transitive"));
        assert_eq!(classes.get("test_heuristic"), Some(&"heuristic"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn change_impact_skips_graph_results_at_work_limit() {
        let limited: Collection<ImpactedSymbol> = work_limited_collection(Vec::new());
        assert!(limited.truncated);
        assert_eq!(limited.total, None);
        assert_eq!(limited.truncation_reason.as_deref(), Some("work_limit"));
    }

    #[test]
    fn change_impact_detects_sqlite_data_version_change_through_engine() {
        let root = impact_repo(
            "data_version_race",
            &[("src/app.py", "def value():\n    return 1\n")],
        );
        write_impact_file(&root, "src/app.py", "def value():\n    return 2\n");
        let store = index_impact(&root, "data_version_race");
        let db = store.db_path().to_path_buf();
        let _hook =
            install_impact_test_hook(ImpactTestStage::BeforeDataVersionRecheck, move || {
                let connection = rusqlite::Connection::open(db).unwrap();
                connection
                    .execute(
                        "INSERT INTO meta(key, value) VALUES ('external_race', '1')",
                        [],
                    )
                    .unwrap();
            });
        assert_eq!(
            change_impact(&store, &root, "HEAD", 3, 100).unwrap_err(),
            ChangeImpactError::SnapshotChanged
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn change_impact_detects_same_status_content_race_through_engine() {
        let root = impact_repo(
            "same_status_race",
            &[("src/app.py", "def value():\n    return 1\n")],
        );
        write_impact_file(&root, "src/app.py", "def value():\n    return 2\n");
        let store = index_impact(&root, "same_status_race");
        let changed = root.join("src/app.py");
        let _hook =
            install_impact_test_hook(ImpactTestStage::BeforeGitSnapshotRecheck, move || {
                std::fs::write(changed, "def value():\n    return 3\n").unwrap();
            });
        assert_eq!(
            change_impact(&store, &root, "HEAD", 3, 100).unwrap_err(),
            ChangeImpactError::SnapshotChanged
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn change_impact_detects_head_or_worktree_snapshot_race() {
        let root = impact_repo(
            "head_race",
            &[("src/app.py", "def value():\n    return 1\n")],
        );
        write_impact_file(&root, "src/app.py", "def value():\n    return 2\n");
        let store = index_impact(&root, "head_race");
        let commit_root = root.clone();
        let _hook =
            install_impact_test_hook(ImpactTestStage::BeforeGitSnapshotRecheck, move || {
                let add = std::process::Command::new("git")
                    .args(["add", "src/app.py"])
                    .current_dir(&commit_root)
                    .status()
                    .unwrap();
                assert!(add.success());
                let commit = std::process::Command::new("git")
                    .args(["commit", "-q", "-m", "race"])
                    .current_dir(&commit_root)
                    .status()
                    .unwrap();
                assert!(commit.success());
            });
        assert_eq!(
            change_impact(&store, &root, "HEAD", 3, 100).unwrap_err(),
            ChangeImpactError::SnapshotChanged
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn test_heuristics_are_scoped_in_sql_and_skip_on_probe_overflow() {
        let path = tmp_db("heuristic_scope");
        let store = Store::open(&path).unwrap();
        for index in 0..501 {
            store
                .insert_symbol(
                    &format!("test_{index}"),
                    "function",
                    &format!("src/tests/test_{index}.py"),
                    1,
                    2,
                    None,
                    None,
                )
                .unwrap();
        }
        store
            .insert_symbol(
                "test_outside",
                "function",
                "vendor/test.py",
                1,
                2,
                None,
                None,
            )
            .unwrap();
        let rows = store
            .test_symbols_in_components(&["src".to_string()], 501)
            .unwrap();
        assert_eq!(rows.len(), 501);
        assert!(rows
            .iter()
            .all(|symbol| symbol.file_path.starts_with("src/")));
        std::fs::remove_file(path).ok();
    }

    fn heuristic_engine_fixture(name: &str) -> (PathBuf, Store) {
        let root = impact_repo(name, &[("src/app.py", "def target():\n    return 1\n")]);
        write_impact_file(&root, "src/app.py", "def target():\n    return 2\n");
        let store = index_impact(&root, name);
        for index in 0..49_999 {
            store
                .upsert_file(&format!("src/tests/a{index:05}.py"), 1, 0)
                .unwrap();
        }
        for index in 0..501 {
            store
                .insert_symbol(
                    &format!("helper_{index:03}"),
                    "function",
                    &format!("src/tests/a{index:05}.py"),
                    1,
                    2,
                    None,
                    None,
                )
                .unwrap();
        }
        store
            .insert_symbol(
                "test_survives",
                "function",
                "src/tests/a49998.py",
                1,
                2,
                None,
                None,
            )
            .unwrap();
        (root, store)
    }

    #[test]
    fn heuristic_path_probe_precedes_test_symbol_cap() {
        let (root, store) = heuristic_engine_fixture("heuristic_engine_boundaries");
        let exact = change_impact(&store, &root, "HEAD", 3, 100).unwrap();
        assert_eq!(exact.tests.total, Some(1));
        assert!(!exact.tests.truncated);
        assert_eq!(exact.tests.items.len(), 1);
        assert_eq!(exact.tests.items[0].symbol.name, "test_survives");
        assert_eq!(exact.tests.items[0].classification, "heuristic");

        store.upsert_file("src/tests/overflow.py", 1, 0).unwrap();
        let overflow = change_impact(&store, &root, "HEAD", 3, 100).unwrap();
        assert_eq!(overflow.tests.total, None);
        assert_eq!(overflow.tests.returned, 0);
        assert!(overflow.tests.items.is_empty());
        assert!(overflow.tests.truncated);
        assert_eq!(
            overflow.tests.truncation_reason.as_deref(),
            Some("work_limit")
        );
        assert!(overflow
            .precision_notes
            .contains(&"heuristic_work_limit".to_string()));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn ordinary_symbols_cannot_displace_heuristic_test_symbols() {
        let (root, store) = heuristic_engine_fixture("heuristic_engine_prefilter");
        let response = change_impact(&store, &root, "HEAD", 3, 100).unwrap();
        assert_eq!(response.tests.total, Some(1));
        assert_eq!(response.tests.items.len(), 1);
        assert_eq!(response.tests.items[0].symbol.name, "test_survives");
        assert_eq!(response.tests.items[0].classification, "heuristic");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn test_symbol_markers_exclude_fixtures_and_lifecycle_hooks() {
        assert!(!test_symbol(&mk_sym(
            "setup_method",
            "function",
            "tests/test_app.py",
            1,
            None
        )));
        assert!(!test_symbol(&mk_sym(
            "fixture_value",
            "function",
            "tests/test_app.py",
            1,
            Some(",fixture,")
        )));
        assert!(test_symbol(&mk_sym(
            "test_value",
            "function",
            "tests/test_app.py",
            1,
            None
        )));
    }

    #[test]
    fn impact_collection_exact_limit_is_not_truncated() {
        let exact = bounded_collection(vec![1, 2, 3], 3, "top_limit");
        assert!(!exact.truncated);
        assert_eq!(exact.total, Some(3));
        assert_eq!(exact.truncation_reason, None);
        let overflow = bounded_collection(vec![1, 2, 3, 4], 3, "top_limit");
        assert!(overflow.truncated);
        assert_eq!(overflow.returned, 3);
    }

    #[test]
    fn test_candidate_precedence_merges_evidence_deterministically() {
        let root = impact_repo(
            "precedence",
            &[
                ("src/app.py", "def target():\n    return 1\n"),
                (
                    "src/test_impact.py",
                    "from src.app import target\n\ndef test_direct():\n    return target()\n",
                ),
            ],
        );
        write_impact_file(&root, "src/app.py", "def target():\n    return 2\n");
        write_impact_file(
            &root,
            "src/test_impact.py",
            "from src.app import target\n\ndef test_direct():\n    assert target() == 2\n    return target()\n",
        );
        let store = index_impact(&root, "precedence");
        let response = change_impact(&store, &root, "HEAD", 3, 100).unwrap();
        let candidate = response
            .tests
            .items
            .iter()
            .find(|test| test.symbol.name == "test_direct")
            .unwrap();
        assert_eq!(candidate.classification, "direct");
        assert_eq!(candidate.minimum_depth, Some(0));
        let kinds = candidate
            .evidence
            .iter()
            .map(|evidence| evidence.kind.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                "changed_test_symbol",
                "graph_seed",
                "same_component_test_filename"
            ]
        );
        std::fs::remove_dir_all(root).ok();
    }
}
