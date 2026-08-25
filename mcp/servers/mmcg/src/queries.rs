//! High-level query layer over the Store.
//!
//! Wraps raw store methods with name-based lookup, structured response types,
//! and JSON serialization for the MCP layer.

use crate::store::{
    FileEntry, InterruptSource, MapBoundaryMatch, MapBoundaryScope, ProjectHistoryHit, Store,
    Symbol, TaskSpecHit,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
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
        "py" | "pyi" => EdgePrecision {
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
        "vue" => EdgePrecision {
            confidence: "medium",
            resolution: "syntactic",
            limitations: vec![
                "template attribute expressions not parsed",
                "auto-imported components produce no edge",
                "no type-based dispatch resolution",
            ],
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
    match ext.to_ascii_lowercase().as_str() {
        "rs" => "rust",
        "go" => "go",
        "java" => "java",
        "cs" => "csharp",
        "py" | "pyi" => "python",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "vue" => "vue",
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
    /// `true` when the underlying walk hit `row_limit` rows — the result is a
    /// prefix of the true blast radius, not the whole thing.
    pub truncated: bool,
    /// The row cap applied to this walk (see `IMPACT_WORK_LIMIT`).
    pub row_limit: u32,
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
        let mut namespaces = HashMap::new();
        for symbol in &raw {
            if is_partial_symbol(symbol) {
                if let Some(namespace) = store.enclosing_namespace(symbol.id)? {
                    namespaces.insert(symbol.id, namespace);
                }
            }
        }
        collapse_partial_hits(raw, &namespaces)
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
fn is_partial_symbol(symbol: &Symbol) -> bool {
    symbol
        .decorators
        .as_deref()
        .is_some_and(|decorators| decorators.contains(",partial,"))
}

fn collapse_partial_hits(
    symbols: Vec<Symbol>,
    namespaces: &HashMap<i64, String>,
) -> Vec<SymbolHit> {
    // Namespace is part of partial-type identity. Malformed or legacy rows
    // without a namespace are deliberately isolated by file instead of being
    // merged just because their leaf names match.
    let mut groups: HashMap<(String, String, String), Vec<Symbol>> = HashMap::new();
    let mut order: Vec<(String, String, String)> = Vec::new();
    let mut passthrough: Vec<SymbolHit> = Vec::new();

    for sym in symbols {
        if !is_partial_symbol(&sym) {
            passthrough.push(SymbolHit::from(sym));
            continue;
        }
        let identity = namespaces
            .get(&sym.id)
            .cloned()
            .unwrap_or_else(|| format!("\0file:{}", sym.file_path));
        let key = (sym.name.clone(), sym.kind.clone(), identity);
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
pub struct HistorySearchResponse {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub count: u32,
    /// Direct FTS matches from the indexed Markdown artifacts.
    pub observed: Vec<ProjectHistoryHit>,
    /// Static epistemic contract: the query engine performs retrieval, not reasoning.
    pub inference: &'static str,
    pub source_of_truth: &'static str,
    /// Candidate files omitted because of admission errors or size limits.
    pub skipped_artifacts: u32,
    /// True when the 5,000-artifact work limit omitted candidates.
    pub truncated: bool,
    /// History freshness is deliberately not inferred from structural status.
    pub freshness: &'static str,
}

pub fn history(
    store: &Store,
    query: &str,
    kind: Option<&str>,
    top: u32,
) -> rusqlite::Result<HistorySearchResponse> {
    let data_version_before = store.data_version()?;
    store.begin_read_snapshot()?;
    let snapshot = (|| {
        let observed = store.search_project_history(query, kind, top)?;
        let skipped_artifacts = store
            .meta_value("project_history_skipped")?
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let truncated = store
            .meta_value("project_history_truncated")?
            .is_some_and(|value| value == "true");
        let freshness = store
            .meta_value("index_root")?
            .map(PathBuf::from)
            .map(|root| crate::indexer::Indexer::new(root).project_history_freshness(store))
            .map(|result| match result {
                Ok(crate::indexer::ProjectHistoryFreshness::Fresh) => "fresh",
                Ok(crate::indexer::ProjectHistoryFreshness::Stale) => "stale",
                Ok(crate::indexer::ProjectHistoryFreshness::Incomplete) => "incomplete",
                Ok(crate::indexer::ProjectHistoryFreshness::SnapshotChanged) => "snapshot_changed",
                Err(_) => "incomplete",
            })
            .unwrap_or("stale");
        Ok::<_, rusqlite::Error>((observed, skipped_artifacts, truncated, freshness))
    })();
    let end_result = store.end_read_snapshot();
    let (observed, skipped_artifacts, truncated, mut freshness) = snapshot?;
    end_result?;
    if store.data_version()? != data_version_before {
        freshness = "snapshot_changed";
    }
    Ok(HistorySearchResponse {
        query: query.to_string(),
        kind: kind.map(str::to_string),
        count: observed.len() as u32,
        observed,
        inference: "none; rank and co-occurrence do not establish causality or correctness",
        source_of_truth: "Markdown artifacts at the returned paths; this FTS index is derived",
        skipped_artifacts,
        truncated,
        freshness,
    })
}

#[derive(Debug, Serialize)]
pub struct DependencyCyclesResponse {
    pub count: u32,
    pub min_size: u32,
    /// Each entry is one cycle (SCC) — file paths in lex order.
    pub cycles: Vec<Vec<String>>,
    /// `true` when the file-pair import graph exceeded the work cap —
    /// Tarjan was **not** run, so `cycles` is empty and the true cycle set is
    /// incomplete and possibly inaccurate, not merely "more available".
    /// Narrow the scope (`language` filter) and retry.
    pub truncated: bool,
}

pub fn symbols_changed_since(
    store: &Store,
    repo_root: &std::path::Path,
    git_ref: &str,
) -> Result<crate::diff::SymbolDiff, crate::diff::DiffError> {
    crate::diff::symbols_changed_since(store, repo_root, git_ref)
}

pub(crate) fn symbols_changed_since_controlled(
    store: &Store,
    repo_root: &std::path::Path,
    git_ref: &str,
    deadline: Option<std::time::Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<crate::diff::SymbolDiff, crate::diff::DiffError> {
    crate::diff::symbols_changed_since_controlled(store, repo_root, git_ref, deadline, interrupted)
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
    #[serde(skip)]
    pub(crate) snapshot_token: String,
    #[serde(skip)]
    pub(crate) checked_snapshot: Option<CheckedSnapshotToken>,
    pub baseline: ImpactBaseline,
    pub scope: ImpactScope,
    pub changes: ImpactChanges,
    pub affected_components: Collection<ComponentImpact>,
    pub impact: Collection<ImpactedSymbol>,
    pub api_crossings: Collection<ApiCrossing>,
    pub tests: Collection<TestCandidate>,
    pub disciplines: ImpactDisciplines,
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

/// Which evidence set a change calls for, derived from the changed paths.
#[derive(Debug, Clone, Serialize)]
pub struct ImpactDisciplines {
    pub detected: Vec<DisciplineSignal>,
    pub unclassified: Vec<String>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DisciplineSignal {
    pub name: String,
    pub basis: String,
    pub file_count: u32,
    pub files: Vec<String>,
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

#[cfg(test)]
thread_local! {
    static BRIEF_TEST_HOOK: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}

#[cfg(test)]
pub(crate) struct BriefTestHookGuard;

#[cfg(test)]
impl Drop for BriefTestHookGuard {
    fn drop(&mut self) {
        BRIEF_TEST_HOOK.with(|hook| hook.borrow_mut().take());
    }
}

#[cfg(test)]
pub(crate) fn install_brief_test_hook(hook: impl FnOnce() + 'static) -> BriefTestHookGuard {
    BRIEF_TEST_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    BriefTestHookGuard
}

#[cfg(test)]
fn run_brief_test_hook() {
    let hook = BRIEF_TEST_HOOK.with(|slot| slot.borrow_mut().take());
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

pub const BRIEF_MIN_BUDGET_TOKENS: u32 = 256;
pub const BRIEF_MAX_BUDGET_TOKENS: u32 = 8_000;
pub const BRIEF_DEFAULT_BUDGET_TOKENS: u32 = 2_000;
const BRIEF_CHANGED_FILE_LIMIT: usize = 100;
const BRIEF_CHANGED_SYMBOL_LIMIT: usize = 100;
const BRIEF_CALLER_LIMIT: usize = 100;
const BRIEF_TEST_LIMIT: usize = 50;
const BRIEF_HISTORY_LIMIT: usize = 10;
const BRIEF_HISTORY_TERM_LIMIT: usize = 8;
const BRIEF_REPOSITORY_STRING_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BriefRole {
    Planner,
    Executor,
    Auditor,
}

impl BriefRole {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "planner" => Some(Self::Planner),
            "executor" => Some(Self::Executor),
            "auditor" => Some(Self::Auditor),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planner => "planner",
            Self::Executor => "executor",
            Self::Auditor => "auditor",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BriefError {
    InvalidArguments,
    InvalidRef,
    RootMismatch,
    IndexStale,
    SchemaIncompatible,
    SnapshotChanged,
    WorkLimitExceeded,
    Serialization,
    BudgetTooSmall { minimum_tokens: u32 },
}

impl BriefError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::InvalidRef => "invalid_ref",
            Self::RootMismatch => "root_mismatch",
            Self::IndexStale => "index_stale",
            Self::SchemaIncompatible => "schema_incompatible",
            Self::SnapshotChanged => "snapshot_changed",
            Self::WorkLimitExceeded => "work_limit_exceeded",
            Self::Serialization => "serialization_failed",
            Self::BudgetTooSmall { .. } => "budget_too_small",
        }
    }
}

impl std::fmt::Display for BriefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for BriefError {}

impl From<ChangeImpactError> for BriefError {
    fn from(error: ChangeImpactError) -> Self {
        match error {
            ChangeImpactError::InvalidRef => Self::InvalidRef,
            ChangeImpactError::RootMismatch => Self::RootMismatch,
            ChangeImpactError::IndexStale => Self::IndexStale,
            ChangeImpactError::SnapshotChanged => Self::SnapshotChanged,
            ChangeImpactError::GitTimeout | ChangeImpactError::GitOutputLimit => {
                Self::WorkLimitExceeded
            }
        }
    }
}

pub type BriefEnvelopeSizer<'a> = dyn Fn(&BriefPacket) -> Result<usize, BriefError> + 'a;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BriefPacket {
    pub schema_version: u32,
    pub repository_content_untrusted: bool,
    pub role: BriefRole,
    pub freshness: BriefFreshness,
    pub baseline: BriefBaseline,
    pub scope: BriefScope,
    pub budget: BriefBudget,
    pub changes: BriefChanges,
    pub callers: BriefCollection<BriefCaller>,
    pub tests: BriefCollection<BriefTest>,
    pub history: BriefHistory,
    pub citations: BriefCollection<BriefHistoryCitation>,
    pub omitted: BriefOmitted,
    pub limits: BriefLimits,
    pub precision_notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BriefFreshness {
    pub structural: BriefFreshnessState,
    pub history: BriefFreshnessState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BriefFreshnessState {
    pub status: String,
    pub checked_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BriefBaseline {
    pub requested_ref: String,
    pub baseline_oid: String,
    pub head_oid: String,
    pub includes_worktree: bool,
    pub includes_untracked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BriefScope {
    pub repository_relative_root: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BriefBudget {
    pub requested_tokens: u32,
    pub estimated_tokens: u32,
    pub final_envelope_bytes: u32,
    pub minimum_tokens: u32,
    pub bytes_per_token: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BriefCollection<T> {
    /// Exact candidate count, or null when an upstream work cap only proves a
    /// lower bound.
    pub total: Option<u32>,
    pub returned: u32,
    pub items: Vec<T>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BriefChanges {
    pub files: BriefCollection<BriefChangedFile>,
    pub symbols: BriefCollection<BriefChangedSymbol>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BriefChangedFile {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BriefChangedSymbol {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub change: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BriefCaller {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub minimum_depth: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BriefTest {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub classification: String,
    pub minimum_depth: Option<u32>,
    pub confidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BriefHistory {
    pub query_terms: Vec<String>,
    pub query_performed: bool,
    pub empty_reason: Option<String>,
    pub total: Option<u32>,
    pub returned: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BriefHistoryCitation {
    pub path: String,
    pub kind: String,
    pub matched_terms: Vec<String>,
    pub rank: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct BriefOmissionCount {
    /// Exact count when `source_limit_exact` is true, otherwise a lower bound.
    pub source_limit: u32,
    pub source_limit_exact: bool,
    pub unsafe_content: u32,
    pub budget: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BriefOmitted {
    pub changed_files: BriefOmissionCount,
    pub changed_symbols: BriefOmissionCount,
    pub callers: BriefOmissionCount,
    pub tests: BriefOmissionCount,
    pub history_citations: BriefOmissionCount,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BriefLimits {
    pub changed_files: u32,
    pub changed_symbols: u32,
    pub callers: u32,
    pub tests: u32,
    pub history_citations: u32,
    pub history_terms: u32,
    pub impact_depth: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckedSnapshotToken {
    data_version: u64,
    structural_worktree_token: String,
    history_inventory_token: String,
    history_freshness: crate::indexer::ProjectHistoryFreshness,
}

struct StoreReadSnapshot<'a> {
    store: &'a Store,
    active: bool,
}

impl<'a> StoreReadSnapshot<'a> {
    fn begin(store: &'a Store) -> rusqlite::Result<Self> {
        store.begin_read_snapshot()?;
        Ok(Self {
            store,
            active: true,
        })
    }

    fn finish(mut self) -> rusqlite::Result<()> {
        self.store.end_read_snapshot()?;
        self.active = false;
        Ok(())
    }
}

impl Drop for StoreReadSnapshot<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ = self.store.end_read_snapshot();
        }
    }
}

fn history_snapshot_error(error: crate::indexer::IndexError) -> ChangeImpactError {
    match error {
        crate::indexer::IndexError::Cancelled | crate::indexer::IndexError::DeadlineExceeded => {
            ChangeImpactError::GitTimeout
        }
        crate::indexer::IndexError::SnapshotChanged => ChangeImpactError::SnapshotChanged,
        _ => ChangeImpactError::IndexStale,
    }
}

pub(crate) fn checked_snapshot_token(
    store: &Store,
    repository_root: &Path,
    structural_worktree_token: &str,
) -> Result<CheckedSnapshotToken, ChangeImpactError> {
    let data_version = store
        .data_version()
        .map_err(|_| ChangeImpactError::SnapshotChanged)?;
    let (history_inventory_token, history_freshness) =
        crate::indexer::Indexer::new(repository_root)
            .live_project_history_inventory(store)
            .map_err(history_snapshot_error)?;
    if history_freshness == crate::indexer::ProjectHistoryFreshness::SnapshotChanged {
        return Err(ChangeImpactError::SnapshotChanged);
    }
    Ok(CheckedSnapshotToken {
        data_version,
        structural_worktree_token: structural_worktree_token.to_string(),
        history_inventory_token,
        history_freshness,
    })
}

fn validate_checked_snapshot(
    store: &Store,
    repository_root: &Path,
    expected: &CheckedSnapshotToken,
    structural_worktree_token: &str,
) -> Result<(), ChangeImpactError> {
    if structural_worktree_token != expected.structural_worktree_token
        || store
            .data_version()
            .map_err(|_| ChangeImpactError::SnapshotChanged)?
            != expected.data_version
    {
        return Err(ChangeImpactError::SnapshotChanged);
    }
    let (history_inventory_token, freshness) = crate::indexer::Indexer::new(repository_root)
        .live_project_history_inventory(store)
        .map_err(history_snapshot_error)?;
    if freshness == crate::indexer::ProjectHistoryFreshness::SnapshotChanged
        || history_inventory_token != expected.history_inventory_token
        || freshness != expected.history_freshness
    {
        return Err(ChangeImpactError::SnapshotChanged);
    }
    Ok(())
}

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

const DISCIPLINE_SAMPLE: usize = 20;

const DISCIPLINE_NOTE: &str = "Path-based classification proposes an evidence set; \
it does not replace reading the change. Only what a path can establish is classified — \
component file types, test-file naming, and a SQL file or migrations directory. \
Anything else is listed unclassified rather than guessed, so a queue consumer in a \
plain `.ts` file will not be labelled. A detected `migration` is a strict-mode trigger, \
not a verdict about what the migration does.";

fn migration_like_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".sql") {
        return true;
    }
    lower
        .split('/')
        .any(|part| matches!(part, "migrations" | "migration" | "migrate"))
}

fn frontend_like_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let ext = match lower.rsplit_once('.') {
        Some((_, ext)) => ext,
        None => return false,
    };
    matches!(
        ext,
        "tsx" | "jsx" | "vue" | "css" | "scss" | "sass" | "less"
    )
}

struct DisciplineRule {
    name: &'static str,
    basis: &'static str,
    matches: fn(&str) -> bool,
}

fn classify_disciplines(files: &[ChangedFile]) -> ImpactDisciplines {
    let rules = [
        DisciplineRule {
            name: "frontend",
            basis: "component or stylesheet file type",
            matches: frontend_like_path,
        },
        DisciplineRule {
            name: "qa",
            basis: "test-file naming convention",
            matches: test_like_path,
        },
        DisciplineRule {
            name: "migration",
            basis: "SQL file type or a migrations directory",
            matches: migration_like_path,
        },
    ];

    let mut detected = Vec::new();
    let mut classified = std::collections::HashSet::<&str>::new();
    for rule in rules {
        let hits: Vec<&str> = files
            .iter()
            .map(|file| file.path.as_str())
            .filter(|path| (rule.matches)(path))
            .collect();
        if hits.is_empty() {
            continue;
        }
        classified.extend(hits.iter().copied());
        detected.push(DisciplineSignal {
            name: rule.name.to_string(),
            basis: rule.basis.to_string(),
            file_count: hits.len() as u32,
            files: hits
                .iter()
                .take(DISCIPLINE_SAMPLE)
                .map(|path| path.to_string())
                .collect(),
        });
    }

    let unclassified: Vec<String> = files
        .iter()
        .map(|file| file.path.as_str())
        .filter(|path| !classified.contains(path))
        .take(DISCIPLINE_SAMPLE)
        .map(|path| path.to_string())
        .collect();

    ImpactDisciplines {
        detected,
        unclassified,
        note: DISCIPLINE_NOTE.to_string(),
    }
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

fn consume_graph_precision_interrupt(
    store: &Store,
    had_parent_budget: bool,
    interrupt_before: Option<InterruptSource>,
) -> bool {
    match store.interrupt_source() {
        None => true,
        Some(InterruptSource::Budget) if !had_parent_budget && interrupt_before.is_none() => {
            store.consume_budget_interrupt()
        }
        Some(InterruptSource::Budget | InterruptSource::Cancel) => false,
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
    let data_version_before = store
        .data_version()
        .map_err(|_| ChangeImpactError::SnapshotChanged)?;
    let read_snapshot =
        StoreReadSnapshot::begin(store).map_err(|_| ChangeImpactError::SnapshotChanged)?;
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
    let root_capability = crate::bounded_fs::RootCapability::open(&repository_root)
        .map_err(|_| ChangeImpactError::SnapshotChanged)?;
    let scope = requested_root
        .strip_prefix(&repository_root)
        .ok()
        .filter(|value| !value.as_os_str().is_empty())
        .map(|value| value.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| ".".to_string());

    let interrupted = || store.work_interrupted();
    let working = crate::diff::symbols_changed_in_worktree_controlled(
        store,
        &repository_root,
        git_ref,
        store.request_deadline(),
        Some(&interrupted),
    )?;
    for file in &working.files {
        if file.status == "deleted"
            || crate::indexer::extractor_for_path(Path::new(&file.path)).is_none()
        {
            continue;
        }
        let bytes = crate::bounded_fs::read_regular_file_with_capability(
            &root_capability,
            Path::new(&file.path),
            crate::indexer::MAX_INDEXABLE_FILE_SIZE,
            crate::indexer::MAX_INDEXABLE_FILE_SIZE,
            crate::bounded_fs::ReadControl {
                deadline: store.request_deadline(),
                interrupted: Some(&interrupted),
            },
        )
        .map_err(|error| match error {
            crate::bounded_fs::BoundedReadError::TooLarge { .. } => ChangeImpactError::IndexStale,
            crate::bounded_fs::BoundedReadError::Interrupted
            | crate::bounded_fs::BoundedReadError::DeadlineExceeded => {
                ChangeImpactError::GitTimeout
            }
            _ => ChangeImpactError::SnapshotChanged,
        })?
        .bytes;
        let digest = crate::hex::encode(&Sha256::digest(bytes));
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

    let checked_snapshot =
        checked_snapshot_token(store, &repository_root, &working.snapshot_token)?;

    let mut precision_notes = vec!["focused_tests_do_not_replace_full_gate".to_string()];
    let graph_seed_overflow = seed_names.len() > CHANGE_SEED_LIMIT;
    let graph_had_parent_budget = store.work_budget_depth() > 0;
    let graph_interrupt_before = store.interrupt_source();
    let (graph_rows, graph_budget_exhausted) = if seed_names.is_empty() || graph_seed_overflow {
        (Vec::new(), false)
    } else {
        match store.impact_of_many(&seed_names, max_depth, IMPACT_WORK_LIMIT, None) {
            Ok(rows) => (rows, false),
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::OperationInterrupted
                    && consume_graph_precision_interrupt(
                        store,
                        graph_had_parent_budget,
                        graph_interrupt_before,
                    ) =>
            {
                precision_notes.push("graph_work_limit".to_string());
                (Vec::new(), true)
            }
            Err(_) => return Err(ChangeImpactError::SnapshotChanged),
        }
    };
    let graph_overflow =
        graph_seed_overflow || graph_budget_exhausted || graph_rows.len() >= IMPACT_WORK_LIMIT;
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
    read_snapshot
        .finish()
        .map_err(|_| ChangeImpactError::SnapshotChanged)?;
    if store
        .data_version()
        .map_err(|_| ChangeImpactError::SnapshotChanged)?
        != data_version_before
    {
        return Err(ChangeImpactError::SnapshotChanged);
    }
    #[cfg(test)]
    run_impact_test_hook(ImpactTestStage::BeforeGitSnapshotRecheck);
    crate::diff::validate_working_tree_snapshot_controlled(
        &repository_root,
        &working.baseline_oid,
        &working.head_oid,
        &working.files,
        &working.snapshot_token,
        store.request_deadline(),
        Some(&interrupted),
    )?;
    validate_checked_snapshot(
        store,
        &repository_root,
        &checked_snapshot,
        &working.snapshot_token,
    )?;
    root_capability
        .verify()
        .map_err(|_| ChangeImpactError::SnapshotChanged)?;

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
    let disciplines = classify_disciplines(&files);
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
        snapshot_token: working.snapshot_token,
        checked_snapshot: Some(checked_snapshot),
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
        disciplines,
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

fn brief_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[derive(Clone, Copy)]
struct BriefSourceLimit {
    total: Option<u32>,
    omitted: u32,
    exact: bool,
}

fn brief_source_omitted<T>(collection: &Collection<T>, cap: usize) -> BriefSourceLimit {
    let admitted = collection.returned.min(brief_u32(cap));
    match collection.total {
        Some(total) => BriefSourceLimit {
            total: Some(total),
            omitted: total.saturating_sub(admitted),
            exact: true,
        },
        None => BriefSourceLimit {
            total: None,
            omitted: collection
                .returned
                .saturating_sub(admitted)
                .saturating_add(u32::from(collection.truncated)),
            exact: !collection.truncated,
        },
    }
}

fn brief_history_source_limit(
    returned: u32,
    skipped_artifacts: u32,
    corpus_truncated: bool,
) -> BriefSourceLimit {
    let exact =
        returned < BRIEF_HISTORY_LIMIT as u32 && skipped_artifacts == 0 && !corpus_truncated;
    BriefSourceLimit {
        total: exact.then_some(returned),
        omitted: 0,
        exact,
    }
}

fn is_unsafe_display_char(value: char) -> bool {
    value.is_control()
        || matches!(
            value,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

/// Repository-derived strings are data, never instructions. Preserve useful
/// Unicode while making terminal/control direction changes visible and
/// bounding the post-escape representation before JSON serialization.
fn safe_brief_string(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let mut output = String::new();
    let mut piece_starts = Vec::new();
    let mut visible = false;
    let mut truncated = false;
    for character in value.chars() {
        let piece = if is_unsafe_display_char(character) {
            format!("\\u{{{:04X}}}", character as u32)
        } else {
            visible = true;
            character.to_string()
        };
        if output.len().saturating_add(piece.len()) > BRIEF_REPOSITORY_STRING_BYTES {
            truncated = true;
            break;
        }
        piece_starts.push(output.len());
        output.push_str(&piece);
    }
    if !visible {
        return None;
    }
    if truncated {
        while output.len().saturating_add(3) > BRIEF_REPOSITORY_STRING_BYTES {
            output.truncate(piece_starts.pop().unwrap_or(0));
        }
        output.push_str("...");
    }
    Some(output)
}

fn brief_history_terms(changes: &ImpactChanges) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "added",
        "body",
        "change",
        "changed",
        "code",
        "file",
        "files",
        "main",
        "removed",
        "signature",
        "source",
        "src",
        "test",
        "tests",
    ];
    fn add_terms(value: &str, seen: &mut BTreeSet<String>, output: &mut Vec<String>) {
        for part in
            value.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        {
            let term = part.to_ascii_lowercase();
            if !(3..=64).contains(&term.len())
                || STOP_WORDS.binary_search(&term.as_str()).is_ok()
                || !seen.insert(term.clone())
            {
                continue;
            }
            output.push(term);
            if output.len() == BRIEF_HISTORY_TERM_LIMIT {
                return;
            }
        }
    }

    let mut symbols = changes
        .symbols
        .items
        .iter()
        .take(BRIEF_CHANGED_SYMBOL_LIMIT)
        .collect::<Vec<_>>();
    symbols.sort_by(|left, right| {
        (&left.file, &left.name, left.line, &left.kind, &left.change).cmp(&(
            &right.file,
            &right.name,
            right.line,
            &right.kind,
            &right.change,
        ))
    });
    let mut seen = BTreeSet::new();
    let mut terms = Vec::new();
    for symbol in symbols {
        add_terms(&symbol.name, &mut seen, &mut terms);
        if terms.len() == BRIEF_HISTORY_TERM_LIMIT {
            return terms;
        }
    }
    let mut paths = changes
        .files
        .items
        .iter()
        .take(BRIEF_CHANGED_FILE_LIMIT)
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    paths.sort_unstable();
    paths.dedup();
    for path in paths {
        add_terms(path, &mut seen, &mut terms);
        if terms.len() == BRIEF_HISTORY_TERM_LIMIT {
            break;
        }
    }
    terms
}

fn brief_history_query(terms: &[String]) -> String {
    terms
        .iter()
        .map(|term| format!("\"{term}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn history_status(freshness: crate::indexer::ProjectHistoryFreshness) -> &'static str {
    match freshness {
        crate::indexer::ProjectHistoryFreshness::Fresh => "fresh",
        crate::indexer::ProjectHistoryFreshness::Stale => "stale",
        crate::indexer::ProjectHistoryFreshness::Incomplete => "incomplete",
        crate::indexer::ProjectHistoryFreshness::SnapshotChanged => "snapshot_changed",
    }
}

fn sync_brief_counts(packet: &mut BriefPacket) {
    packet.changes.files.returned = brief_u32(packet.changes.files.items.len());
    packet.changes.symbols.returned = brief_u32(packet.changes.symbols.items.len());
    packet.callers.returned = brief_u32(packet.callers.items.len());
    packet.tests.returned = brief_u32(packet.tests.items.len());
    packet.citations.returned = brief_u32(packet.citations.items.len());
    packet.history.total = packet.citations.total;
    packet.history.returned = packet.citations.returned;
}

#[derive(Clone, Copy)]
enum BriefSection {
    Changes,
    Callers,
    Tests,
    History,
}

fn brief_priority(role: BriefRole) -> [BriefSection; 4] {
    match role {
        BriefRole::Planner => [
            BriefSection::Changes,
            BriefSection::Callers,
            BriefSection::History,
            BriefSection::Tests,
        ],
        BriefRole::Executor => [
            BriefSection::Changes,
            BriefSection::Tests,
            BriefSection::Callers,
            BriefSection::History,
        ],
        BriefRole::Auditor => [
            BriefSection::Tests,
            BriefSection::Callers,
            BriefSection::Changes,
            BriefSection::History,
        ],
    }
}

fn remove_brief_candidate(packet: &mut BriefPacket) -> bool {
    for section in brief_priority(packet.role).into_iter().rev() {
        let removed = match section {
            BriefSection::Changes => {
                if packet.changes.symbols.items.pop().is_some() {
                    packet.omitted.changed_symbols.budget =
                        packet.omitted.changed_symbols.budget.saturating_add(1);
                    true
                } else if packet.changes.files.items.pop().is_some() {
                    packet.omitted.changed_files.budget =
                        packet.omitted.changed_files.budget.saturating_add(1);
                    true
                } else {
                    false
                }
            }
            BriefSection::Callers => {
                if packet.callers.items.pop().is_some() {
                    packet.omitted.callers.budget = packet.omitted.callers.budget.saturating_add(1);
                    true
                } else {
                    false
                }
            }
            BriefSection::Tests => {
                if packet.tests.items.pop().is_some() {
                    packet.omitted.tests.budget = packet.omitted.tests.budget.saturating_add(1);
                    true
                } else {
                    false
                }
            }
            BriefSection::History => {
                if packet.citations.items.pop().is_some() {
                    packet.omitted.history_citations.budget =
                        packet.omitted.history_citations.budget.saturating_add(1);
                    true
                } else {
                    false
                }
            }
        };
        if removed {
            sync_brief_counts(packet);
            return true;
        }
    }
    false
}

fn clear_brief_candidates(packet: &mut BriefPacket) {
    packet.omitted.changed_files.budget = packet
        .omitted
        .changed_files
        .budget
        .saturating_add(brief_u32(packet.changes.files.items.len()));
    packet.omitted.changed_symbols.budget = packet
        .omitted
        .changed_symbols
        .budget
        .saturating_add(brief_u32(packet.changes.symbols.items.len()));
    packet.omitted.callers.budget = packet
        .omitted
        .callers
        .budget
        .saturating_add(brief_u32(packet.callers.items.len()));
    packet.omitted.tests.budget = packet
        .omitted
        .tests
        .budget
        .saturating_add(brief_u32(packet.tests.items.len()));
    packet.omitted.history_citations.budget = packet
        .omitted
        .history_citations
        .budget
        .saturating_add(brief_u32(packet.citations.items.len()));
    packet.changes.files.items.clear();
    packet.changes.symbols.items.clear();
    packet.callers.items.clear();
    packet.tests.items.clear();
    packet.citations.items.clear();
    sync_brief_counts(packet);
}

fn stabilize_brief_budget(
    packet: &mut BriefPacket,
    envelope_sizer: &BriefEnvelopeSizer<'_>,
) -> Result<u32, BriefError> {
    for _ in 0..32 {
        let bytes = envelope_sizer(packet)?;
        let tokens = brief_u32(bytes.saturating_add(3) / 4);
        let bytes = brief_u32(bytes);
        if packet.budget.final_envelope_bytes == bytes && packet.budget.estimated_tokens == tokens {
            return Ok(tokens);
        }
        packet.budget.final_envelope_bytes = bytes;
        packet.budget.estimated_tokens = tokens;
    }
    Err(BriefError::Serialization)
}

fn minimum_brief_tokens(
    packet: &BriefPacket,
    envelope_sizer: &BriefEnvelopeSizer<'_>,
) -> Result<u32, BriefError> {
    let mut minimum = packet.clone();
    clear_brief_candidates(&mut minimum);
    minimum.budget.minimum_tokens = 0;
    for _ in 0..32 {
        let tokens = stabilize_brief_budget(&mut minimum, envelope_sizer)?;
        if minimum.budget.minimum_tokens == tokens {
            return Ok(tokens);
        }
        minimum.budget.minimum_tokens = tokens;
    }
    Err(BriefError::Serialization)
}

fn apply_brief_budget(
    mut packet: BriefPacket,
    envelope_sizer: &BriefEnvelopeSizer<'_>,
) -> Result<BriefPacket, BriefError> {
    let minimum_tokens = minimum_brief_tokens(&packet, envelope_sizer)?;
    if packet.budget.requested_tokens < minimum_tokens {
        return Err(BriefError::BudgetTooSmall { minimum_tokens });
    }
    packet.budget.minimum_tokens = minimum_tokens;
    let mut estimated = stabilize_brief_budget(&mut packet, envelope_sizer)?;
    while estimated > packet.budget.requested_tokens {
        if !remove_brief_candidate(&mut packet) {
            return Err(BriefError::BudgetTooSmall { minimum_tokens });
        }
        estimated = stabilize_brief_budget(&mut packet, envelope_sizer)?;
    }
    Ok(packet)
}

fn safe_brief_collection<T>(
    candidates: impl IntoIterator<Item = Option<T>>,
    source_limit: BriefSourceLimit,
) -> (BriefCollection<T>, BriefOmissionCount) {
    let mut unsafe_content = 0u32;
    let items = candidates
        .into_iter()
        .filter_map(|candidate| match candidate {
            Some(candidate) => Some(candidate),
            None => {
                unsafe_content = unsafe_content.saturating_add(1);
                None
            }
        })
        .collect::<Vec<_>>();
    (
        BriefCollection {
            total: source_limit.total,
            returned: brief_u32(items.len()),
            items,
        },
        BriefOmissionCount {
            source_limit: source_limit.omitted,
            source_limit_exact: source_limit.exact,
            unsafe_content,
            budget: 0,
        },
    )
}

pub fn validate_brief_request(since: &str, budget_tokens: u32) -> Result<(), BriefError> {
    if since.is_empty()
        || since.len() > 1_024
        || since.starts_with('-')
        || since.contains('\0')
        || !(BRIEF_MIN_BUDGET_TOKENS..=BRIEF_MAX_BUDGET_TOKENS).contains(&budget_tokens)
    {
        return Err(BriefError::InvalidArguments);
    }
    Ok(())
}

/// Build one deterministic, revision-bound context packet. The caller supplies
/// the exact MCP result-envelope sizer so admission accounts for protocol
/// escaping and duplication without coupling the query layer to MCP framing.
pub fn brief(
    store: &Store,
    requested_root: &Path,
    since: &str,
    role: BriefRole,
    budget_tokens: u32,
    envelope_sizer: &BriefEnvelopeSizer<'_>,
) -> Result<BriefPacket, BriefError> {
    validate_brief_request(since, budget_tokens)?;
    let requested_root = requested_root
        .canonicalize()
        .map_err(|_| BriefError::RootMismatch)?;
    let impact = change_impact(store, &requested_root, since, 3, BRIEF_CALLER_LIMIT)?;
    let repository_root = owning_repository(&requested_root).ok_or(BriefError::RootMismatch)?;
    let root_capability = crate::bounded_fs::RootCapability::open(&repository_root)
        .map_err(|_| BriefError::SnapshotChanged)?;
    let checked = impact
        .checked_snapshot
        .clone()
        .ok_or(BriefError::SnapshotChanged)?;
    match checked.history_freshness {
        crate::indexer::ProjectHistoryFreshness::Stale => return Err(BriefError::IndexStale),
        crate::indexer::ProjectHistoryFreshness::SnapshotChanged => {
            return Err(BriefError::SnapshotChanged)
        }
        crate::indexer::ProjectHistoryFreshness::Fresh
        | crate::indexer::ProjectHistoryFreshness::Incomplete => {}
    }

    let terms = brief_history_terms(&impact.changes);
    let query = brief_history_query(&terms);
    let history_source_limit;
    let mut raw_history = Vec::new();
    let query_performed = !terms.is_empty();
    if query_performed {
        let response = history(store, &query, None, BRIEF_HISTORY_LIMIT as u32)
            .map_err(|_| BriefError::IndexStale)?;
        match response.freshness {
            "stale" => return Err(BriefError::IndexStale),
            "snapshot_changed" => return Err(BriefError::SnapshotChanged),
            _ => {}
        }
        history_source_limit = brief_history_source_limit(
            response.count,
            response.skipped_artifacts,
            response.truncated,
        );
        raw_history = response.observed;
    } else {
        history_source_limit = BriefSourceLimit {
            total: Some(0),
            omitted: 0,
            exact: true,
        };
    }

    let expected_files = impact
        .changes
        .files
        .items
        .iter()
        .map(|file| crate::diff::WorkingTreeChangedFile {
            path: file.path.clone(),
            status: file.status.clone(),
        })
        .collect::<Vec<_>>();
    #[cfg(test)]
    run_brief_test_hook();
    let interrupted = || store.work_interrupted();
    crate::diff::validate_working_tree_snapshot_controlled(
        &repository_root,
        &impact.baseline.baseline_oid,
        &impact.baseline.head_oid,
        &expected_files,
        &impact.snapshot_token,
        store.request_deadline(),
        Some(&interrupted),
    )
    .map_err(ChangeImpactError::from)?;
    validate_checked_snapshot(store, &repository_root, &checked, &impact.snapshot_token)?;
    root_capability
        .verify()
        .map_err(|_| BriefError::SnapshotChanged)?;

    let file_source_limit = brief_source_omitted(&impact.changes.files, BRIEF_CHANGED_FILE_LIMIT);
    let (files, omitted_files) = safe_brief_collection(
        impact
            .changes
            .files
            .items
            .iter()
            .take(BRIEF_CHANGED_FILE_LIMIT)
            .map(|file| {
                Some(BriefChangedFile {
                    path: safe_brief_string(&file.path)?,
                    status: safe_brief_string(&file.status)?,
                })
            }),
        file_source_limit,
    );

    let symbol_source_limit =
        brief_source_omitted(&impact.changes.symbols, BRIEF_CHANGED_SYMBOL_LIMIT);
    let (symbols, omitted_symbols) = safe_brief_collection(
        impact
            .changes
            .symbols
            .items
            .iter()
            .take(BRIEF_CHANGED_SYMBOL_LIMIT)
            .map(|symbol| {
                Some(BriefChangedSymbol {
                    file: safe_brief_string(&symbol.file)?,
                    name: safe_brief_string(&symbol.name)?,
                    kind: safe_brief_string(&symbol.kind)?,
                    line: symbol.line,
                    change: safe_brief_string(&symbol.change)?,
                })
            }),
        symbol_source_limit,
    );

    let caller_source_limit = brief_source_omitted(&impact.impact, BRIEF_CALLER_LIMIT);
    let (callers, omitted_callers) = safe_brief_collection(
        impact
            .impact
            .items
            .iter()
            .take(BRIEF_CALLER_LIMIT)
            .map(|caller| {
                Some(BriefCaller {
                    file: safe_brief_string(&caller.symbol.file)?,
                    name: safe_brief_string(&caller.symbol.name)?,
                    kind: safe_brief_string(&caller.symbol.kind)?,
                    line: caller.symbol.line,
                    minimum_depth: caller.minimum_depth,
                })
            }),
        caller_source_limit,
    );

    let test_source_limit = brief_source_omitted(&impact.tests, BRIEF_TEST_LIMIT);
    let (tests, omitted_tests) = safe_brief_collection(
        impact
            .tests
            .items
            .iter()
            .take(BRIEF_TEST_LIMIT)
            .map(|test| {
                Some(BriefTest {
                    file: safe_brief_string(&test.symbol.file)?,
                    name: safe_brief_string(&test.symbol.name)?,
                    kind: safe_brief_string(&test.symbol.kind)?,
                    line: test.symbol.line,
                    classification: safe_brief_string(&test.classification)?,
                    minimum_depth: test.minimum_depth,
                    confidence: safe_brief_string(&test.confidence)?,
                })
            }),
        test_source_limit,
    );

    raw_history.sort_by(|left, right| {
        left.score
            .total_cmp(&right.score)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.kind.cmp(&right.kind))
    });
    let (citations, omitted_history) = safe_brief_collection(
        raw_history
            .into_iter()
            .take(BRIEF_HISTORY_LIMIT)
            .enumerate()
            .map(|(index, hit)| {
                let matched_terms = hit
                    .matched_terms
                    .iter()
                    .filter_map(|term| safe_brief_string(term))
                    .take(BRIEF_HISTORY_TERM_LIMIT)
                    .collect::<Vec<_>>();
                if matched_terms.is_empty() {
                    return None;
                }
                Some(BriefHistoryCitation {
                    path: safe_brief_string(&hit.path)?,
                    kind: safe_brief_string(&hit.kind)?,
                    matched_terms,
                    rank: brief_u32(index.saturating_add(1)),
                })
            }),
        history_source_limit,
    );

    let structural_token =
        safe_brief_string(&checked.structural_worktree_token).ok_or(BriefError::SnapshotChanged)?;
    let history_token =
        safe_brief_string(&checked.history_inventory_token).ok_or(BriefError::SnapshotChanged)?;
    let mut precision_notes = impact.precision_notes.clone();
    if checked.history_freshness == crate::indexer::ProjectHistoryFreshness::Incomplete {
        precision_notes.push("history_index_incomplete".to_string());
    }
    precision_notes.sort();
    precision_notes.dedup();
    let citations_total = citations.total;
    let citations_returned = citations.returned;
    let packet = BriefPacket {
        schema_version: 1,
        repository_content_untrusted: true,
        role,
        freshness: BriefFreshness {
            structural: BriefFreshnessState {
                status: "fresh".to_string(),
                checked_token: structural_token,
            },
            history: BriefFreshnessState {
                status: history_status(checked.history_freshness).to_string(),
                checked_token: history_token,
            },
        },
        baseline: BriefBaseline {
            requested_ref: safe_brief_string(&impact.baseline.requested_ref)
                .ok_or(BriefError::InvalidRef)?,
            baseline_oid: impact.baseline.baseline_oid,
            head_oid: impact.baseline.head_oid,
            includes_worktree: impact.baseline.includes_worktree,
            includes_untracked: impact.baseline.includes_untracked,
        },
        scope: BriefScope {
            repository_relative_root: safe_brief_string(&impact.scope.repository_relative_root)
                .ok_or(BriefError::RootMismatch)?,
        },
        budget: BriefBudget {
            requested_tokens: budget_tokens,
            estimated_tokens: 0,
            final_envelope_bytes: 0,
            minimum_tokens: 0,
            bytes_per_token: 4,
        },
        changes: BriefChanges { files, symbols },
        callers,
        tests,
        history: BriefHistory {
            query_terms: terms,
            query_performed,
            empty_reason: (!query_performed).then(|| "no_eligible_changed_terms".to_string()),
            total: citations_total,
            returned: citations_returned,
        },
        citations,
        omitted: BriefOmitted {
            changed_files: omitted_files,
            changed_symbols: omitted_symbols,
            callers: omitted_callers,
            tests: omitted_tests,
            history_citations: omitted_history,
        },
        limits: BriefLimits {
            changed_files: BRIEF_CHANGED_FILE_LIMIT as u32,
            changed_symbols: BRIEF_CHANGED_SYMBOL_LIMIT as u32,
            callers: BRIEF_CALLER_LIMIT as u32,
            tests: BRIEF_TEST_LIMIT as u32,
            history_citations: BRIEF_HISTORY_LIMIT as u32,
            history_terms: BRIEF_HISTORY_TERM_LIMIT as u32,
            impact_depth: 3,
        },
        precision_notes,
    };
    apply_brief_budget(packet, envelope_sizer)
}

pub fn dependency_cycles(
    store: &Store,
    language: Option<&str>,
    min_size: u32,
) -> rusqlite::Result<DependencyCyclesResponse> {
    let (cycles, truncated) = store.dependency_cycles(language, min_size as usize)?;
    Ok(DependencyCyclesResponse {
        count: cycles.len() as u32,
        min_size,
        cycles,
        truncated,
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

/// Served by the same guarded, visited-set walk as `change_impact`'s
/// many-seed call (`Store::impact_of_many`) — single-seed here. Terminates on
/// cyclic call graphs (the visited set prevents revisiting a symbol id within
/// a path) and is bounded by `IMPACT_WORK_LIMIT` rows — part of the tool's
/// documented contract, surfaced through the `truncated`/`row_limit` fields.
pub fn impact(
    store: &Store,
    name: &str,
    max_depth: u32,
    language: Option<&str>,
) -> rusqlite::Result<ImpactResponse> {
    let depth = max_depth.clamp(1, 10);
    let rows = store.impact_of_many(&[name.to_string()], depth, IMPACT_WORK_LIMIT, language)?;
    let truncated = rows.len() >= IMPACT_WORK_LIMIT;
    let impact: Vec<ImpactEntry> = rows
        .into_iter()
        .map(|row| ImpactEntry {
            symbol: SymbolHit::from(row.symbol),
            depth: row.depth,
        })
        .collect();
    Ok(ImpactResponse {
        target: name.to_string(),
        max_depth: depth,
        count: impact.len() as u32,
        name_collision: store.definition_count(name)?,
        truncated,
        row_limit: IMPACT_WORK_LIMIT as u32,
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
    let stale_files = store
        .meta_value("index_root")?
        .map(|root| stale_count(std::path::Path::new(&root), db_path))
        .unwrap_or(1);
    Ok(StatusResponse {
        db_path: db_path.to_string_lossy().to_string(),
        symbol_count: store.symbol_count()?,
        file_count: store.file_count()?,
        stale_files,
        extractor_contract_current: store.extractor_contract_current()?,
    })
}

/// Best-effort count of source files that differ from their stored index mtime
/// (capped). Returns 1 when freshness cannot be read so `status` does not claim
/// that a damaged index is current.
/// Both paths may be relative, so canonicalize before comparing filesystem
/// state. The repository root comes from the index binding rather than the DB
/// location because `--index` may place SQLite anywhere.
fn stale_count(index_root: &std::path::Path, db_path: &std::path::Path) -> usize {
    let Ok(db_abs) = db_path.canonicalize() else {
        return 1;
    };
    let Ok(root) = index_root.canonicalize() else {
        return 1;
    };
    crate::workflow_status::stale_paths(&root, &db_abs, 100)
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

pub(crate) fn component_for_file(scope: &str, kind: &str, file: &str, depth: u8) -> String {
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

pub(crate) fn map_cycle_components(edges: &[(String, String)]) -> Vec<Vec<String>> {
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

/// Empty head-side projection for consumers that still need to explain a
/// fully deleted scope. The ordinary `map` command keeps its explicit
/// no-files error; Lens uses this only when Git impact proves the deletion.
pub(crate) fn empty_project_map(
    path: &str,
    depth: u8,
    production_only: bool,
) -> Result<ProjectMapResponse, String> {
    let normalized = normalize_map_path(path)?;
    Ok(ProjectMapResponse {
        schema_version: 1,
        scope: MapScope {
            path: if normalized.is_empty() {
                ".".into()
            } else {
                normalized
            },
            kind: if path.is_empty() || path == "." {
                "root".into()
            } else {
                "directory".into()
            },
            depth: depth.clamp(1, 6),
            aggregation_paths_truncated: false,
            production_only,
        },
        files: MapCount {
            total: Some(0),
            returned: 0,
            truncated: false,
            truncation_reason: None,
        },
        languages: empty_map_section(),
        components: empty_map_section(),
        entry_points: empty_map_section(),
        hotspots: empty_map_section(),
        cycles: empty_map_section(),
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
        precision_notes: Vec::new(),
    })
}

fn empty_map_section<T>() -> MapSection<T> {
    MapSection {
        total: Some(0),
        returned: 0,
        truncated: false,
        truncation_reason: None,
        items: Vec::new(),
    }
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

    let (import_edges, cycle_work_truncated) = store
        .map_import_edges_capped_filtered(&normalized, kind, MAP_CYCLE_EDGE_LIMIT, production_only)
        .map_err(|error| error.to_string())?;
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
            truncation_reason: if paths_truncated {
                Some("path_work_limit")
            } else if component_total > components.len() {
                Some("top_limit")
            } else {
                None
            },
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
    fn impact_terminates_on_cycles_and_reports_truncation() {
        let path = tmp_db("impact_cycle_termination");
        let store = Store::open(&path).unwrap();
        let a = store
            .insert_symbol("a", "function", "x.py", 1, 2, None, None)
            .unwrap();
        let b = store
            .insert_symbol("b", "function", "x.py", 3, 4, None, None)
            .unwrap();
        store.insert_edge(a, Some(b), "b", "calls", 1).unwrap();
        store.insert_edge(b, Some(a), "a", "calls", 3).unwrap();

        let response = impact(&store, "a", 10, None).unwrap();
        assert!(!response.truncated);
        assert_eq!(response.row_limit, IMPACT_WORK_LIMIT as u32);
        let names: Vec<&str> = response
            .impact
            .iter()
            .map(|entry| entry.symbol.name.as_str())
            .collect();
        assert!(names.contains(&"b"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn impact_max_depth_8_matches_advertised_contract() {
        let path = tmp_db("impact_depth_8");
        let store = Store::open(&path).unwrap();
        // Chain: h1 calls seed, h2 calls h1, ..., h9 calls h8 — depth k
        // reaches exactly h1..=hk.
        let mut prev = "seed".to_string();
        for index in 1..=9u32 {
            let name = format!("h{index}");
            let id = store
                .insert_symbol(&name, "function", "x.py", index, index + 1, None, None)
                .unwrap();
            store.insert_edge(id, None, &prev, "calls", 1).unwrap();
            prev = name;
        }

        let response = impact(&store, "seed", 8, None).unwrap();
        let names: Vec<&str> = response
            .impact
            .iter()
            .map(|entry| entry.symbol.name.as_str())
            .collect();
        assert!(
            names.contains(&"h8"),
            "max_depth=8 must reach h8: {names:?}"
        );
        assert!(
            !names.contains(&"h9"),
            "h9 is 9 hops away, beyond max_depth=8: {names:?}"
        );
        std::fs::remove_file(&path).ok();
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
            "src/core/test_helpers/service.py",
            "src/core/service_test.py",
            "src/core/widget.test.ts",
            "src/core/DatabaseTest.cpp",
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
        let helper = store
            .insert_symbol(
                "helper_target",
                "function",
                "src/core/test_helpers/service.py",
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
        store
            .insert_edge(caller, Some(helper), "helper_target", "calls", 4)
            .unwrap();

        let value =
            serde_json::to_value(project_map_with_options(&store, ".", 2, 20, true).unwrap())
                .unwrap();
        assert_eq!(value["scope"]["production_only"], true);
        assert_eq!(value["files"]["total"], 3);
        let rendered = serde_json::to_string(&value).unwrap();
        assert!(!rendered.contains("tests/fixture"));
        assert!(!rendered.contains("examples/demo"));
        assert!(!rendered.contains("evals/runner"));
        assert!(!rendered.contains("service_test.py"));
        assert!(!rendered.contains("widget.test.ts"));
        assert!(!rendered.contains("DatabaseTest.cpp"));
        assert!(rendered.contains("production_target"));
        assert!(rendered.contains("test_helpers/service.py"));
        assert!(rendered.contains("helper_target"));
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
    fn project_map_reports_component_top_limit() {
        let path = tmp_db("project_map_component_top_limit");
        let store = Store::open(&path).unwrap();
        for file in ["alpha/lib.rs", "beta/lib.rs", "gamma/lib.rs"] {
            store.upsert_file(file, 1, 1).unwrap();
        }

        let value = serde_json::to_value(project_map(&store, ".", 1, 1).unwrap()).unwrap();
        assert_eq!(value["components"]["returned"], 1);
        assert_eq!(value["components"]["truncated"], true);
        assert_eq!(value["components"]["truncation_reason"], "top_limit");
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
        let mut symbols = vec![
            mk_sym("User", "class", "User.B.cs", 3, Some(",partial,")),
            mk_sym("User", "class", "User.A.cs", 3, Some(",partial,")),
            mk_sym("User", "class", "User.C.cs", 3, Some(",partial,")),
            mk_sym("Service", "class", "Service.cs", 2, None),
        ];
        for (index, symbol) in symbols.iter_mut().enumerate() {
            symbol.id = index as i64 + 1;
        }
        let namespaces = HashMap::from([
            (1, "App.Users".to_string()),
            (2, "App.Users".to_string()),
            (3, "App.Users".to_string()),
        ]);
        let hits = collapse_partial_hits(symbols, &namespaces);
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
        let hits = collapse_partial_hits(symbols, &HashMap::new());
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| h.locations.is_none()));
    }

    #[test]
    fn collapse_partials_does_not_merge_rows_without_shared_namespace_identity() {
        let symbols = vec![
            mk_sym(
                "Settings",
                "class",
                "AppA/Settings.cs",
                1,
                Some(",partial,"),
            ),
            mk_sym(
                "Settings",
                "class",
                "AppB/Settings.cs",
                1,
                Some(",partial,"),
            ),
        ];
        let hits = collapse_partial_hits(symbols, &HashMap::new());
        assert_eq!(hits.len(), 2);
        assert!(hits
            .iter()
            .all(|hit| hit.locations.as_ref().is_some_and(|rows| rows.len() == 1)));
    }

    #[test]
    fn search_collapses_partial_types_only_within_the_same_namespace() {
        let path = tmp_db("partial_namespace_identity");
        let mut store = Store::open(&path).unwrap();
        for (file, namespace) in [
            ("AppA/One.cs", "AppA"),
            ("AppA/Two.cs", "AppA"),
            ("AppB/One.cs", "AppB"),
        ] {
            store
                .commit_file(crate::store::PendingFile {
                    path: file.to_string(),
                    mtime: 1,
                    content_sha256: format!("hash-{file}"),
                    language: "csharp".to_string(),
                    symbols: vec![
                        crate::store::PendingSymbol {
                            name: "<module>".to_string(),
                            kind: "module".to_string(),
                            line_start: 1,
                            line_end: 3,
                            signature: None,
                            parent_index: None,
                            decorators: None,
                        },
                        crate::store::PendingSymbol {
                            name: namespace.to_string(),
                            kind: "namespace".to_string(),
                            line_start: 1,
                            line_end: 3,
                            signature: Some(format!("namespace {namespace}")),
                            parent_index: Some(0),
                            decorators: None,
                        },
                        crate::store::PendingSymbol {
                            name: "Settings".to_string(),
                            kind: "class".to_string(),
                            line_start: 2,
                            line_end: 3,
                            signature: Some("partial class Settings".to_string()),
                            parent_index: Some(1),
                            decorators: Some(",partial,".to_string()),
                        },
                    ],
                    edges: Vec::new(),
                })
                .unwrap();
        }

        let response = search(&store, "Settings", Some("class"), Some("csharp"), true).unwrap();
        assert_eq!(response.results.len(), 2);
        let location_counts: Vec<usize> = response
            .results
            .iter()
            .map(|hit| hit.locations.as_ref().unwrap().len())
            .collect();
        assert_eq!(location_counts, vec![2, 1]);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn search_uses_the_full_nested_namespace_identity_for_partials() {
        let path = tmp_db("partial_nested_namespace_identity");
        let mut store = Store::open(&path).unwrap();
        for (file, outer) in [
            ("AppA/One.cs", "AppA"),
            ("AppA/Two.cs", "AppA"),
            ("AppB/One.cs", "AppB"),
        ] {
            store
                .commit_file(crate::store::PendingFile {
                    path: file.to_string(),
                    mtime: 1,
                    content_sha256: format!("hash-{file}"),
                    language: "csharp".to_string(),
                    symbols: vec![
                        crate::store::PendingSymbol {
                            name: "<module>".to_string(),
                            kind: "module".to_string(),
                            line_start: 1,
                            line_end: 5,
                            signature: None,
                            parent_index: None,
                            decorators: None,
                        },
                        crate::store::PendingSymbol {
                            name: outer.to_string(),
                            kind: "namespace".to_string(),
                            line_start: 1,
                            line_end: 5,
                            signature: Some(format!("namespace {outer}")),
                            parent_index: Some(0),
                            decorators: None,
                        },
                        crate::store::PendingSymbol {
                            name: "Common".to_string(),
                            kind: "namespace".to_string(),
                            line_start: 2,
                            line_end: 5,
                            signature: Some("namespace Common".to_string()),
                            parent_index: Some(1),
                            decorators: None,
                        },
                        crate::store::PendingSymbol {
                            name: "Settings".to_string(),
                            kind: "class".to_string(),
                            line_start: 3,
                            line_end: 4,
                            signature: Some("partial class Settings".to_string()),
                            parent_index: Some(2),
                            decorators: Some(",partial,".to_string()),
                        },
                    ],
                    edges: Vec::new(),
                })
                .unwrap();
        }

        let response = search(&store, "Settings", Some("class"), Some("csharp"), true).unwrap();
        assert_eq!(response.results.len(), 2);
        let location_counts = response
            .results
            .iter()
            .map(|hit| hit.locations.as_ref().unwrap().len())
            .collect::<Vec<_>>();
        assert_eq!(location_counts, vec![2, 1]);
        std::fs::remove_file(path).ok();
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

    fn brief_repo(name: &str) -> (PathBuf, Store) {
        let root = impact_repo(
            name,
            &[
                (
                    "src/app.py",
                    "def target():\n    return 1\n\ndef caller():\n    return target()\n",
                ),
                (
                    "src/test_app.py",
                    "from src.app import target\n\ndef test_target():\n    assert target() == 1\n",
                ),
                ("src/é\"quoted.py", "def unicode_target():\n    return 1\n"),
                (
                    "CONTEXT.md",
                    "# Target decision\nDO_NOT_OUTPUT_HISTORY_BODY target caller details.\n",
                ),
            ],
        );
        write_impact_file(
            &root,
            "src/app.py",
            "def target():\n    return 2\n\ndef caller():\n    return target()\n",
        );
        write_impact_file(
            &root,
            "src/é\"quoted.py",
            "def unicode_target():\n    return 2\n",
        );
        let store = index_impact(&root, name);
        (root, store)
    }

    #[test]
    fn brief_schema_v1_is_deterministic_bounded_and_body_free() {
        let (root, store) = brief_repo("brief_schema");
        let first = brief(
            &store,
            &root,
            "HEAD",
            BriefRole::Executor,
            8_000,
            &crate::mcp::brief_current_envelope_bytes,
        )
        .unwrap();
        let second = brief(
            &store,
            &root,
            "HEAD",
            BriefRole::Executor,
            8_000,
            &crate::mcp::brief_current_envelope_bytes,
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.schema_version, 1);
        assert!(first.repository_content_untrusted);
        assert_eq!(first.freshness.structural.status, "fresh");
        assert!(matches!(
            first.freshness.history.status.as_str(),
            "fresh" | "incomplete"
        ));
        assert!(first
            .changes
            .symbols
            .items
            .iter()
            .any(|symbol| symbol.name == "target"));
        assert!(first
            .callers
            .items
            .iter()
            .any(|caller| caller.name == "caller"));
        assert!(first
            .history
            .query_terms
            .iter()
            .any(|term| term == "target"));
        assert!(first.citations.items.iter().any(|citation| {
            citation
                .matched_terms
                .iter()
                .any(|term| term.eq_ignore_ascii_case("target"))
        }));
        let bytes = crate::mcp::brief_current_envelope_bytes(&first).unwrap();
        assert_eq!(first.budget.final_envelope_bytes, bytes as u32);
        assert_eq!(first.budget.estimated_tokens, bytes.div_ceil(4) as u32);
        assert!(first.budget.estimated_tokens <= first.budget.requested_tokens);
        let encoded = serde_json::to_string(&first).unwrap();
        assert!(encoded.contains("é"));
        assert!(encoded.contains("quoted.py"));
        assert!(!encoded.contains("DO_NOT_OUTPUT_HISTORY_BODY"));
        assert!(!encoded.contains("excerpt"));
        assert!(!encoded.contains("signature"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn brief_role_changes_prefix_admission_without_changing_schema() {
        let (root, store) = brief_repo("brief-role-admission");
        let mut full = brief(
            &store,
            &root,
            "HEAD",
            BriefRole::Executor,
            8_000,
            &crate::mcp::brief_current_envelope_bytes,
        )
        .unwrap();
        for index in 0..4 {
            full.tests.items.push(BriefTest {
                file: format!("src/test_{index}.py"),
                name: format!("test_{index}"),
                kind: "function".into(),
                line: index + 1,
                classification: "direct".into(),
                minimum_depth: Some(1),
                confidence: "high".into(),
            });
            full.citations.items.push(BriefHistoryCitation {
                path: format!(".mastermind/tasks/{index}/spec.md"),
                kind: "task_spec".into(),
                matched_terms: vec!["target".into()],
                rank: index + 1,
            });
        }
        full.tests.total = Some(brief_u32(full.tests.items.len()));
        full.citations.total = Some(brief_u32(full.citations.items.len()));
        sync_brief_counts(&mut full);
        full.budget.requested_tokens = 8_000;
        full.budget.estimated_tokens = 0;
        full.budget.final_envelope_bytes = 0;
        full.budget.minimum_tokens = 0;
        full = apply_brief_budget(full, &crate::mcp::brief_current_envelope_bytes).unwrap();
        let mut observed_priority_difference = false;
        for budget in full.budget.minimum_tokens..full.budget.estimated_tokens {
            let build = |role| {
                let mut packet = full.clone();
                packet.role = role;
                packet.budget.requested_tokens = budget;
                packet.budget.estimated_tokens = 0;
                packet.budget.final_envelope_bytes = 0;
                packet.budget.minimum_tokens = 0;
                packet.omitted.changed_files.budget = 0;
                packet.omitted.changed_symbols.budget = 0;
                packet.omitted.callers.budget = 0;
                packet.omitted.tests.budget = 0;
                packet.omitted.history_citations.budget = 0;
                apply_brief_budget(packet, &crate::mcp::brief_current_envelope_bytes).ok()
            };
            let (Some(planner), Some(executor)) =
                (build(BriefRole::Planner), build(BriefRole::Executor))
            else {
                continue;
            };
            if planner.tests.returned < executor.tests.returned
                && planner.citations.returned > executor.citations.returned
            {
                observed_priority_difference = true;
                assert_eq!(
                    serde_json::to_value(&planner)
                        .unwrap()
                        .as_object()
                        .unwrap()
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>(),
                    serde_json::to_value(&executor)
                        .unwrap()
                        .as_object()
                        .unwrap()
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                );
                break;
            }
        }
        assert!(observed_priority_difference);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn brief_256_returns_a_precise_minimum_instead_of_overrunning() {
        let (root, store) = brief_repo("brief_minimum");
        let error = brief(
            &store,
            &root,
            "HEAD",
            BriefRole::Planner,
            256,
            &crate::mcp::brief_current_envelope_bytes,
        )
        .unwrap_err();
        match error {
            BriefError::BudgetTooSmall { minimum_tokens } => {
                assert!(minimum_tokens > 256);
                assert!(minimum_tokens <= BRIEF_MAX_BUDGET_TOKENS);
            }
            other => panic!("unexpected brief error: {other:?}"),
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn brief_no_change_skips_fts_with_an_explicit_reason() {
        let root = impact_repo(
            "brief_no_change",
            &[("src/app.py", "def stable():\n    return 1\n")],
        );
        let store = index_impact(&root, "brief_no_change");
        let packet = brief(
            &store,
            &root,
            "HEAD",
            BriefRole::Auditor,
            8_000,
            &crate::mcp::brief_current_envelope_bytes,
        )
        .unwrap();
        assert!(!packet.history.query_performed);
        assert_eq!(
            packet.history.empty_reason.as_deref(),
            Some("no_eligible_changed_terms")
        );
        assert!(packet.history.query_terms.is_empty());
        assert!(packet.citations.items.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn brief_rejects_structural_and_history_snapshot_races() {
        for history_race in [false, true] {
            let suffix = if history_race { "history" } else { "worktree" };
            let (root, store) = brief_repo(&format!("brief-race-{suffix}"));
            let changed_root = root.clone();
            let _hook = install_brief_test_hook(move || {
                if history_race {
                    write_impact_file(
                        &changed_root,
                        "CONTEXT.md",
                        "# Replaced while briefing\nnew target record\n",
                    );
                } else {
                    write_impact_file(
                        &changed_root,
                        "src/app.py",
                        "def target():\n    return 3\n\ndef caller():\n    return target()\n",
                    );
                }
            });
            assert_eq!(
                brief(
                    &store,
                    &root,
                    "HEAD",
                    BriefRole::Auditor,
                    8_000,
                    &crate::mcp::brief_current_envelope_bytes,
                )
                .unwrap_err(),
                BriefError::SnapshotChanged
            );
            std::fs::remove_dir_all(root).ok();
        }
    }

    #[test]
    fn brief_history_terms_are_quoted_bounded_and_stable() {
        let changes = ImpactChanges {
            files: Collection {
                total: Some(2),
                returned: 2,
                truncated: false,
                truncation_reason: None,
                items: vec![
                    ChangedFile {
                        path: "src/HTTP-client.ts".into(),
                        status: "modified".into(),
                    },
                    ChangedFile {
                        path: "tests/ignored.py".into(),
                        status: "modified".into(),
                    },
                ],
            },
            symbols: Collection {
                total: Some(2),
                returned: 2,
                truncated: false,
                truncation_reason: None,
                items: vec![
                    ChangedSymbol {
                        file: "z.py".into(),
                        name: "Beta_handler".into(),
                        kind: "function".into(),
                        line: 9,
                        change: "body_changed".into(),
                    },
                    ChangedSymbol {
                        file: "a.py".into(),
                        name: "Alpha-handler".into(),
                        kind: "function".into(),
                        line: 1,
                        change: "body_changed".into(),
                    },
                ],
            },
        };
        let terms = brief_history_terms(&changes);
        assert_eq!(
            terms,
            vec![
                "alpha",
                "handler",
                "beta_handler",
                "http",
                "client",
                "ignored"
            ]
        );
        assert_eq!(
            brief_history_query(&terms),
            r#""alpha" OR "handler" OR "beta_handler" OR "http" OR "client" OR "ignored""#
        );
        assert!(terms.len() <= BRIEF_HISTORY_TERM_LIMIT);
    }

    #[test]
    fn brief_sanitizer_escapes_controls_and_rejects_control_only_values() {
        assert_eq!(
            safe_brief_string("src/ok\n\u{202e}.rs").as_deref(),
            Some("src/ok\\u{000A}\\u{202E}.rs")
        );
        assert_eq!(safe_brief_string("\n\u{202e}"), None);
        let long = safe_brief_string(&"é".repeat(600)).unwrap();
        assert!(long.len() <= BRIEF_REPOSITORY_STRING_BYTES);
        assert!(long.ends_with("..."));
        let escape_boundary = safe_brief_string(&("a".repeat(504) + "\u{202e}" + "z")).unwrap();
        assert_eq!(escape_boundary, "a".repeat(504) + "...");
        assert!(!escape_boundary.contains("\\u{"));
        let (collection, omitted) = safe_brief_collection(
            [None, Some("safe")],
            BriefSourceLimit {
                total: Some(9),
                omitted: 7,
                exact: true,
            },
        );
        assert_eq!(collection.total, Some(9));
        assert_eq!(collection.returned, 1);
        assert_eq!(omitted.source_limit, 7);
        assert!(omitted.source_limit_exact);
        assert_eq!(omitted.unsafe_content, 1);
        assert_eq!(omitted.budget, 0);
    }

    #[test]
    fn brief_unknown_source_truncation_stays_a_lower_bound() {
        let source = Collection {
            total: None,
            returned: 4,
            truncated: true,
            truncation_reason: Some("work_limit".into()),
            items: vec!["one", "two", "three", "four"],
        };
        let source_limit = brief_source_omitted(&source, 3);
        let (collection, omitted) =
            safe_brief_collection(source.items.into_iter().take(3).map(Some), source_limit);
        assert_eq!(collection.total, None);
        assert_eq!(collection.returned, 3);
        assert_eq!(omitted.source_limit, 2);
        assert!(!omitted.source_limit_exact);
    }

    #[test]
    fn brief_history_totals_are_unknown_at_the_query_cap_or_with_missing_sources() {
        for source_limit in [
            brief_history_source_limit(BRIEF_HISTORY_LIMIT as u32, 0, false),
            brief_history_source_limit(3, 1, false),
            brief_history_source_limit(3, 0, true),
        ] {
            assert_eq!(source_limit.total, None);
            assert_eq!(source_limit.omitted, 0);
            assert!(!source_limit.exact);
        }
        let exact = brief_history_source_limit(3, 0, false);
        assert_eq!(exact.total, Some(3));
        assert!(exact.exact);
    }

    #[test]
    fn brief_role_priority_is_fixed() {
        let names = |role| {
            brief_priority(role)
                .into_iter()
                .map(|section| match section {
                    BriefSection::Changes => "changes",
                    BriefSection::Callers => "callers",
                    BriefSection::Tests => "tests",
                    BriefSection::History => "history",
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(BriefRole::Planner),
            ["changes", "callers", "history", "tests"]
        );
        assert_eq!(
            names(BriefRole::Executor),
            ["changes", "tests", "callers", "history"]
        );
        assert_eq!(
            names(BriefRole::Auditor),
            ["tests", "callers", "changes", "history"]
        );
    }

    #[test]
    fn status_uses_bound_index_root_for_an_external_database() {
        let root = impact_repo(
            "status_external_db",
            &[("src/app.py", "def current():\n    return 1\n")],
        );
        let store = index_impact(&root, "status_external_db");
        assert!(!store.db_path().starts_with(&root));

        let response = status(&store).unwrap();
        assert_eq!(response.stale_files, 0);
        assert!(response.extractor_contract_current);
        std::fs::remove_dir_all(root).ok();
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
    fn unguarded_change_impact_recovers_its_local_graph_budget() {
        let root = impact_repo(
            "unguarded_local_graph_limit",
            &[("src/app.py", "def value():\n    return 1\n")],
        );
        write_impact_file(&root, "src/app.py", "def value():\n    return 2\n");
        let store = index_impact(&root, "unguarded_local_graph_limit");
        for index in 0..256 {
            let caller = store
                .insert_symbol(
                    &format!("dense_caller_{index}"),
                    "function",
                    "src/generated.py",
                    index + 1,
                    index + 1,
                    None,
                    None,
                )
                .unwrap();
            store
                .insert_edge(caller, None, "value", "calls", index + 1)
                .unwrap();
        }
        let _local_budget =
            crate::store::override_impact_precision_budget(crate::store::WorkBudget {
                deadline: None,
                op_ticks: Some(1),
            });

        let response = change_impact(&store, &root, "HEAD", 3, 100).unwrap();
        assert!(response
            .precision_notes
            .iter()
            .any(|note| note == "graph_work_limit"));
        assert!(response.impact.truncated);
        assert_eq!(
            response.impact.truncation_reason.as_deref(),
            Some("work_limit")
        );
        assert_eq!(store.interrupt_source(), None);
        std::fs::remove_dir_all(root).ok();
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

    #[test]
    fn every_indexable_extension_reports_a_known_language_and_precision() {
        let extensions = [
            "py", "pyi", "PY", "ts", "tsx", "js", "jsx", "mjs", "cjs", "vue", "rs", "cs", "go",
            "java", "php", "phtml", "c", "cc", "cpp", "CPP", "cxx", "h", "hpp", "hh", "hxx", "ipp",
            "tpp",
        ];
        for ext in extensions {
            let path = format!("src/file.{ext}");
            assert!(
                crate::indexer::extractor_for_path(std::path::Path::new(&path)).is_some(),
                "{ext}: no extractor, drop it from this list"
            );
            assert_ne!(
                lang_from_ext(ext),
                "unknown",
                "{ext} is indexed but lang_from_ext reports unknown"
            );
            let precision = lang_precision(&path);
            assert_ne!(
                precision.confidence, "unknown",
                "{ext} is indexed but lang_precision reports unknown"
            );
            assert!(
                !precision
                    .limitations
                    .contains(&"unsupported or unrecognized language"),
                "{ext} is indexed but carries the unsupported-language note"
            );
        }
    }

    #[test]
    fn disciplines_classify_only_what_a_path_can_establish() {
        let files: Vec<ChangedFile> = [
            "src/ui/Button.tsx",
            "src/ui/Card.vue",
            "src/ui/theme.scss",
            "src/ui/Button.test.tsx",
            "tests/checkout_spec.py",
            "src/server/queue_consumer.ts",
            "migrations/0007_add_index.sql",
        ]
        .into_iter()
        .map(|path| ChangedFile {
            path: path.to_string(),
            status: "M".to_string(),
        })
        .collect();

        let result = classify_disciplines(&files);
        let by_name = |name: &str| {
            result
                .detected
                .iter()
                .find(|signal| signal.name == name)
                .unwrap_or_else(|| panic!("{name} not detected"))
        };

        let frontend = by_name("frontend");
        assert_eq!(frontend.file_count, 4);
        assert!(frontend.files.contains(&"src/ui/Card.vue".to_string()));
        assert!(frontend.files.contains(&"src/ui/theme.scss".to_string()));

        // A test-named component file belongs to both sets, not to one.
        let qa = by_name("qa");
        assert!(qa.files.contains(&"src/ui/Button.test.tsx".to_string()));
        assert!(frontend
            .files
            .contains(&"src/ui/Button.test.tsx".to_string()));

        // A SQL file and a migrations directory are both unambiguous.
        let migration = by_name("migration");
        assert_eq!(migration.file_count, 1);
        assert!(migration
            .files
            .contains(&"migrations/0007_add_index.sql".to_string()));

        // A queue consumer is backend work that no path proves.
        assert_eq!(
            result.unclassified,
            vec!["src/server/queue_consumer.ts".to_string()]
        );
    }

    #[test]
    fn migration_covers_the_common_layouts_without_guessing() {
        let files: Vec<ChangedFile> = [
            "db/migrate/20240101_add_orders.rb",
            "prisma/migrations/0001_init/migration.sql",
            "app/db/migration/V3__add_index.sql",
            "src/orders/repository.ts",
        ]
        .into_iter()
        .map(|path| ChangedFile {
            path: path.to_string(),
            status: "M".to_string(),
        })
        .collect();

        let result = classify_disciplines(&files);
        let migration = result
            .detected
            .iter()
            .find(|signal| signal.name == "migration")
            .expect("migration not detected");
        assert_eq!(migration.file_count, 3);

        // A repository that talks to the database is not a migration.
        assert_eq!(
            result.unclassified,
            vec!["src/orders/repository.ts".to_string()]
        );
    }

    #[test]
    fn disciplines_are_absent_rather_than_empty_when_nothing_matches() {
        let files = vec![ChangedFile {
            path: "README.md".to_string(),
            status: "M".to_string(),
        }];
        let result = classify_disciplines(&files);
        assert!(result.detected.is_empty());
        assert_eq!(result.unclassified, vec!["README.md".to_string()]);
    }
}
