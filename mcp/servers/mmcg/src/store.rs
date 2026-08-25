//! SQLite storage for the code graph.
//!
//! Schema:
//!   symbols(id, name, kind, file_path, line_start, line_end, signature, parent_id)
//!   edges(id, from_id, to_id?, to_name, kind, line)
//!   files(path, indexed_at, symbol_count)
//!   semantic_* (optional compiler-resolved overlay; never rewrites the graph)
//!   fact_* (validated declarative evidence; never rewrites the graph)
//!   meta(key, value)

use crate::facts::{
    source_public_id, FactAnnotation, FactArtifact, FactFileRecord, FactImportBatch,
    FactQueryFilter, FactRelationship, FactSourceRecord,
};
use crate::scip_overlay::{SemanticDefinition, SemanticEdge, SemanticImportBatch, SemanticSource};
use rusqlite::{
    params, types::Value as SqlValue, Connection, OpenFlags, OptionalExtension, Result as SqlResult,
};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

const SCHEMA_VERSION: &str = "7";
pub const CONCEPT_NORMALIZATION_META_KEY: &str = "concept_normalization_version";
pub const CONCEPT_NORMALIZATION_VERSION: &str = "mmcg-concepts-v2";
pub const CONCEPT_DOCUMENTATION_SUPPORTED_LANGUAGES: &str = "javascript,python,rust,tsx,typescript";
pub const CONCEPT_DOCUMENTATION_INDEXED_META_KEY: &str = "concept_documentation_indexed_count";
pub const CONCEPT_DOCUMENTATION_SECRET_OMITTED_META_KEY: &str =
    "concept_documentation_secret_omitted_count";
pub const CONCEPT_DOCUMENTATION_SIZE_OMITTED_META_KEY: &str =
    "concept_documentation_size_omitted_count";
pub const CONCEPT_DOCUMENTATION_UNSUPPORTED_META_KEY: &str =
    "concept_documentation_unsupported_language_count";
pub const CONCEPT_DOCUMENTATION_LANGUAGES_META_KEY: &str =
    "concept_documentation_supported_languages";
pub const CONCEPT_TERM_MAX_BYTES: usize = 64;
pub const CONCEPT_SIGNATURE_MAX_BYTES: usize = 256;
pub const CONCEPT_DOCUMENTATION_MAX_BYTES: usize = 512;
const CONCEPT_FIELD_MAX_BYTES: usize = CONCEPT_DOCUMENTATION_MAX_BYTES;
const CONCEPT_INDEX_TERM_LIMIT: usize = 128;
const CONCEPT_SIGNATURE_SCAN_MAX_BYTES: usize = 64 * 1024;
const CONCEPT_CONTRACT_DIRTY: &str = "dirty";
static INSERT_SYMBOL_SAVEPOINT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const CONCEPT_SCHEMA_DROP_SQL: &str = r#"
    DROP TRIGGER IF EXISTS symbol_concepts_graph_ai;
    DROP TRIGGER IF EXISTS symbol_concepts_graph_ad;
    DROP TRIGGER IF EXISTS symbol_concepts_graph_au;
    DROP TRIGGER IF EXISTS symbol_concepts_ai;
    DROP TRIGGER IF EXISTS symbol_concepts_ad;
    DROP TRIGGER IF EXISTS symbol_concepts_au;
    DROP TABLE IF EXISTS symbol_concepts_fts;
    DROP TABLE IF EXISTS concept_documentation_file_stats;
    DROP TABLE IF EXISTS symbol_concepts;
"#;
const CONCEPT_SCHEMA_DDL: &str = r#"
    CREATE TABLE symbol_concepts (
        symbol_id             INTEGER PRIMARY KEY
                              REFERENCES symbols(id) ON DELETE CASCADE,
        name_search           TEXT NOT NULL,
        path_search           TEXT NOT NULL,
        path_sort             TEXT NOT NULL,
        signature_search      TEXT NOT NULL,
        documentation_search  TEXT NOT NULL DEFAULT ''
    );
    CREATE TABLE concept_documentation_file_stats (
        path                TEXT PRIMARY KEY
                            REFERENCES files(path) ON DELETE CASCADE,
        language_supported  INTEGER NOT NULL
                            CHECK(language_supported IN (0, 1)),
        indexed_documents   INTEGER NOT NULL CHECK(indexed_documents >= 0),
        secret_omitted      INTEGER NOT NULL CHECK(secret_omitted >= 0),
        size_omitted        INTEGER NOT NULL CHECK(size_omitted >= 0)
    );
    CREATE VIRTUAL TABLE symbol_concepts_fts USING fts5(
        name_search,
        path_search,
        signature_search,
        documentation_search,
        content='symbol_concepts',
        content_rowid='symbol_id',
        tokenize = "unicode61 remove_diacritics 0 tokenchars '_'"
    );
    CREATE TRIGGER symbol_concepts_ai
    AFTER INSERT ON symbol_concepts BEGIN
        INSERT INTO symbol_concepts_fts(
            rowid, name_search, path_search, signature_search,
            documentation_search
        ) VALUES (
            new.symbol_id, new.name_search, new.path_search,
            new.signature_search, new.documentation_search
        );
    END;
    CREATE TRIGGER symbol_concepts_ad
    AFTER DELETE ON symbol_concepts BEGIN
        INSERT INTO symbol_concepts_fts(
            symbol_concepts_fts, rowid, name_search, path_search,
            signature_search, documentation_search
        ) VALUES (
            'delete', old.symbol_id, old.name_search, old.path_search,
            old.signature_search, old.documentation_search
        );
    END;
    CREATE TRIGGER symbol_concepts_au
    AFTER UPDATE ON symbol_concepts BEGIN
        INSERT INTO symbol_concepts_fts(
            symbol_concepts_fts, rowid, name_search, path_search,
            signature_search, documentation_search
        ) VALUES (
            'delete', old.symbol_id, old.name_search, old.path_search,
            old.signature_search, old.documentation_search
        );
        INSERT INTO symbol_concepts_fts(
            rowid, name_search, path_search, signature_search,
            documentation_search
        ) VALUES (
            new.symbol_id, new.name_search, new.path_search,
            new.signature_search, new.documentation_search
        );
    END;
    CREATE TRIGGER symbol_concepts_graph_ai
    AFTER INSERT ON symbols BEGIN
        INSERT INTO meta(key, value)
        VALUES ('concept_normalization_version', 'dirty')
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        WHERE meta.value <> excluded.value;
    END;
    CREATE TRIGGER symbol_concepts_graph_ad
    AFTER DELETE ON symbols BEGIN
        INSERT INTO meta(key, value)
        VALUES ('concept_normalization_version', 'dirty')
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        WHERE meta.value <> excluded.value;
    END;
    CREATE TRIGGER symbol_concepts_graph_au
    AFTER UPDATE ON symbols BEGIN
        INSERT INTO meta(key, value)
        VALUES ('concept_normalization_version', 'dirty')
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        WHERE meta.value <> excluded.value;
    END;
"#;
const CONCEPT_SHADOW_REPAIR_SQL: &str = r#"
    CREATE TABLE IF NOT EXISTS 'symbol_concepts_fts_config'(k PRIMARY KEY, v) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS 'symbol_concepts_fts_data'(id INTEGER PRIMARY KEY, block BLOB);
    CREATE TABLE IF NOT EXISTS 'symbol_concepts_fts_docsize'(id INTEGER PRIMARY KEY, sz BLOB);
    CREATE TABLE IF NOT EXISTS 'symbol_concepts_fts_idx'(
        segid, term, pgno, PRIMARY KEY(segid, term)
    ) WITHOUT ROWID;
    INSERT OR IGNORE INTO symbol_concepts_fts_config(k, v) VALUES ('version', 4);
    INSERT OR IGNORE INTO symbol_concepts_fts_data(id, block) VALUES (1, x'');
    INSERT OR IGNORE INTO symbol_concepts_fts_data(id, block) VALUES (10, zeroblob(7));
"#;
const CONCEPT_SHADOW_NAMES: &[&str] = &[
    "symbol_concepts_fts_config",
    "symbol_concepts_fts_data",
    "symbol_concepts_fts_docsize",
    "symbol_concepts_fts_idx",
];
const CONCEPT_TRIGGER_NAMES: &[&str] = &[
    "symbol_concepts_graph_ai",
    "symbol_concepts_graph_ad",
    "symbol_concepts_graph_au",
    "symbol_concepts_ai",
    "symbol_concepts_ad",
    "symbol_concepts_au",
];
const CONCEPT_SCHEMA_OBJECTS: &[(&str, &str)] = &[
    ("table", "symbol_concepts"),
    ("table", "concept_documentation_file_stats"),
    ("table", "symbol_concepts_fts"),
    ("table", "symbol_concepts_fts_config"),
    ("table", "symbol_concepts_fts_data"),
    ("table", "symbol_concepts_fts_docsize"),
    ("table", "symbol_concepts_fts_idx"),
    ("trigger", "symbol_concepts_ai"),
    ("trigger", "symbol_concepts_ad"),
    ("trigger", "symbol_concepts_au"),
    ("trigger", "symbol_concepts_graph_ai"),
    ("trigger", "symbol_concepts_graph_ad"),
    ("trigger", "symbol_concepts_graph_au"),
];
static CONCEPT_SCHEMA_CONTRACT: OnceLock<Vec<(&'static str, &'static str, String)>> =
    OnceLock::new();
const READ_ONLY_SNAPSHOT_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const READ_ONLY_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(60);

fn row_u64(row: &rusqlite::Row<'_>, index: usize) -> SqlResult<u64> {
    let value = row.get::<_, i64>(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

/// A per-request work-budget: a wall-clock deadline and/or a cap on SQLite
/// progress-handler ticks (each tick fires every 1,000 VM instructions).
/// `None` in either field means that dimension is unbounded. Read via
/// [`query_budget_ms_from_env`] for the env-var-driven defaults; construct
/// directly for tests that need an exact, deterministic budget.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkBudget {
    pub deadline: Option<Duration>,
    pub op_ticks: Option<u64>,
}

impl WorkBudget {
    pub const UNLIMITED: Self = Self {
        deadline: None,
        op_ticks: None,
    };

    /// `0` means unlimited (matches the `MMCG_QUERY_BUDGET_MS` / CLI contract:
    /// "0 = unlimited"). Any other value is a wall-clock deadline only.
    pub fn from_millis(budget_ms: u64) -> Self {
        if budget_ms == 0 {
            Self::UNLIMITED
        } else {
            Self {
                deadline: Some(Duration::from_millis(budget_ms)),
                op_ticks: None,
            }
        }
    }
}

fn impact_precision_budget() -> WorkBudget {
    #[cfg(test)]
    if let Some(budget) = IMPACT_PRECISION_BUDGET_OVERRIDE.with(Cell::get) {
        return budget;
    }
    WorkBudget {
        deadline: Some(Duration::from_secs(2)),
        op_ticks: Some(250_000),
    }
}

#[cfg(test)]
thread_local! {
    static IMPACT_PRECISION_BUDGET_OVERRIDE: Cell<Option<WorkBudget>> = const { Cell::new(None) };
    static FAIL_CONCEPT_SCHEMA_AFTER_SHADOW_REPAIR: Cell<bool> = const { Cell::new(false) };
}

#[cfg(test)]
pub(crate) struct ImpactPrecisionBudgetOverride {
    previous: Option<WorkBudget>,
}

#[cfg(test)]
impl Drop for ImpactPrecisionBudgetOverride {
    fn drop(&mut self) {
        IMPACT_PRECISION_BUDGET_OVERRIDE.with(|slot| slot.set(self.previous));
    }
}

#[cfg(test)]
pub(crate) fn override_impact_precision_budget(
    budget: WorkBudget,
) -> ImpactPrecisionBudgetOverride {
    let previous = IMPACT_PRECISION_BUDGET_OVERRIDE.with(|slot| slot.replace(Some(budget)));
    ImpactPrecisionBudgetOverride { previous }
}

/// Which mechanism raised a `SQLITE_INTERRUPT`-shaped error on the connection:
/// the work-budget guard itself, or an out-of-band client cancel notification
/// (`Connection::get_interrupt_handle().interrupt()`). Recorded on [`Store`]
/// immediately before each mechanism fires so callers can map budget expiry to
/// `work_limit_exceeded` and client cancel to `cancelled` without conflating
/// the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptSource {
    Budget,
    Cancel,
}

const INTERRUPT_NONE: u8 = 0;
const INTERRUPT_BUDGET: u8 = 1;
const INTERRUPT_CANCEL: u8 = 2;

/// One frame of the work-budget guard stack. Holds the *effective* (already
/// min-composed with the parent, if any) absolute deadline and op-tick cap, so
/// checking a frame never needs to re-walk its ancestors.
#[derive(Debug, Clone, Copy)]
struct GuardFrame {
    deadline: Option<Instant>,
    op_cap: Option<u64>,
    /// Snapshot of the shared ops counter when this frame was pushed — lets us
    /// compute "ticks consumed since this frame started" without a per-frame
    /// counter, so ticks consumed by a nested child frame still count against
    /// this frame's cap once the child pops back.
    ops_baseline: u64,
}

impl GuardFrame {
    fn expired(&self, ops_counter: &AtomicU64) -> bool {
        if let Some(deadline) = self.deadline {
            if Instant::now() >= deadline {
                return true;
            }
        }
        if let Some(cap) = self.op_cap {
            let used = ops_counter
                .load(Ordering::Relaxed)
                .saturating_sub(self.ops_baseline);
            if used >= cap {
                return true;
            }
        }
        false
    }
}

/// Read a millisecond budget from `MMCG_QUERY_BUDGET_MS`, falling back to
/// `default_ms` when unset, empty, or unparsable. `default_ms` differs by
/// context (MCP serve vs one-shot CLI queries) even though the env var is
/// shared.
pub fn query_budget_ms_from_env(default_ms: u64) -> u64 {
    std::env::var("MMCG_QUERY_BUDGET_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default_ms)
}

/// A cloneable, `Send + Sync` handle that lets another thread (the `serve_io`
/// reader thread) abort whatever query is currently running on this `Store`'s
/// connection — the cross-thread mechanism; the in-thread progress handler
/// cannot be fired externally. Records the interrupt source as `Cancel`
/// *before* calling into SQLite, so the source is never ambiguous with a
/// budget expiry.
#[derive(Clone)]
pub struct CancelHandle {
    interrupt_source: Arc<AtomicU8>,
    sqlite: Arc<rusqlite::InterruptHandle>,
}

impl CancelHandle {
    pub fn cancel(&self) {
        self.interrupt_source
            .store(INTERRUPT_CANCEL, Ordering::SeqCst);
        self.sqlite.interrupt();
    }
}

/// Default MCP-serve budget when `MMCG_QUERY_BUDGET_MS` is unset.
pub const DEFAULT_SERVE_BUDGET_MS: u64 = 10_000;
/// Default one-shot CLI-query budget when `MMCG_QUERY_BUDGET_MS` is unset.
pub const DEFAULT_CLI_BUDGET_MS: u64 = 60_000;

/// Work cap for `Store::dependency_cycles`: the largest number of distinct
/// file-pair import edges it will feed to Tarjan. Above this, the result is
/// reported as truncated ("incomplete and possibly inaccurate") without
/// computing SCCs at all.
pub const DEPENDENCY_CYCLE_PAIR_LIMIT: usize = 50_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub file_path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub signature: Option<String>,
    pub parent_id: Option<i64>,
    /// Comma-bookended decorators/attributes/modifiers (e.g. `",Fact,"`,
    /// `",partial,sealed,"`); `None` if none. Used by `mmcg_unreferenced`
    /// filtering and `mmcg_search` partial-class collapse.
    pub decorators: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MapBoundaryRow {
    pub component: String,
    pub symbol: Symbol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapBoundaryMatch {
    Direct,
    Recursive,
}

#[derive(Debug, Clone)]
pub struct MapBoundaryScope {
    pub label: String,
    pub path: String,
    pub match_mode: MapBoundaryMatch,
}

#[derive(Debug, Clone)]
pub struct MapCentralityRow {
    pub symbol: Symbol,
    pub in_degree: u32,
    pub name_collision: u32,
}

#[derive(Debug, Clone)]
pub struct SeedImpact {
    pub seed: String,
    pub symbol: Symbol,
    pub depth: u32,
}

/// Column list for every SELECT that hydrates a [`Symbol`] via [`Store::row_to_symbol`].
/// Adding a column? Update both constants AND `row_to_symbol`.
const SYMBOL_COLS: &str =
    "id, name, kind, file_path, line_start, line_end, signature, parent_id, decorators";
const SYMBOL_COLS_S: &str = "s.id, s.name, s.kind, s.file_path, s.line_start, s.line_end, s.signature, s.parent_id, s.decorators";

/// One file and how many incoming edges resolve into it. See
/// [`Store::file_in_degrees`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileInDegree {
    pub file: String,
    pub in_degree: u32,
}

/// One file and its size (last symbol's end line — a proxy for line count).
/// See [`Store::largest_files`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileSize {
    pub file: String,
    pub lines: u32,
}

pub(crate) fn is_production_path(path: &str) -> bool {
    const SEGMENTS: &[&str] = &[
        "test",
        "tests",
        "__tests__",
        "fixture",
        "fixtures",
        "example",
        "examples",
        "demo",
        "demos",
        "benchmark",
        "benchmarks",
        "bench",
        "benches",
        "eval",
        "evals",
        "generated",
        "vendor",
        "node_modules",
        "target",
    ];
    let wrapped = format!("/{}/", path.to_ascii_lowercase());
    if SEGMENTS
        .iter()
        .any(|segment| wrapped.contains(&format!("/{segment}/")))
    {
        return false;
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    !(lower.starts_with("test_")
        || lower.contains("_test.")
        || lower.contains(".test.")
        || lower.contains(".spec.")
        || name
            .rsplit_once('.')
            .is_some_and(|(stem, _)| stem.ends_with("Test") || stem.ends_with("Tests")))
}

fn unreferenced_candidates_sql() -> String {
    format!(
        "WITH referenced_names AS (
             SELECT DISTINCT to_name AS nm FROM edges
             UNION
             SELECT DISTINCT to_type AS nm FROM edges
               WHERE to_type IS NOT NULL AND to_type <> ''
         ),
         candidates AS (
             SELECT {SYMBOL_COLS_S}
             FROM symbols s
             LEFT JOIN referenced_names r ON r.nm = s.name
             WHERE r.nm IS NULL
               AND (?1 IS NULL OR s.kind = ?1)
               AND (?2 IS NULL OR s.language = ?2)
               AND s.kind != 'module'
               AND (?1 IS NOT NULL OR s.kind != 'constant')
               AND NOT (
                   s.name LIKE 'test_%'
                   AND (s.file_path LIKE '%test%' OR s.file_path LIKE '%spec%')
               )
               AND (s.decorators IS NULL OR (
                   s.decorators NOT LIKE '%,fixture,%'
                   AND s.decorators NOT LIKE '%,pytest.fixture,%'
                   AND s.decorators NOT LIKE '%,parametrize,%'
                   AND s.decorators NOT LIKE '%,pytest.mark.parametrize,%'
                   AND s.decorators NOT LIKE '%,pytest.mark.%'
                   AND s.decorators NOT LIKE '%.route,%'
                   AND s.decorators NOT LIKE '%.get,%'
                   AND s.decorators NOT LIKE '%.post,%'
                   AND s.decorators NOT LIKE '%.put,%'
                   AND s.decorators NOT LIKE '%.delete,%'
                   AND s.decorators NOT LIKE '%.patch,%'
                   AND s.decorators NOT LIKE '%.websocket,%'
                   AND s.decorators NOT LIKE '%triton.jit,%'
                   AND s.decorators NOT LIKE '%numba.jit,%'
                   AND s.decorators NOT LIKE '%numba.njit,%'
                   AND s.decorators NOT LIKE '%nb.njit,%'
                   AND s.decorators NOT LIKE '%,jit,%'
                   AND s.decorators NOT LIKE '%,njit,%'
                   AND s.decorators NOT LIKE '%celery.task,%'
                   AND s.decorators NOT LIKE '%shared_task,%'
                   AND s.decorators NOT LIKE '%,task,%'
                   AND s.decorators NOT LIKE '%click.command,%'
                   AND s.decorators NOT LIKE '%click.group,%'
                   AND s.decorators NOT LIKE '%,command,%'
                   AND s.decorators NOT LIKE '%,callback,%'
                   AND s.decorators NOT LIKE '%,test,%'
                   AND s.decorators NOT LIKE '%,tokio::test,%'
                   AND s.decorators NOT LIKE '%,tokio::main,%'
                   AND s.decorators NOT LIKE '%,async_std::main,%'
                   AND s.decorators NOT LIKE '%,async_std::test,%'
                   AND s.decorators NOT LIKE '%,Fact,%'
                   AND s.decorators NOT LIKE '%,Theory,%'
                   AND s.decorators NOT LIKE '%,Test,%'
                   AND s.decorators NOT LIKE '%,TestMethod,%'
                   AND s.decorators NOT LIKE '%,TestCase,%'
                   AND s.decorators NOT LIKE '%,TestFixture,%'
                   AND s.decorators NOT LIKE '%,SetUp,%'
                   AND s.decorators NOT LIKE '%,TearDown,%'
                   AND s.decorators NOT LIKE '%,OneTimeSetUp,%'
                   AND s.decorators NOT LIKE '%,OneTimeTearDown,%'
                   AND s.decorators NOT LIKE '%,TestInitialize,%'
                   AND s.decorators NOT LIKE '%,TestCleanup,%'
                   AND s.decorators NOT LIKE '%,ClassInitialize,%'
                   AND s.decorators NOT LIKE '%,ClassCleanup,%'
                   AND s.decorators NOT LIKE '%,HttpGet,%'
                   AND s.decorators NOT LIKE '%,HttpPost,%'
                   AND s.decorators NOT LIKE '%,HttpPut,%'
                   AND s.decorators NOT LIKE '%,HttpDelete,%'
                   AND s.decorators NOT LIKE '%,HttpPatch,%'
                   AND s.decorators NOT LIKE '%,Route,%'
                   AND s.decorators NOT LIKE '%,Benchmark,%'
                   AND s.decorators NOT LIKE '%,GlobalSetup,%'
                   AND s.decorators NOT LIKE '%,GlobalCleanup,%'
                   AND s.decorators NOT LIKE '%,Override,%'
                   AND s.decorators NOT LIKE '%,ParameterizedTest,%'
                   AND s.decorators NOT LIKE '%,RepeatedTest,%'
                   AND s.decorators NOT LIKE '%,TestFactory,%'
                   AND s.decorators NOT LIKE '%,BeforeEach,%'
                   AND s.decorators NOT LIKE '%,AfterEach,%'
                   AND s.decorators NOT LIKE '%,BeforeAll,%'
                   AND s.decorators NOT LIKE '%,AfterAll,%'
                   AND s.decorators NOT LIKE '%,Before,%'
                   AND s.decorators NOT LIKE '%,After,%'
                   AND s.decorators NOT LIKE '%,BeforeMethod,%'
                   AND s.decorators NOT LIKE '%,AfterMethod,%'
                   AND s.decorators NOT LIKE '%,BeforeClass,%'
                   AND s.decorators NOT LIKE '%,AfterClass,%'
                   AND s.decorators NOT LIKE '%,RequestMapping,%'
                   AND s.decorators NOT LIKE '%,GetMapping,%'
                   AND s.decorators NOT LIKE '%,PostMapping,%'
                   AND s.decorators NOT LIKE '%,PutMapping,%'
                   AND s.decorators NOT LIKE '%,DeleteMapping,%'
                   AND s.decorators NOT LIKE '%,PatchMapping,%'
                   AND s.decorators NOT LIKE '%,Bean,%'
                   AND s.decorators NOT LIKE '%,Scheduled,%'
                   AND s.decorators NOT LIKE '%,EventListener,%'
                   AND s.decorators NOT LIKE '%,DataProvider,%'
                   AND s.decorators NOT LIKE '%,TestDox,%'
                   AND s.decorators NOT LIKE '%,Group,%'
                   AND s.decorators NOT LIKE '%,AsCommand,%'
                   AND s.decorators NOT LIKE '%,AsController,%'
                   AND s.decorators NOT LIKE '%,AsEventListener,%'
                   AND s.decorators NOT LIKE '%,On,%'
               ))
               AND (
                   ?4 = 'root'
                   OR (?4 = 'file' AND s.file_path = ?3)
                   OR (
                       ?4 = 'directory'
                       AND substr(s.file_path, 1, length(?3) + 1) = ?3 || '/'
                   )
               )
               AND (?5 = 0 OR s.production = 1)
         )"
    )
}

fn maybe_production_symbol_filter(enabled: bool, alias: &str) -> String {
    if enabled {
        format!("AND {alias}.production = 1")
    } else {
        String::new()
    }
}

fn normalize_repo_relative(path: &str) -> Option<String> {
    let mut components: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            value => components.push(value),
        }
    }
    (!components.is_empty()).then(|| components.join("/"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: i64,
    pub from_id: i64,
    pub to_id: Option<i64>,
    pub to_name: String,
    pub kind: String, // "calls" | "imports" | "inherits"
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub indexed_at: i64,
    pub symbol_count: u32,
}

/// One task-spec file ready to be inserted into the FTS5 corpus.
#[derive(Debug, Clone)]
pub struct TaskSpecEntry {
    pub path: String,
    pub title: String,
    pub body: String,
}

/// One durable project-history artifact ready for the derived FTS5 corpus.
/// Markdown files remain authoritative; this row is only a rebuildable search view.
#[derive(Debug, Clone)]
pub struct ProjectHistoryEntry {
    pub path: String,
    pub kind: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Serialize)]
pub struct ScratchpadEntry {
    pub id: i64,
    pub ts: i64,
    pub agent: String,
    pub kind: String,
    pub body: String,
}

/// One result from `mmcg_tasks` — a matched task-spec with snippet + score.
#[derive(Debug, Clone, Serialize)]
pub struct TaskSpecHit {
    pub path: String,
    pub title: String,
    /// Body excerpt around the matched terms with `«match»` highlights.
    pub excerpt: String,
    /// FTS5 BM25 score — lower = better match (negative is normal).
    pub score: f64,
}

/// One observed match from the derived project-history corpus.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectHistoryHit {
    pub path: String,
    pub kind: String,
    pub title: String,
    /// Body excerpt around matched terms with `«match»` highlights.
    pub excerpt: String,
    /// FTS5 BM25 score — lower = better match (negative is normal).
    pub score: f64,
    /// Source lexemes highlighted by the same FTS5 query that selected this
    /// row. Kept out of the public history response; bounded briefs use it as
    /// typed match evidence without returning a title or excerpt.
    #[serde(skip)]
    pub(crate) matched_terms: Vec<String>,
}

#[derive(Debug, Clone)]
struct ConceptDocument {
    name_search: String,
    path_search: String,
    path_sort: String,
    signature_search: String,
    documentation_search: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ConceptStoreHit {
    pub name: String,
    pub kind: String,
    pub language: Option<String>,
    pub path: String,
    pub line: u32,
    pub signature_shape: String,
    pub name_matched: bool,
    pub path_matched: bool,
    pub signature_matched: bool,
    pub documentation_matched: bool,
    pub score: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConceptFinalizeStats {
    pub rows: u32,
    pub orphans_purged: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ConceptDocumentationStats {
    pub indexed_documents: u32,
    pub secret_omitted: u32,
    pub size_omitted: u32,
    pub unsupported_language_files: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingConceptDocumentation {
    pub symbol_index: usize,
    pub documentation_search: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PendingConceptCorpus {
    pub language_supported: bool,
    pub documents: Vec<PendingConceptDocumentation>,
    pub secret_omitted: u32,
    pub size_omitted: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NormalizedConceptDocumentation {
    pub value: String,
    pub truncated: bool,
}

fn lowercase_concept(value: &str) -> String {
    value.chars().flat_map(char::to_lowercase).collect()
}

fn push_concept_term(term: String, seen: &mut HashSet<String>, terms: &mut Vec<String>) {
    if !term.is_empty() && seen.insert(term.clone()) {
        terms.push(term);
    }
}

fn split_concept_identifier(value: &[char], seen: &mut HashSet<String>, terms: &mut Vec<String>) {
    if value.is_empty() {
        return;
    }
    let joined = lowercase_concept(
        &value
            .iter()
            .filter(|character| **character != '_' && **character != '-')
            .collect::<String>(),
    );
    push_concept_term(joined, seen, terms);

    for part in value.split(|character| matches!(character, '_' | '-')) {
        if part.is_empty() {
            continue;
        }
        let mut start = 0usize;
        for index in 1..part.len() {
            let previous = part[index - 1];
            let current = part[index];
            let next = part.get(index + 1).copied();
            let lower_to_upper = previous.is_lowercase() && current.is_uppercase();
            let acronym_to_word = previous.is_uppercase()
                && current.is_uppercase()
                && next.is_some_and(char::is_lowercase);
            let letter_to_digit = previous.is_alphabetic() && current.is_numeric();
            let digit_to_letter = previous.is_numeric() && current.is_alphabetic();
            if lower_to_upper || acronym_to_word || letter_to_digit || digit_to_letter {
                push_concept_term(
                    lowercase_concept(&part[start..index].iter().collect::<String>()),
                    seen,
                    terms,
                );
                start = index;
            }
        }
        push_concept_term(
            lowercase_concept(&part[start..].iter().collect::<String>()),
            seen,
            terms,
        );
    }
}

fn normalized_concept_terms(value: &str, formatting_whitespace: bool) -> Option<Vec<String>> {
    let mut seen = HashSet::new();
    let mut terms = Vec::new();
    let mut identifier = Vec::new();
    for character in value.chars() {
        if character.is_control() {
            if formatting_whitespace && matches!(character, '\r' | '\n' | '\t') {
                split_concept_identifier(&identifier, &mut seen, &mut terms);
                identifier.clear();
                continue;
            }
            return None;
        }
        if character.is_alphanumeric() || matches!(character, '_' | '-') {
            identifier.push(character);
        } else {
            split_concept_identifier(&identifier, &mut seen, &mut terms);
            identifier.clear();
        }
    }
    split_concept_identifier(&identifier, &mut seen, &mut terms);
    Some(terms)
}

/// Query lexical normalization. Index fields use the same splitting and
/// lowercase rules but additionally admit CR/LF/tab as declaration separators.
pub(crate) fn concept_terms(value: &str) -> Option<Vec<String>> {
    normalized_concept_terms(value, false)
}

fn concept_index_terms(value: &str) -> Option<Vec<String>> {
    normalized_concept_terms(value, true)
}

fn concept_path_sort(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => output.push('/'),
            character if character.is_control() => {
                output.push_str(&format!("\\u{:06x}", character as u32));
            }
            character => output.extend(character.to_lowercase()),
        }
    }
    output
}

#[derive(Clone, Copy, Default)]
struct SignatureDialect {
    hash_line_comments: bool,
    nested_block_comments: bool,
    csharp_raw_strings: bool,
    interpolated_strings: bool,
    javascript_regex: bool,
    python_defaults: bool,
    php_heredoc: bool,
    cpp_operators: bool,
}

fn signature_dialect(path: &str) -> SignatureDialect {
    let language = crate::indexer::guess_language_for(path).unwrap_or_default();
    SignatureDialect {
        hash_line_comments: matches!(language, "python" | "php"),
        nested_block_comments: language == "rust",
        csharp_raw_strings: language == "csharp",
        interpolated_strings: matches!(language, "python" | "csharp"),
        javascript_regex: matches!(language, "javascript" | "typescript" | "tsx" | "vue"),
        python_defaults: language == "python",
        php_heredoc: language == "php",
        cpp_operators: language == "cpp",
    }
}

fn php_heredoc_signature_end(characters: &[char], start: usize) -> Option<usize> {
    if characters.get(start..start + 3) != Some(&['<', '<', '<']) {
        return None;
    }
    let mut cursor = start + 3;
    while characters
        .get(cursor)
        .is_some_and(|character| matches!(character, ' ' | '\t'))
    {
        cursor += 1;
    }
    let quote = characters
        .get(cursor)
        .copied()
        .filter(|character| matches!(character, '\'' | '"'));
    if quote.is_some() {
        cursor += 1;
    }
    let label_start = cursor;
    while characters
        .get(cursor)
        .is_some_and(|character| character.is_alphanumeric() || *character == '_')
    {
        cursor += 1;
    }
    if cursor == label_start {
        return Some(characters.len());
    }
    let label = &characters[label_start..cursor];
    if let Some(quote) = quote {
        if characters.get(cursor) != Some(&quote) {
            return Some(characters.len());
        }
        cursor += 1;
    }
    while characters
        .get(cursor)
        .is_some_and(|character| !matches!(character, '\r' | '\n'))
    {
        cursor += 1;
    }
    if characters.get(cursor) == Some(&'\r') {
        cursor += 1;
    }
    if characters.get(cursor) == Some(&'\n') {
        cursor += 1;
    } else {
        return Some(characters.len());
    }

    while cursor < characters.len() {
        let mut candidate = cursor;
        while characters
            .get(candidate)
            .is_some_and(|character| matches!(character, ' ' | '\t'))
        {
            candidate += 1;
        }
        if characters.get(candidate..candidate + label.len()) == Some(label) {
            candidate += label.len();
            while characters
                .get(candidate)
                .is_some_and(|character| matches!(character, ' ' | '\t'))
            {
                candidate += 1;
            }
            if characters.get(candidate) == Some(&';') {
                candidate += 1;
                while characters
                    .get(candidate)
                    .is_some_and(|character| matches!(character, ' ' | '\t'))
                {
                    candidate += 1;
                }
            }
            if characters.get(candidate) == Some(&',') {
                return Some(candidate);
            }
            if characters
                .get(candidate)
                .is_none_or(|character| matches!(character, '\r' | '\n'))
            {
                if characters.get(candidate) == Some(&'\r') {
                    candidate += 1;
                }
                if characters.get(candidate) == Some(&'\n') {
                    candidate += 1;
                }
                return Some(candidate);
            }
        }
        while characters
            .get(cursor)
            .is_some_and(|character| !matches!(character, '\r' | '\n'))
        {
            cursor += 1;
        }
        if characters.get(cursor) == Some(&'\r') {
            cursor += 1;
        }
        if characters.get(cursor) == Some(&'\n') {
            cursor += 1;
        }
    }
    Some(characters.len())
}

fn quoted_signature_end(characters: &[char], start: usize, dialect: SignatureDialect) -> usize {
    let quote = characters[start];
    let quote_run = characters[start..]
        .iter()
        .take_while(|character| **character == quote)
        .count();
    let csharp_raw = dialect.csharp_raw_strings && quote_run >= 3;
    let width = if csharp_raw {
        quote_run
    } else if quote_run >= 3 {
        3
    } else {
        1
    };
    let mut prefix_start = start;
    while prefix_start > 0
        && start - prefix_start < 3
        && matches!(
            characters[prefix_start - 1],
            'f' | 'F' | 'r' | 'R' | '$' | '@'
        )
    {
        prefix_start -= 1;
    }
    let prefix = characters[prefix_start..start]
        .iter()
        .collect::<String>()
        .to_ascii_lowercase();
    let prefix_boundary = prefix_start == 0
        || !characters[prefix_start - 1].is_alphanumeric() && characters[prefix_start - 1] != '_';
    let interpolated = dialect.interpolated_strings
        && prefix_boundary
        && (dialect.python_defaults && matches!(prefix.as_str(), "f" | "fr" | "rf")
            || dialect.csharp_raw_strings && matches!(prefix.as_str(), "$" | "$@" | "@$"));
    let doubled_quote_escape = dialect.csharp_raw_strings && prefix.contains('@');
    let mut interpolation_depth = 0usize;
    let mut index = start + width;
    while index < characters.len() {
        if !csharp_raw && characters[index] == '\\' {
            index = index.saturating_add(2);
            continue;
        }
        if interpolated && width == 1 {
            if interpolation_depth > 0 && matches!(characters[index], '\'' | '"') {
                index = quoted_signature_end(characters, index, SignatureDialect::default());
                continue;
            }
            match characters[index] {
                '{' if characters.get(index + 1) == Some(&'{') => {
                    index += 2;
                    continue;
                }
                '{' => {
                    interpolation_depth += 1;
                    index += 1;
                    continue;
                }
                '}' if characters.get(index + 1) == Some(&'}') => {
                    index += 2;
                    continue;
                }
                '}' if interpolation_depth > 0 => {
                    interpolation_depth -= 1;
                    index += 1;
                    continue;
                }
                _ => {}
            }
        }
        if interpolation_depth == 0
            && (0..width).all(|offset| characters.get(index + offset) == Some(&quote))
        {
            if doubled_quote_escape && characters.get(index + width) == Some(&quote) {
                index += width + 1;
                continue;
            }
            return index + width;
        }
        index += 1;
    }
    characters.len()
}

fn rust_raw_signature_end(characters: &[char], start: usize) -> Option<usize> {
    let boundary =
        start == 0 || !characters[start - 1].is_alphanumeric() && characters[start - 1] != '_';
    if !boundary {
        return None;
    }
    let mut cursor = start;
    if matches!(characters.get(cursor), Some('b' | 'c')) {
        cursor += 1;
    }
    if characters.get(cursor) != Some(&'r') {
        return None;
    }
    cursor += 1;
    let hashes_start = cursor;
    while characters.get(cursor) == Some(&'#') && cursor - hashes_start <= 255 {
        cursor += 1;
    }
    let hashes = cursor - hashes_start;
    if hashes > 255 || characters.get(cursor) != Some(&'"') {
        return None;
    }
    cursor += 1;
    while cursor < characters.len() {
        if characters[cursor] == '"'
            && (0..hashes).all(|offset| characters.get(cursor + 1 + offset) == Some(&'#'))
        {
            return Some(cursor + 1 + hashes);
        }
        cursor += 1;
    }
    Some(characters.len())
}

fn cpp_raw_signature_end(characters: &[char], start: usize) -> Option<usize> {
    let boundary =
        start == 0 || !characters[start - 1].is_alphanumeric() && characters[start - 1] != '_';
    if !boundary {
        return None;
    }
    let raw_prefix_end = match characters.get(start..) {
        Some(['R', '"', ..]) => start + 1,
        Some(['L' | 'u' | 'U', 'R', '"', ..]) => start + 2,
        Some(['u', '8', 'R', '"', ..]) => start + 3,
        _ => return None,
    };
    let mut open = raw_prefix_end + 1;
    while open < characters.len()
        && open - (raw_prefix_end + 1) <= 16
        && characters[open] != '('
        && !characters[open].is_whitespace()
        && !matches!(characters[open], '\\' | ')')
    {
        open += 1;
    }
    if open - (raw_prefix_end + 1) > 16 || characters.get(open) != Some(&'(') {
        return None;
    }
    let delimiter = &characters[raw_prefix_end + 1..open];
    let mut cursor = open + 1;
    while cursor < characters.len() {
        if characters[cursor] == ')'
            && characters.get(cursor + 1..cursor + 1 + delimiter.len()) == Some(delimiter)
            && characters.get(cursor + 1 + delimiter.len()) == Some(&'"')
        {
            return Some(cursor + 2 + delimiter.len());
        }
        cursor += 1;
    }
    Some(characters.len())
}

fn raw_signature_end(characters: &[char], start: usize) -> Option<usize> {
    rust_raw_signature_end(characters, start).or_else(|| cpp_raw_signature_end(characters, start))
}

fn template_signature_end(characters: &[char], start: usize) -> Option<usize> {
    if characters.get(start) != Some(&'`') {
        return None;
    }

    #[derive(Clone, Copy)]
    enum Mode {
        Template,
        Expression { depth: usize, regex_allowed: bool },
    }

    let mut modes = vec![Mode::Template];
    let mut index = start + 1;
    while index < characters.len() {
        match modes.last().copied().unwrap_or(Mode::Template) {
            Mode::Template => match characters[index] {
                '\\' => index = index.saturating_add(2),
                '$' if characters.get(index + 1) == Some(&'{') => {
                    modes.push(Mode::Expression {
                        depth: 1,
                        regex_allowed: true,
                    });
                    index += 2;
                }
                '`' => {
                    modes.pop();
                    index += 1;
                    if modes.is_empty() {
                        return Some(index);
                    }
                }
                _ => index += 1,
            },
            Mode::Expression {
                depth,
                regex_allowed,
            } => {
                if let Some(end) = comment_signature_end(
                    characters,
                    index,
                    SignatureDialect {
                        javascript_regex: true,
                        ..SignatureDialect::default()
                    },
                ) {
                    index = end;
                    continue;
                }
                if matches!(characters[index], '\'' | '"') {
                    index = quoted_signature_end(characters, index, SignatureDialect::default());
                    *modes.last_mut().expect("template expression exists") = Mode::Expression {
                        depth,
                        regex_allowed: false,
                    };
                    continue;
                }
                if characters[index].is_alphabetic() || matches!(characters[index], '_' | '$') {
                    let token_start = index;
                    index += 1;
                    while characters.get(index).is_some_and(|character| {
                        character.is_alphanumeric() || matches!(character, '_' | '$')
                    }) {
                        index += 1;
                    }
                    let token = characters[token_start..index].iter().collect::<String>();
                    *modes.last_mut().expect("template expression exists") = Mode::Expression {
                        depth,
                        regex_allowed: matches!(
                            token.as_str(),
                            "await"
                                | "case"
                                | "delete"
                                | "do"
                                | "else"
                                | "in"
                                | "instanceof"
                                | "new"
                                | "of"
                                | "return"
                                | "throw"
                                | "typeof"
                                | "void"
                                | "yield"
                        ),
                    };
                    continue;
                }
                match characters[index] {
                    '`' => {
                        *modes.last_mut().expect("template expression exists") = Mode::Expression {
                            depth,
                            regex_allowed: false,
                        };
                        modes.push(Mode::Template);
                        index += 1;
                    }
                    '{' => {
                        *modes.last_mut().expect("template mode exists") = Mode::Expression {
                            depth: depth + 1,
                            regex_allowed: true,
                        };
                        index += 1;
                    }
                    '}' if depth == 1 => {
                        modes.pop();
                        index += 1;
                    }
                    '}' => {
                        *modes.last_mut().expect("template mode exists") = Mode::Expression {
                            depth: depth - 1,
                            regex_allowed: false,
                        };
                        index += 1;
                    }
                    '/' => {
                        if regex_allowed {
                            if let Some(end) = js_regex_signature_end(characters, index) {
                                index = end;
                                *modes.last_mut().expect("template expression exists") =
                                    Mode::Expression {
                                        depth,
                                        regex_allowed: false,
                                    };
                                continue;
                            }
                        }
                        index += 1;
                        *modes.last_mut().expect("template expression exists") = Mode::Expression {
                            depth,
                            regex_allowed: true,
                        };
                    }
                    '(' | '[' | ',' | ':' | '?' | '!' | '&' | '|' | '+' | '-' | '*' | '%' | '^'
                    | '~' | '=' => {
                        index += 1;
                        *modes.last_mut().expect("template expression exists") = Mode::Expression {
                            depth,
                            regex_allowed: true,
                        };
                    }
                    ')' | ']' => {
                        index += 1;
                        *modes.last_mut().expect("template expression exists") = Mode::Expression {
                            depth,
                            regex_allowed: false,
                        };
                    }
                    character if character.is_whitespace() => index += 1,
                    _ => {
                        index += 1;
                        *modes.last_mut().expect("template expression exists") = Mode::Expression {
                            depth,
                            regex_allowed: false,
                        };
                    }
                }
            }
        }
    }
    Some(characters.len())
}

fn comment_signature_end(
    characters: &[char],
    start: usize,
    dialect: SignatureDialect,
) -> Option<usize> {
    match (characters.get(start), characters.get(start + 1)) {
        (Some('#'), _) if dialect.hash_line_comments => {
            let content_start = start + 1;
            Some(
                characters[content_start..]
                    .iter()
                    .position(|character| matches!(character, '\r' | '\n'))
                    .map_or(characters.len(), |offset| content_start + offset),
            )
        }
        (Some('/'), Some('/')) => {
            let content_start = start + usize::from(characters[start] == '/') + 1;
            Some(
                characters[content_start..]
                    .iter()
                    .position(|character| matches!(character, '\r' | '\n'))
                    .map_or(characters.len(), |offset| content_start + offset),
            )
        }
        (Some('/'), Some('*')) => {
            let mut index = start + 2;
            let mut depth = 1usize;
            while index + 1 < characters.len() {
                if dialect.nested_block_comments
                    && characters[index] == '/'
                    && characters[index + 1] == '*'
                {
                    depth += 1;
                    index += 2;
                    continue;
                }
                if characters[index] == '*' && characters[index + 1] == '/' {
                    depth -= 1;
                    index += 2;
                    if depth == 0 {
                        return Some(index);
                    }
                    continue;
                }
                index += 1;
            }
            Some(characters.len())
        }
        _ => None,
    }
}

fn rust_lifetime_start(characters: &[char], start: usize) -> bool {
    if characters.get(start) != Some(&'\'') {
        return false;
    }
    let Some(first) = characters.get(start + 1) else {
        return false;
    };
    if !first.is_alphabetic() && *first != '_' {
        return false;
    }
    let mut end = start + 2;
    while characters
        .get(end)
        .is_some_and(|character| character.is_alphanumeric() || *character == '_')
    {
        end += 1;
    }
    characters.get(end) != Some(&'\'')
}

fn js_regex_signature_end(characters: &[char], start: usize) -> Option<usize> {
    if characters.get(start) != Some(&'/') || matches!(characters.get(start + 1), Some('/' | '*')) {
        return None;
    }
    let mut index = start + 1;
    let mut in_class = false;
    while index < characters.len() {
        match characters[index] {
            '\\' => index = index.saturating_add(2),
            '[' => {
                in_class = true;
                index += 1;
            }
            ']' => {
                in_class = false;
                index += 1;
            }
            '/' if !in_class => {
                index += 1;
                while characters
                    .get(index)
                    .is_some_and(|character| character.is_alphabetic())
                {
                    index += 1;
                }
                return Some(index);
            }
            _ => index += 1,
        }
    }
    Some(characters.len())
}

fn previous_non_whitespace(characters: &[char], index: usize) -> Option<usize> {
    (0..index)
        .rev()
        .find(|candidate| !characters[*candidate].is_whitespace())
}

fn next_non_whitespace(characters: &[char], index: usize) -> Option<usize> {
    (index + 1..characters.len()).find(|candidate| !characters[*candidate].is_whitespace())
}

fn generic_angle_has_close(characters: &[char], start: usize) -> bool {
    let (mut angles, mut parentheses, mut brackets, mut braces) = (1usize, 0usize, 0usize, 0usize);
    let mut index = start + 1;
    while index < characters.len() {
        if let Some(end) = raw_signature_end(characters, index) {
            index = end;
            continue;
        }
        if let Some(end) = template_signature_end(characters, index) {
            index = end;
            continue;
        }
        if let Some(end) = comment_signature_end(characters, index, SignatureDialect::default()) {
            index = end;
            continue;
        }
        if matches!(characters[index], '\'' | '"') {
            index = quoted_signature_end(characters, index, SignatureDialect::default());
            continue;
        }
        match characters[index] {
            '(' => parentheses += 1,
            ')' if parentheses == 0 && brackets == 0 && braces == 0 => return false,
            ')' => parentheses = parentheses.saturating_sub(1),
            '[' => brackets += 1,
            ']' if brackets == 0 && parentheses == 0 && braces == 0 => return false,
            ']' => brackets = brackets.saturating_sub(1),
            '{' => braces += 1,
            '}' if braces == 0 && parentheses == 0 && brackets == 0 => return false,
            '}' => braces = braces.saturating_sub(1),
            '<' if !matches!(characters.get(index + 1), Some('<' | '=')) => angles += 1,
            '>' if !matches!(characters.get(index + 1), Some('=')) => {
                angles -= 1;
                if angles == 0 {
                    return true;
                }
            }
            ';' if parentheses == 0 && brackets == 0 && braces == 0 => return false,
            _ => {}
        }
        index += 1;
    }
    false
}

fn generic_angle_open(characters: &[char], index: usize) -> bool {
    if characters.get(index) != Some(&'<') || matches!(characters.get(index + 1), Some('<' | '=')) {
        return false;
    }
    let Some(previous) = previous_non_whitespace(characters, index) else {
        return false;
    };
    let Some(next) = next_non_whitespace(characters, index) else {
        return false;
    };
    if !matches!(characters[previous], '>' | ':' | ')' | ']')
        && !characters[previous].is_alphanumeric()
        && characters[previous] != '_'
    {
        return false;
    }
    !matches!(
        characters[next],
        '<' | '=' | '>' | ',' | ';' | ')' | ']' | '}'
    ) && generic_angle_has_close(characters, index)
}

fn cpp_operator_equals(characters: &[char], index: usize) -> bool {
    let mut cursor = index;
    while cursor > 0 && characters[cursor - 1].is_whitespace() {
        cursor -= 1;
    }
    while cursor > 0
        && matches!(
            characters[cursor - 1],
            '+' | '-' | '*' | '/' | '%' | '&' | '|' | '^' | '!' | '<' | '>' | '~'
        )
    {
        cursor -= 1;
    }
    while cursor > 0 && characters[cursor - 1].is_whitespace() {
        cursor -= 1;
    }
    let end = cursor;
    while cursor > 0 && (characters[cursor - 1].is_alphanumeric() || characters[cursor - 1] == '_')
    {
        cursor -= 1;
    }
    characters[cursor..end].iter().collect::<String>() == "operator"
}

fn standalone_default_equals(characters: &[char], index: usize, dialect: SignatureDialect) -> bool {
    if characters.get(index) != Some(&'=') {
        return false;
    }
    if dialect.cpp_operators && cpp_operator_equals(characters, index) {
        return false;
    }
    let previous =
        previous_non_whitespace(characters, index).and_then(|value| characters.get(value));
    let next = characters.get(index + 1);
    !previous.is_some_and(|value| {
        matches!(
            value,
            '=' | '!' | '<' | '>' | '-' | ':' | '+' | '*' | '/' | '%' | '&' | '|' | '^'
        )
    }) && !next.is_some_and(|value| matches!(value, '=' | '>'))
}

fn skip_signature_default(
    characters: &[char],
    start: usize,
    base_depth: (usize, usize, usize, usize),
    dialect: SignatureDialect,
) -> usize {
    let (mut parentheses, mut brackets, mut braces, mut angles) = base_depth;
    let mut index = start;
    let mut regex_allowed = true;
    let mut python_lambda_parameters = 0usize;
    while index < characters.len() {
        if dialect.php_heredoc {
            if let Some(end) = php_heredoc_signature_end(characters, index) {
                index = end;
                regex_allowed = false;
                continue;
            }
        }
        if let Some(end) = raw_signature_end(characters, index) {
            index = end;
            regex_allowed = false;
            continue;
        }
        if let Some(end) = template_signature_end(characters, index) {
            index = end;
            regex_allowed = false;
            continue;
        }
        if let Some(end) = comment_signature_end(characters, index, dialect) {
            index = end;
            continue;
        }
        if characters[index] == '\'' && rust_lifetime_start(characters, index) {
            index += 1;
            regex_allowed = false;
            continue;
        }
        if matches!(characters[index], '\'' | '"') {
            index = quoted_signature_end(characters, index, dialect);
            regex_allowed = false;
            continue;
        }
        if dialect.javascript_regex && characters[index] == '/' && regex_allowed {
            if let Some(end) = js_regex_signature_end(characters, index) {
                index = end;
                regex_allowed = false;
                continue;
            }
        }
        if dialect.python_defaults
            && (parentheses, brackets, braces, angles) == base_depth
            && characters[index..].starts_with(&['l', 'a', 'm', 'b', 'd', 'a'])
            && (index == 0
                || !characters[index - 1].is_alphanumeric() && characters[index - 1] != '_')
            && characters
                .get(index + 6)
                .is_none_or(|character| !character.is_alphanumeric() && *character != '_')
        {
            python_lambda_parameters += 1;
            index += 6;
            regex_allowed = false;
            continue;
        }
        match characters[index] {
            '(' => {
                parentheses += 1;
                regex_allowed = true;
            }
            ')' if parentheses == base_depth.0 => return index,
            ')' => {
                parentheses = parentheses.saturating_sub(1);
                regex_allowed = false;
            }
            '[' => {
                brackets += 1;
                regex_allowed = true;
            }
            ']' if brackets == base_depth.1 => return index,
            ']' => {
                brackets = brackets.saturating_sub(1);
                regex_allowed = false;
            }
            '{' => {
                braces += 1;
                regex_allowed = true;
            }
            '}' if braces == base_depth.2 => return index,
            '}' => {
                braces = braces.saturating_sub(1);
                regex_allowed = false;
            }
            '<' if generic_angle_open(characters, index) => {
                angles += 1;
                regex_allowed = true;
            }
            '>' if angles > base_depth.3 => {
                angles -= 1;
                regex_allowed = false;
            }
            ':' if dialect.python_defaults
                && python_lambda_parameters > 0
                && (parentheses, brackets, braces, angles) == base_depth =>
            {
                python_lambda_parameters -= 1;
                regex_allowed = true;
            }
            ',' if dialect.python_defaults
                && python_lambda_parameters > 0
                && (parentheses, brackets, braces, angles) == base_depth =>
            {
                regex_allowed = true;
            }
            ',' | ';' if (parentheses, brackets, braces, angles) == base_depth => {
                return index + usize::from(characters[index] == ',');
            }
            character if character.is_whitespace() => {}
            '=' | ':' | '?' | '!' | '&' | '|' | '+' | '-' | '*' | '/' | '%' | '^' | '~' => {
                regex_allowed = true;
            }
            _ => regex_allowed = false,
        }
        index += 1;
    }
    index
}

fn redact_signature(value: &str, dialect: SignatureDialect) -> Option<String> {
    if value.len() > CONCEPT_SIGNATURE_SCAN_MAX_BYTES {
        return None;
    }
    let characters = value.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(value.len().min(CONCEPT_SIGNATURE_MAX_BYTES));
    let (mut parentheses, mut brackets, mut braces, mut angles) = (0, 0, 0, 0);
    let mut index = 0usize;
    let mut regex_allowed = true;
    while index < characters.len() {
        if characters[index].is_control() {
            if matches!(characters[index], '\r' | '\n' | '\t') {
                output.push(' ');
                index += 1;
                continue;
            }
            return None;
        }
        if dialect.php_heredoc {
            if let Some(end) = php_heredoc_signature_end(&characters, index) {
                output.push(' ');
                index = end;
                regex_allowed = false;
                continue;
            }
        }
        if let Some(end) = raw_signature_end(&characters, index) {
            output.push(' ');
            index = end;
            regex_allowed = false;
            continue;
        }
        if let Some(end) = template_signature_end(&characters, index) {
            output.push(' ');
            index = end;
            regex_allowed = false;
            continue;
        }
        if let Some(end) = comment_signature_end(&characters, index, dialect) {
            output.push(' ');
            index = end;
            continue;
        }
        if characters[index] == '\'' && rust_lifetime_start(&characters, index) {
            output.push(characters[index]);
            index += 1;
            regex_allowed = false;
            continue;
        }
        if matches!(characters[index], '\'' | '"') {
            output.push(' ');
            index = quoted_signature_end(&characters, index, dialect);
            regex_allowed = false;
            continue;
        }
        if dialect.javascript_regex && characters[index] == '/' && regex_allowed {
            if let Some(end) = js_regex_signature_end(&characters, index) {
                output.push(' ');
                index = end;
                regex_allowed = false;
                continue;
            }
        }
        if standalone_default_equals(&characters, index, dialect) {
            output.push(' ');
            index = skip_signature_default(
                &characters,
                index + 1,
                (parentheses, brackets, braces, angles),
                dialect,
            );
            regex_allowed = true;
            continue;
        }
        let numeric_boundary =
            index == 0 || !characters[index - 1].is_alphanumeric() && characters[index - 1] != '_';
        if characters[index].is_ascii_digit() && numeric_boundary {
            output.push(' ');
            index += 1;
            while index < characters.len()
                && (characters[index].is_alphanumeric()
                    || matches!(characters[index], '_' | '.' | '+' | '-'))
            {
                index += 1;
            }
            regex_allowed = false;
            continue;
        }
        match characters[index] {
            '(' => {
                parentheses += 1;
                regex_allowed = true;
            }
            ')' => {
                parentheses = parentheses.saturating_sub(1);
                regex_allowed = false;
            }
            '[' => {
                brackets += 1;
                regex_allowed = true;
            }
            ']' => {
                brackets = brackets.saturating_sub(1);
                regex_allowed = false;
            }
            '{' => {
                braces += 1;
                regex_allowed = true;
            }
            '}' => {
                braces = braces.saturating_sub(1);
                regex_allowed = false;
            }
            '<' if generic_angle_open(&characters, index) => {
                angles += 1;
                regex_allowed = true;
            }
            '>' => {
                angles = angles.saturating_sub(1);
                regex_allowed = false;
            }
            character if character.is_whitespace() => {}
            '=' | ':' | '?' | '!' | '&' | '|' | '+' | '-' | '*' | '/' | '%' | '^' | '~' | ',' => {
                regex_allowed = true;
            }
            _ => regex_allowed = false,
        }
        output.push(characters[index]);
        index += 1;
    }
    Some(output)
}

fn bounded_concept_field_with_status(
    value: &str,
    byte_limit: usize,
) -> Option<NormalizedConceptDocumentation> {
    let terms = concept_index_terms(value)?;
    let mut output = String::new();
    let mut truncated = terms.len() > CONCEPT_INDEX_TERM_LIMIT;
    for term in terms.into_iter().take(CONCEPT_INDEX_TERM_LIMIT) {
        if term.len() > CONCEPT_TERM_MAX_BYTES {
            truncated = true;
            continue;
        }
        let separator = usize::from(!output.is_empty());
        if output
            .len()
            .saturating_add(separator)
            .saturating_add(term.len())
            > byte_limit
        {
            truncated = true;
            break;
        }
        if separator == 1 {
            output.push(' ');
        }
        output.push_str(&term);
    }
    Some(NormalizedConceptDocumentation {
        value: output,
        truncated,
    })
}

fn bounded_concept_field(value: &str, byte_limit: usize) -> Option<String> {
    bounded_concept_field_with_status(value, byte_limit).map(|field| field.value)
}

pub(crate) fn normalize_concept_documentation(
    value: &str,
) -> Option<NormalizedConceptDocumentation> {
    bounded_concept_field_with_status(value, CONCEPT_FIELD_MAX_BYTES)
}

fn concept_document(
    name: &str,
    path: &str,
    signature: Option<&str>,
    documentation_search: &str,
) -> ConceptDocument {
    let name_search = bounded_concept_field(name, CONCEPT_FIELD_MAX_BYTES).unwrap_or_default();
    let path_search = bounded_concept_field(path, CONCEPT_FIELD_MAX_BYTES).unwrap_or_default();
    let signature_search = signature
        .and_then(|value| redact_signature(value, signature_dialect(path)))
        .and_then(|value| bounded_concept_field(&value, CONCEPT_SIGNATURE_MAX_BYTES))
        .unwrap_or_default();
    ConceptDocument {
        name_search,
        path_search,
        path_sort: concept_path_sort(path),
        signature_search,
        documentation_search: documentation_search.to_string(),
    }
}

fn insert_concept_document(
    connection: &Connection,
    symbol_id: i64,
    name: &str,
    path: &str,
    signature: Option<&str>,
    documentation_search: &str,
) -> SqlResult<()> {
    let document = concept_document(name, path, signature, documentation_search);
    connection.execute(
        "INSERT INTO symbol_concepts(
             symbol_id, name_search, path_search, path_sort, signature_search,
             documentation_search
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            symbol_id,
            document.name_search,
            document.path_search,
            document.path_sort,
            document.signature_search,
            document.documentation_search
        ],
    )?;
    Ok(())
}

pub(crate) fn concept_documentation_language_supported(language: &str) -> bool {
    matches!(
        language,
        "javascript" | "python" | "rust" | "tsx" | "typescript"
    )
}

fn valid_documentation_search(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= CONCEPT_FIELD_MAX_BYTES
        && value.split(' ').all(|term| {
            !term.is_empty()
                && term.len() <= CONCEPT_TERM_MAX_BYTES
                && term
                    .chars()
                    .all(|character| character.is_alphanumeric() || character == '_')
        })
}

fn concept_documentation_stats_on(connection: &Connection) -> SqlResult<ConceptDocumentationStats> {
    connection.query_row(
        "SELECT
             COALESCE(SUM(indexed_documents), 0),
             COALESCE(SUM(secret_omitted), 0),
             COALESCE(SUM(size_omitted), 0),
             COALESCE(SUM(CASE WHEN language_supported = 0 THEN 1 ELSE 0 END), 0)
         FROM concept_documentation_file_stats",
        [],
        |row| {
            let bounded = |value: i64| u32::try_from(value).unwrap_or(u32::MAX);
            Ok(ConceptDocumentationStats {
                indexed_documents: bounded(row.get(0)?),
                secret_omitted: bounded(row.get(1)?),
                size_omitted: bounded(row.get(2)?),
                unsupported_language_files: bounded(row.get(3)?),
            })
        },
    )
}

fn set_meta_on(connection: &Connection, key: &str, value: &str) -> SqlResult<()> {
    connection.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn update_concept_documentation_meta_on(connection: &Connection) -> SqlResult<()> {
    let stats = concept_documentation_stats_on(connection)?;
    for (key, value) in [
        (
            CONCEPT_DOCUMENTATION_LANGUAGES_META_KEY,
            CONCEPT_DOCUMENTATION_SUPPORTED_LANGUAGES.to_string(),
        ),
        (
            CONCEPT_DOCUMENTATION_INDEXED_META_KEY,
            stats.indexed_documents.to_string(),
        ),
        (
            CONCEPT_DOCUMENTATION_SECRET_OMITTED_META_KEY,
            stats.secret_omitted.to_string(),
        ),
        (
            CONCEPT_DOCUMENTATION_SIZE_OMITTED_META_KEY,
            stats.size_omitted.to_string(),
        ),
        (
            CONCEPT_DOCUMENTATION_UNSUPPORTED_META_KEY,
            stats.unsupported_language_files.to_string(),
        ),
    ] {
        set_meta_on(connection, key, &value)?;
    }
    Ok(())
}

fn highlighted_fts_terms(values: [&str; 2]) -> Vec<String> {
    const START: char = '\u{001e}';
    const END: char = '\u{001f}';
    const LIMIT: usize = 32;

    let mut seen = HashSet::new();
    let mut terms = Vec::new();
    for value in values {
        let mut remaining = value;
        while let Some(start) = remaining.find(START) {
            remaining = &remaining[start + START.len_utf8()..];
            let Some(end) = remaining.find(END) else {
                break;
            };
            let term = remaining[..end].trim();
            let is_token = !term.is_empty()
                && term.len() <= 512
                && term
                    .chars()
                    .all(|character| character.is_alphanumeric() || character == '_');
            if is_token && seen.insert(term.to_lowercase()) {
                terms.push(term.to_string());
                if terms.len() == LIMIT {
                    return terms;
                }
            }
            remaining = &remaining[end + END.len_utf8()..];
        }
    }
    terms
}

/// Per-file batch ready to be committed in a single transaction.
/// Symbols hold local indices; the store resolves them to rowids at commit time.
#[derive(Debug, Default)]
pub struct PendingFile {
    pub path: String,
    pub mtime: i64,
    pub content_sha256: String,
    /// Language id — `python`, `typescript`, `tsx`, `javascript`, `rust`.
    /// Stored on every symbol of this file; powers the `language` query filter
    /// (defends against cross-language name collisions in monorepos).
    pub language: String,
    pub symbols: Vec<PendingSymbol>,
    pub edges: Vec<PendingEdge>,
}

#[derive(Debug)]
pub struct PendingSymbol {
    pub name: String,
    pub kind: String,
    pub line_start: u32,
    pub line_end: u32,
    pub signature: Option<String>,
    /// Index into `symbols` vec of the parent (e.g. class for a method). None = top-level.
    pub parent_index: Option<usize>,
    /// Decorator/attribute names, comma-delimited with leading+trailing commas
    /// for safe `LIKE ',name,'` matching. e.g. `,pytest.fixture,property,`,
    /// `,tokio::main,`, or None.
    pub decorators: Option<String>,
}

#[derive(Debug)]
pub struct PendingEdge {
    /// Index into `symbols` vec of the symbol making the call/import.
    pub from_index: usize,
    /// Leaf name — `foo` in `obj.foo()`, `baz` in `from a.b import baz`.
    pub to_name: String,
    /// Fully-qualified path as in source — `obj.foo`, `a.b.baz`, `Foo::bar`.
    /// None if no resolvable path beyond the leaf.
    pub to_path: Option<String>,
    /// Type/namespace prefix — `SessionStore` for `SessionStore::new()`, `Foo`
    /// for `Foo::bar()`. None if no prefix (free function, plain method on a
    /// variable). Lets `mmcg_callers <Type>` find Rust constructor and
    /// associated-function calls that would otherwise hide under their leaf name.
    pub to_type: Option<String>,
    pub kind: String,
    pub line: u32,
}

pub struct Store {
    conn: Connection,
    _snapshot_dir: Option<tempfile::TempDir>,
    db_path: PathBuf,
    guard_stack: RefCell<Vec<GuardFrame>>,
    ops_counter: Arc<AtomicU64>,
    interrupt_source: Arc<AtomicU8>,
    default_budget: Cell<WorkBudget>,
    managed_root: Option<PathBuf>,
    serve_root: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
struct SourceFileState {
    len: u64,
    identity: crate::bounded_fs::StableFileIdentity,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct IndexFileState {
    database: SourceFileState,
    wal: Option<SourceFileState>,
}

#[derive(Debug, Clone, Copy)]
struct SnapshotCopyBudget {
    max_bytes: u64,
    deadline: Instant,
}

impl SnapshotCopyBudget {
    fn for_request(request_deadline: Option<Instant>) -> Self {
        let hard_deadline = Instant::now() + READ_ONLY_SNAPSHOT_TIMEOUT;
        Self {
            max_bytes: READ_ONLY_SNAPSHOT_MAX_BYTES,
            deadline: request_deadline.map_or(hard_deadline, |value| value.min(hard_deadline)),
        }
    }
}

fn sqlite_io_error(context: &str, error: std::io::Error) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
        Some(format!("{context}: {error}")),
    )
}

fn sqlite_snapshot_changed() -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
        Some("index changed while creating a read-only snapshot".into()),
    )
}

fn sqlite_snapshot_too_large() -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_TOOBIG),
        Some("active index snapshot exceeds the read-only copy limit".into()),
    )
}

fn sqlite_snapshot_timeout() -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_INTERRUPT),
        Some("active index snapshot exceeded its copy deadline".into()),
    )
}

fn sqlite_sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut value = db_path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn normalized_schema_sql(value: &str) -> String {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Quote {
        Single,
        Double,
        Backtick,
        Bracket,
    }

    let characters = value.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(value.len());
    let mut quote = None;
    let mut index = 0usize;
    while index < characters.len() {
        let character = characters[index];
        if let Some(active) = quote {
            output.push(character);
            let closing = match active {
                Quote::Single => '\'',
                Quote::Double => '"',
                Quote::Backtick => '`',
                Quote::Bracket => ']',
            };
            if character == closing {
                if characters.get(index + 1) == Some(&closing) {
                    output.push(closing);
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        quote = match character {
            '\'' => Some(Quote::Single),
            '"' => Some(Quote::Double),
            '`' => Some(Quote::Backtick),
            '[' => Some(Quote::Bracket),
            _ => None,
        };
        if quote.is_some() {
            output.push(character);
        } else if !character.is_whitespace() {
            output.extend(character.to_lowercase());
        }
        index += 1;
    }
    output
}

fn concept_schema_contract() -> &'static [(&'static str, &'static str, String)] {
    CONCEPT_SCHEMA_CONTRACT.get_or_init(|| {
        let connection = Connection::open_in_memory()
            .expect("constant concept schema contract must open an in-memory database");
        connection
            .execute_batch(
                "CREATE TABLE symbols(id INTEGER PRIMARY KEY);
                 CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )
            .expect("constant concept schema prerequisites must be valid");
        connection
            .execute_batch(CONCEPT_SCHEMA_DDL)
            .expect("constant concept schema DDL must be valid");
        CONCEPT_SCHEMA_OBJECTS
            .iter()
            .map(|&(object_type, name)| {
                let sql: String = connection
                    .query_row(
                        "SELECT sql FROM sqlite_master WHERE type = ?1 AND name = ?2",
                        params![object_type, name],
                        |row| row.get(0),
                    )
                    .expect("constant concept schema object must exist");
                (object_type, name, normalized_schema_sql(&sql))
            })
            .collect()
    })
}

fn schema_object_sql(
    connection: &Connection,
    object_type: &str,
    name: &str,
) -> SqlResult<Option<String>> {
    connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = ?1 AND name = ?2",
            params![object_type, name],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map(Option::flatten)
}

fn concept_schema_object_current_on(
    connection: &Connection,
    object_type: &str,
    name: &str,
) -> SqlResult<bool> {
    let Some((_, _, expected_sql)) =
        concept_schema_contract()
            .iter()
            .find(|(expected_type, expected_name, _)| {
                *expected_type == object_type && *expected_name == name
            })
    else {
        return Ok(false);
    };
    Ok(schema_object_sql(connection, object_type, name)?
        .is_some_and(|sql| normalized_schema_sql(&sql) == *expected_sql))
}

fn retire_schema_object_name(connection: &Connection, name: &str) -> SqlResult<()> {
    let mut statement = connection.prepare(
        "SELECT type FROM sqlite_master
         WHERE name = ?1 AND type IN ('trigger', 'index', 'view', 'table')
         ORDER BY CASE type
             WHEN 'trigger' THEN 1
             WHEN 'index' THEN 2
             WHEN 'view' THEN 3
             ELSE 4
         END",
    )?;
    let object_types = statement
        .query_map([name], |row| row.get::<_, String>(0))?
        .collect::<SqlResult<Vec<_>>>()?;
    drop(statement);
    let quoted_name = name.replace('"', "\"\"");
    for object_type in object_types {
        let keyword = match object_type.as_str() {
            "trigger" => "TRIGGER",
            "index" => "INDEX",
            "view" => "VIEW",
            "table" => "TABLE",
            _ => continue,
        };
        connection.execute_batch(&format!("DROP {keyword} IF EXISTS \"{quoted_name}\";"))?;
    }
    Ok(())
}

fn sqlite_bounded_error(
    context: &str,
    error: crate::bounded_fs::BoundedReadError,
) -> rusqlite::Error {
    match error {
        crate::bounded_fs::BoundedReadError::TooLarge { .. } => sqlite_snapshot_too_large(),
        crate::bounded_fs::BoundedReadError::SnapshotChanged => sqlite_snapshot_changed(),
        crate::bounded_fs::BoundedReadError::DeadlineExceeded
        | crate::bounded_fs::BoundedReadError::Interrupted => sqlite_snapshot_timeout(),
        crate::bounded_fs::BoundedReadError::Io(error) => sqlite_io_error(context, error),
        error => sqlite_io_error(
            context,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string()),
        ),
    }
}

fn source_file_state(
    root: &crate::bounded_fs::RootCapability,
    path: &Path,
    control: crate::bounded_fs::ReadControl<'_>,
) -> SqlResult<SourceFileState> {
    let file =
        crate::bounded_fs::read_regular_file_with_capability(root, path, u64::MAX, 0, control)
            .map_err(|error| sqlite_bounded_error("read index", error))?;
    Ok(SourceFileState {
        len: file.declared_len,
        identity: file.identity,
    })
}

fn optional_source_file_state(
    root: &crate::bounded_fs::RootCapability,
    path: &Path,
    control: crate::bounded_fs::ReadControl<'_>,
) -> SqlResult<Option<SourceFileState>> {
    match crate::bounded_fs::read_regular_file_with_capability(root, path, u64::MAX, 0, control) {
        Ok(file) => Ok(Some(SourceFileState {
            len: file.declared_len,
            identity: file.identity,
        })),
        Err(crate::bounded_fs::BoundedReadError::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(sqlite_bounded_error("read index sidecar", error)),
    }
}

fn index_file_state(db_path: &Path) -> SqlResult<IndexFileState> {
    index_file_state_with_control(db_path, crate::bounded_fs::ReadControl::default())
}

fn index_file_state_with_control(
    db_path: &Path,
    control: crate::bounded_fs::ReadControl<'_>,
) -> SqlResult<IndexFileState> {
    let absolute = std::path::absolute(db_path)
        .map_err(|error| sqlite_io_error("resolve index path", error))?;
    let parent = absolute
        .parent()
        .ok_or_else(|| rusqlite::Error::InvalidPath(absolute.clone()))?;
    let root = crate::bounded_fs::RootCapability::open(parent)
        .map_err(|error| sqlite_bounded_error("open index parent", error))?;
    Ok(IndexFileState {
        database: source_file_state(&root, &absolute, control)?,
        wal: optional_source_file_state(&root, &sqlite_sidecar_path(&absolute, "-wal"), control)?,
    })
}

fn copy_index_snapshot(
    db_path: &Path,
    budget: SnapshotCopyBudget,
) -> SqlResult<(tempfile::TempDir, PathBuf)> {
    if Instant::now() >= budget.deadline {
        return Err(sqlite_snapshot_timeout());
    }
    let absolute = std::path::absolute(db_path)
        .map_err(|error| sqlite_io_error("resolve index path", error))?;
    let parent = absolute
        .parent()
        .ok_or_else(|| rusqlite::Error::InvalidPath(absolute.clone()))?;
    let root = crate::bounded_fs::RootCapability::open(parent)
        .map_err(|error| sqlite_bounded_error("open index parent", error))?;
    let control = crate::bounded_fs::ReadControl {
        deadline: Some(budget.deadline),
        interrupted: None,
    };
    let before = IndexFileState {
        database: source_file_state(&root, &absolute, control)?,
        wal: optional_source_file_state(&root, &sqlite_sidecar_path(&absolute, "-wal"), control)?,
    };
    let expected_bytes = before
        .database
        .len
        .checked_add(before.wal.as_ref().map_or(0, |wal| wal.len))
        .ok_or_else(sqlite_snapshot_too_large)?;
    if expected_bytes > budget.max_bytes {
        return Err(sqlite_snapshot_too_large());
    }
    let snapshot_dir = tempfile::Builder::new()
        .prefix("mastermind-lens-index-")
        .tempdir()
        .map_err(|error| sqlite_io_error("create private index snapshot", error))?;
    let file_name = absolute
        .file_name()
        .ok_or_else(|| rusqlite::Error::InvalidPath(absolute.clone()))?;
    let snapshot_path = snapshot_dir.path().join(file_name);
    let source_wal = sqlite_sidecar_path(&absolute, "-wal");
    let snapshot_wal = sqlite_sidecar_path(&snapshot_path, "-wal");

    let mut copied = 0_u64;
    let mut database_output = std::fs::File::create(&snapshot_path)
        .map_err(|error| sqlite_io_error("create private index snapshot", error))?;
    let database = crate::bounded_fs::copy_regular_file_with_capability(
        &root,
        &absolute,
        budget.max_bytes,
        control,
        Some(before.database.identity),
        &mut database_output,
    )
    .map_err(|error| sqlite_bounded_error("copy index snapshot", error))?;
    database_output
        .flush()
        .map_err(|error| sqlite_io_error("flush private index snapshot", error))?;
    copied = copied
        .checked_add(database.declared_len)
        .filter(|value| *value <= budget.max_bytes)
        .ok_or_else(sqlite_snapshot_too_large)?;
    if before.wal.as_ref().is_some_and(|wal| wal.len > 0) {
        let mut wal_output = std::fs::File::create(&snapshot_wal)
            .map_err(|error| sqlite_io_error("create private WAL snapshot", error))?;
        let wal = crate::bounded_fs::copy_regular_file_with_capability(
            &root,
            &source_wal,
            budget.max_bytes.saturating_sub(copied),
            control,
            before.wal.as_ref().map(|wal| wal.identity),
            &mut wal_output,
        )
        .map_err(|error| sqlite_bounded_error("copy index WAL snapshot", error))?;
        wal_output
            .flush()
            .map_err(|error| sqlite_io_error("flush private WAL snapshot", error))?;
        copied
            .checked_add(wal.declared_len)
            .filter(|value| *value <= budget.max_bytes)
            .ok_or_else(sqlite_snapshot_too_large)?;
    }
    let after = IndexFileState {
        database: source_file_state(&root, &absolute, control)?,
        wal: optional_source_file_state(&root, &sqlite_sidecar_path(&absolute, "-wal"), control)?,
    };
    if after == before {
        return Ok((snapshot_dir, snapshot_path));
    }
    Err(sqlite_snapshot_changed())
}

fn open_private_index_snapshot(
    db_path: &Path,
    budget: SnapshotCopyBudget,
) -> SqlResult<(Connection, tempfile::TempDir)> {
    let (snapshot_dir, snapshot_path) = copy_index_snapshot(db_path, budget)?;
    let connection = Connection::open_with_flags(
        &snapshot_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE,
    )?;
    Ok((connection, snapshot_dir))
}

#[cfg(test)]
fn encode_sqlite_uri_path(path: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = Vec::with_capacity(path.len());
    let mut previous_was_slash = false;
    for &byte in path {
        let byte = if cfg!(windows) && byte == b'\\' {
            b'/'
        } else {
            byte
        };
        if byte == b'/' {
            if !previous_was_slash {
                encoded.push(byte);
            }
            previous_was_slash = true;
            continue;
        }
        previous_was_slash = false;
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b':') {
            encoded.push(byte);
        } else {
            encoded.push(b'%');
            encoded.push(HEX[usize::from(byte >> 4)]);
            encoded.push(HEX[usize::from(byte & 0x0f)]);
        }
    }
    encoded
}

#[cfg(test)]
fn windows_sqlite_uri_path(text: &str) -> Option<Vec<u8>> {
    if text
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\UNC\"))
    {
        return None;
    }
    let text = text.strip_prefix(r"\\?\").unwrap_or(text);
    if text.starts_with(r"\\") {
        return None;
    }
    let normalized = text.replace('\\', "/");
    let mut path = encode_sqlite_uri_path(normalized.as_bytes());
    if path.get(1) == Some(&b':') {
        path.insert(0, b'/');
    }
    Some(path)
}

fn connection_index_root(connection: &Connection) -> SqlResult<Option<String>> {
    let meta_exists = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta'",
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !meta_exists {
        return Ok(None);
    }
    connection
        .query_row("SELECT value FROM meta WHERE key='index_root'", [], |row| {
            row.get(0)
        })
        .optional()
}

impl Store {
    /// MCP serving may refresh only the canonical repository-owned index.
    /// Explicit custom paths are opened through the non-mutating snapshot
    /// reader, including when their schema is incompatible.
    pub fn open_for_serve(
        db_path: impl AsRef<Path>,
        managed_root: Option<&Path>,
    ) -> SqlResult<Self> {
        let db_path = db_path.as_ref();
        let Some(managed_root) = managed_root else {
            return Self::open_read_only(db_path);
        };
        let root = crate::bounded_fs::RootCapability::open(managed_root)
            .map_err(|error| sqlite_bounded_error("open managed repository root", error))?;
        let expected = root.canonical_root().join(".mastermind/mmcg.db");
        let requested_expected = root.requested_root().join(".mastermind/mmcg.db");
        let selected = std::path::absolute(db_path)
            .map_err(|error| sqlite_io_error("resolve managed index", error))?;
        if selected != expected && selected != requested_expected {
            let mut store = Self::open_read_only(&selected)?;
            store.serve_root = Some(root.canonical_root().to_path_buf());
            return Ok(store);
        }
        root.ensure_directory(Path::new(".mastermind"))
            .map_err(|error| sqlite_bounded_error("create managed index directory", error))?;
        let state =
            crate::bounded_fs::RootCapability::open(&root.canonical_root().join(".mastermind"))
                .map_err(|error| sqlite_bounded_error("open managed index directory", error))?;
        let existing_identity = match crate::bounded_fs::read_regular_file_with_capability(
            &state,
            &expected,
            u64::MAX,
            0,
            crate::bounded_fs::ReadControl::default(),
        ) {
            Ok(file) => Some(file.identity),
            Err(crate::bounded_fs::BoundedReadError::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(error) => return Err(sqlite_bounded_error("inspect managed index", error)),
        };
        if existing_identity.is_some() {
            let mut snapshot = Self::open_read_only(&expected)?;
            let authorized = snapshot
                .meta_value("index_root")?
                .and_then(|stored| PathBuf::from(stored).canonicalize().ok())
                .is_some_and(|stored| stored == root.canonical_root());
            if !authorized {
                snapshot.serve_root = Some(root.canonical_root().to_path_buf());
                return Ok(snapshot);
            }
            drop(snapshot);
        }
        let mut created_file = None;
        let expected_identity = match existing_identity {
            Some(identity) => identity,
            None => {
                let (file, identity) =
                    crate::bounded_fs::create_regular_file_with_capability(&state, &expected)
                        .map_err(|error| sqlite_bounded_error("create managed index", error))?;
                created_file = Some(file);
                identity
            }
        };
        let connection = Connection::open_with_flags(
            &expected,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        root.verify()
            .map_err(|error| sqlite_bounded_error("verify managed repository root", error))?;
        state
            .verify()
            .map_err(|error| sqlite_bounded_error("verify managed index directory", error))?;
        let opened_file = crate::bounded_fs::read_regular_file_with_capability(
            &state,
            &expected,
            u64::MAX,
            0,
            crate::bounded_fs::ReadControl::default(),
        )
        .map_err(|error| sqlite_bounded_error("verify managed index identity", error))?;
        let identity_matches = if existing_identity.is_some() {
            opened_file.identity == expected_identity
        } else {
            opened_file.identity.same_object(expected_identity)
        };
        if !identity_matches {
            return Err(sqlite_snapshot_changed());
        }
        drop(created_file);
        if existing_identity.is_some() {
            let authorized = connection_index_root(&connection)?
                .and_then(|stored| PathBuf::from(stored).canonicalize().ok())
                .is_some_and(|stored| stored == root.canonical_root());
            if !authorized {
                drop(connection);
                let mut store = Self::open_read_only(&expected)?;
                store.serve_root = Some(root.canonical_root().to_path_buf());
                return Ok(store);
            }
        }
        connection.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA busy_timeout = 5000;
            PRAGMA cache_size = -65536;
            "#,
        )?;
        let mut store = Self::from_connection(connection, expected, None);
        store.managed_root = Some(root.canonical_root().to_path_buf());
        store.serve_root = Some(root.canonical_root().to_path_buf());
        store.init_schema()?;
        root.verify()
            .map_err(|error| sqlite_bounded_error("verify managed repository root", error))?;
        state
            .verify()
            .map_err(|error| sqlite_bounded_error("verify managed index directory", error))?;
        Ok(store)
    }

    pub fn open(db_path: impl AsRef<Path>) -> SqlResult<Self> {
        let db_path = db_path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                    Some(format!("create parent dir: {e}")),
                )
            })?;
        }
        let conn = Connection::open(&db_path)?;
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA busy_timeout = 5000;
            PRAGMA cache_size = -65536;
            "#,
        )?;
        let store = Self::from_connection(conn, db_path, None);
        store.init_schema()?;
        Ok(store)
    }

    /// Open an existing index for query-only surfaces such as Lens.
    ///
    /// Unlike [`Store::open`], this never creates parent directories, creates
    /// a database, creates WAL/SHM sidecars, changes journal settings, or runs
    /// schema initialization. A missing or outdated index remains an explicit
    /// operator error instead of a read-only command mutating repository state
    /// while diagnosing it. The database and active WAL are copied through
    /// no-follow handles into one bounded private snapshot, so source sidecars
    /// remain untouched and special files cannot block SQLite startup.
    /// Long-running callers must reject a result if the source database or WAL
    /// changes during the query, as Lens does around each refresh.
    pub fn open_read_only(db_path: impl AsRef<Path>) -> SqlResult<Self> {
        Self::open_read_only_with_deadline(db_path, None)
    }

    pub(crate) fn open_read_only_with_deadline(
        db_path: impl AsRef<Path>,
        request_deadline: Option<Instant>,
    ) -> SqlResult<Self> {
        let requested_path = db_path.as_ref();
        let db_path = std::path::absolute(requested_path)
            .map_err(|error| sqlite_io_error("resolve read-only index", error))?;
        let snapshot_budget = SnapshotCopyBudget::for_request(request_deadline);
        let (conn, snapshot_dir) = open_private_index_snapshot(&db_path, snapshot_budget)?;
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            PRAGMA busy_timeout = 5000;
            PRAGMA cache_size = -65536;
            PRAGMA query_only = ON;
            "#,
        )?;
        Ok(Self::from_connection(conn, db_path, Some(snapshot_dir)))
    }

    /// Clone the exact connection snapshot into a private writable database.
    ///
    /// Temporal analysis uses this to rewind changed files to a Git baseline
    /// without checking out files or mutating the repository index. `VACUUM
    /// INTO` reads through this connection, so rows that came from an active
    /// WAL snapshot are retained. The only writes land under a temporary
    /// directory owned by the returned [`Store`].
    pub(crate) fn private_writable_snapshot(&self) -> SqlResult<Self> {
        let state = index_file_state(&self.db_path)?;
        let source_bytes = state
            .database
            .len
            .checked_add(state.wal.as_ref().map_or(0, |wal| wal.len))
            .ok_or_else(sqlite_snapshot_too_large)?;
        if source_bytes > READ_ONLY_SNAPSHOT_MAX_BYTES {
            return Err(sqlite_snapshot_too_large());
        }
        let snapshot_dir = tempfile::Builder::new()
            .prefix("mastermind-temporal-index-")
            .tempdir()
            .map_err(|error| sqlite_io_error("create temporal index snapshot", error))?;
        // A managed source connection carries SQLITE_OPEN_NOFOLLOW. SQLite
        // applies that policy to `VACUUM INTO` as well, so canonicalize the
        // private directory first on systems where the temp root (for example
        // macOS `/var`) is itself a symlink.
        let snapshot_path = snapshot_dir
            .path()
            .canonicalize()
            .map_err(|error| sqlite_io_error("resolve temporal index snapshot", error))?
            .join("mmcg.db");
        let query_only = self
            .conn
            .query_row("PRAGMA query_only", [], |row| row.get::<_, i64>(0))?
            != 0;
        if query_only {
            self.conn.execute_batch("PRAGMA query_only = OFF;")?;
        }
        let vacuum = self.conn.execute(
            "VACUUM INTO ?1",
            params![snapshot_path.to_string_lossy().as_ref()],
        );
        let restore = if query_only {
            self.conn.execute_batch("PRAGMA query_only = ON;")
        } else {
            Ok(())
        };
        vacuum?;
        restore?;

        let conn = Connection::open(&snapshot_path)?;
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA busy_timeout = 5000;
            PRAGMA cache_size = -65536;
            "#,
        )?;
        let mut snapshot = Self::from_connection(conn, snapshot_path, Some(snapshot_dir));
        snapshot.interrupt_source = Arc::clone(&self.interrupt_source);
        snapshot.set_default_work_budget(self.default_work_budget());
        if !snapshot.schema_current()? {
            return Err(rusqlite::Error::InvalidQuery);
        }
        snapshot.init_schema()?;
        Ok(snapshot)
    }

    pub fn schema_current(&self) -> SqlResult<bool> {
        let meta_exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta'",
                [],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !meta_exists {
            return Ok(false);
        }
        Ok(self.meta_value("schema_version")?.as_deref() == Some(SCHEMA_VERSION))
    }

    pub(crate) fn managed_root(&self) -> Option<&Path> {
        self.managed_root.as_deref()
    }

    pub(crate) fn serve_root(&self) -> Option<&Path> {
        self.serve_root.as_deref()
    }

    fn from_connection(
        conn: Connection,
        db_path: PathBuf,
        snapshot_dir: Option<tempfile::TempDir>,
    ) -> Self {
        Self {
            conn,
            _snapshot_dir: snapshot_dir,
            db_path,
            guard_stack: RefCell::new(Vec::new()),
            ops_counter: Arc::new(AtomicU64::new(0)),
            interrupt_source: Arc::new(AtomicU8::new(INTERRUPT_NONE)),
            default_budget: Cell::new(WorkBudget::from_millis(DEFAULT_SERVE_BUDGET_MS)),
            managed_root: None,
            serve_root: None,
        }
    }

    /// The work budget applied at MCP tool dispatch / CLI query boundaries
    /// unless a call site installs a tighter one. Defaults to
    /// [`DEFAULT_SERVE_BUDGET_MS`]; callers (`main.rs`, `commands/query.rs`)
    /// override it once at startup from `MMCG_QUERY_BUDGET_MS`.
    pub fn default_work_budget(&self) -> WorkBudget {
        self.default_budget.get()
    }

    /// Remaining effective budget of the innermost active guard. Separate
    /// private SQLite snapshots use this to preserve the parent request's
    /// deadline and operation cap instead of starting a fresh allowance.
    pub(crate) fn remaining_work_budget(&self) -> WorkBudget {
        let stack = self.guard_stack.borrow();
        let Some(frame) = stack.last() else {
            return self.default_work_budget();
        };
        let now = Instant::now();
        WorkBudget {
            deadline: frame
                .deadline
                .map(|deadline| deadline.saturating_duration_since(now)),
            op_ticks: frame.op_cap.map(|cap| {
                let used = self
                    .ops_counter
                    .load(Ordering::Relaxed)
                    .saturating_sub(frame.ops_baseline);
                cap.saturating_sub(used)
            }),
        }
    }

    /// Absolute deadline of the active outer request. Filesystem and Git work
    /// consume this exact deadline instead of converting the default duration
    /// into a fresh allowance for each phase.
    pub(crate) fn request_deadline(&self) -> Option<Instant> {
        self.guard_stack
            .borrow()
            .last()
            .and_then(|frame| frame.deadline)
    }

    pub(crate) fn work_budget_depth(&self) -> usize {
        self.guard_stack.borrow().len()
    }

    /// Cooperative check for non-SQL phases inside a guarded graph request.
    /// It shares the same cancel/budget marker used by SQLite so transports
    /// keep returning `cancelled` and `work_limit_exceeded` consistently.
    pub(crate) fn work_interrupted(&self) -> bool {
        if self.interrupt_source.load(Ordering::SeqCst) == INTERRUPT_CANCEL {
            return true;
        }
        let expired = self
            .guard_stack
            .borrow()
            .last()
            .is_some_and(|frame| frame.expired(&self.ops_counter));
        if expired {
            self.interrupt_source
                .store(INTERRUPT_BUDGET, Ordering::SeqCst);
        }
        expired
    }

    /// Override the default work budget directly — used by tests that need an
    /// exact, non-env-driven budget (e.g. a budget that is already expired at
    /// install time).
    pub fn set_default_work_budget(&self, budget: WorkBudget) {
        self.default_budget.set(budget);
    }

    /// Override the default work budget from a millisecond value, applying
    /// the "0 = unlimited" convention.
    pub fn set_default_work_budget_ms(&self, budget_ms: u64) {
        self.set_default_work_budget(WorkBudget::from_millis(budget_ms));
    }

    /// Push a new guard frame whose effective deadline/op cap are the min of
    /// its own values and whatever remains of the parent frame (if any), then
    /// installs the connection's progress handler from the new top frame —
    /// the stack is the progress handler's single owner; nothing else may
    /// call `Connection::progress_handler` on this connection. Returns `true`
    /// when the newly pushed frame is *already* exhausted (e.g. a zero
    /// budget) — callers must not run the guarded work in that case.
    pub fn push_work_budget(&self, budget: WorkBudget) -> bool {
        let mut stack = self.guard_stack.borrow_mut();
        let now = Instant::now();
        let own_deadline = budget.deadline.map(|d| now + d);
        let frame = match stack.last() {
            Some(parent) => {
                let parent_remaining_ops = parent.op_cap.map(|cap| {
                    let used = self
                        .ops_counter
                        .load(Ordering::Relaxed)
                        .saturating_sub(parent.ops_baseline);
                    cap.saturating_sub(used)
                });
                let deadline = match (own_deadline, parent.deadline) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (Some(a), None) => Some(a),
                    (None, other) => other,
                };
                let op_cap = match (budget.op_ticks, parent_remaining_ops) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (Some(a), None) => Some(a),
                    (None, other) => other,
                };
                GuardFrame {
                    deadline,
                    op_cap,
                    ops_baseline: self.ops_counter.load(Ordering::Relaxed),
                }
            }
            None => GuardFrame {
                deadline: own_deadline,
                op_cap: budget.op_ticks,
                ops_baseline: self.ops_counter.load(Ordering::Relaxed),
            },
        };
        let expired = frame.expired(&self.ops_counter);
        stack.push(frame);
        drop(stack);
        self.install_progress_handler();
        if expired && self.interrupt_source.load(Ordering::SeqCst) != INTERRUPT_CANCEL {
            self.interrupt_source
                .store(INTERRUPT_BUDGET, Ordering::SeqCst);
        }
        expired
    }

    /// Pop the innermost guard frame and reinstall the parent's progress
    /// handler (or clear it entirely when the stack is empty).
    pub fn pop_work_budget(&self) {
        {
            let mut stack = self.guard_stack.borrow_mut();
            stack.pop();
        }
        self.install_progress_handler();
    }

    fn install_progress_handler(&self) {
        let top = self.guard_stack.borrow().last().copied();
        let Some(frame) = top else {
            let _ = self.conn.progress_handler(0, None::<fn() -> bool>);
            return;
        };
        let ops_counter = self.ops_counter.clone();
        let interrupt_source = self.interrupt_source.clone();
        let _ = self.conn.progress_handler(
            1_000,
            Some(move || {
                ops_counter.fetch_add(1, Ordering::Relaxed);
                if interrupt_source.load(Ordering::SeqCst) == INTERRUPT_CANCEL {
                    true
                } else if frame.expired(&ops_counter) {
                    interrupt_source.store(INTERRUPT_BUDGET, Ordering::SeqCst);
                    true
                } else {
                    false
                }
            }),
        );
    }

    /// Run `f` under `budget`, composed via min with whatever guard is
    /// already installed (if any). This is the single owner of the
    /// connection's progress handler — no code path may install one outside
    /// this stack. On budget expiry (own or inherited), `f` is not run at all
    /// if the effective budget is already exhausted at push time; otherwise
    /// SQLite raises `SQLITE_INTERRUPT`, surfaced here as a matchable
    /// `rusqlite::Error::SqliteFailure` with `ErrorCode::OperationInterrupted`.
    pub fn with_work_budget<T>(
        &self,
        budget: WorkBudget,
        f: impl FnOnce() -> SqlResult<T>,
    ) -> SqlResult<T> {
        if self.push_work_budget(budget) {
            self.pop_work_budget();
            return Err(Self::interrupted_error());
        }
        let result = f();
        self.pop_work_budget();
        result
    }

    /// Run a deliberately narrower, recoverable precision budget below an
    /// already-active request guard. If only this local frame expires, consume
    /// its budget marker after restoring the still-live parent. Request expiry
    /// and cancellation remain marked for the transport to map normally.
    fn with_local_work_budget<T>(
        &self,
        budget: WorkBudget,
        f: impl FnOnce() -> SqlResult<T>,
    ) -> SqlResult<T> {
        let had_parent = self.work_budget_depth() > 0;
        let interrupt_before = self.interrupt_source.load(Ordering::SeqCst);
        let result = self.with_work_budget(budget, f);
        let parent_expired = self
            .guard_stack
            .borrow()
            .last()
            .is_some_and(|frame| frame.expired(&self.ops_counter));
        if had_parent && interrupt_before == INTERRUPT_NONE && !parent_expired {
            let _ = self.interrupt_source.compare_exchange(
                INTERRUPT_BUDGET,
                INTERRUPT_NONE,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
        result
    }

    fn interrupted_error() -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_INTERRUPT),
            Some("query interrupted: work budget exceeded".to_string()),
        )
    }

    /// Read and clear which mechanism raised the most recent interrupt (if
    /// any) — single-shot so a stale value from an earlier, internally
    /// recovered interrupt (e.g. `change_impact`'s graph-portion degrade)
    /// never leaks into the next check.
    pub fn take_interrupt_source(&self) -> Option<InterruptSource> {
        match self.interrupt_source.swap(INTERRUPT_NONE, Ordering::SeqCst) {
            INTERRUPT_BUDGET => Some(InterruptSource::Budget),
            INTERRUPT_CANCEL => Some(InterruptSource::Cancel),
            _ => None,
        }
    }

    pub(crate) fn interrupt_source(&self) -> Option<InterruptSource> {
        match self.interrupt_source.load(Ordering::SeqCst) {
            INTERRUPT_BUDGET => Some(InterruptSource::Budget),
            INTERRUPT_CANCEL => Some(InterruptSource::Cancel),
            _ => None,
        }
    }

    /// Consume a handled budget marker without clearing a cancel that won the
    /// race. Reserved for recoverable local precision limits.
    pub(crate) fn consume_budget_interrupt(&self) -> bool {
        self.interrupt_source
            .compare_exchange(
                INTERRUPT_BUDGET,
                INTERRUPT_NONE,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
    }

    /// A cloneable, cross-thread handle that marks the interrupt source as
    /// `Cancel` and aborts whatever statement is currently running on this
    /// connection. Used by `serve_io`'s reader thread to implement client
    /// cancel notifications.
    pub fn cancel_handle(&self) -> CancelHandle {
        CancelHandle {
            interrupt_source: self.interrupt_source.clone(),
            sqlite: Arc::new(self.conn.get_interrupt_handle()),
        }
    }

    /// Stable long-running SQLite work for interrupt-path tests. Keeping the
    /// query behind `Store` lets MCP watchdog tests exercise the real
    /// cross-thread `InterruptHandle` without manufacturing a large fixture.
    #[cfg(test)]
    pub(crate) fn run_interrupt_probe(&self, budget: WorkBudget) -> SqlResult<i64> {
        self.with_work_budget(budget, || {
            self.conn
                .prepare(
                    "WITH RECURSIVE cnt(x) AS (
                     SELECT 1
                     UNION ALL
                     SELECT x + 1 FROM cnt WHERE x < 100000000
                 )
                 SELECT count(*) FROM cnt",
                )?
                .query_row([], |row| row.get(0))
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn init_schema(&self) -> SqlResult<()> {
        // If the derived graph schema changes, drop the rebuildable graph while
        // preserving repository identity and the durable auxiliary channels.
        // Keeping `index_root` is essential: otherwise retained history or
        // scratchpad rows make the next in-place index run look like unsafe,
        // unbound data and it correctly refuses to guess a repository.
        let meta_exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta'",
                [],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);

        if meta_exists {
            let stored: Option<String> = self
                .conn
                .query_row(
                    "SELECT value FROM meta WHERE key='schema_version'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            if stored.as_deref() != Some(SCHEMA_VERSION) {
                eprintln!(
                    "[mmcg] schema version mismatch (have {:?}, need {}). Rebuilding — re-run `mastermind index <root>` to repopulate.",
                    stored, SCHEMA_VERSION
                );
                self.conn.execute_batch(CONCEPT_SCHEMA_DROP_SQL)?;
                self.conn.execute_batch(
                    r#"
                    DROP TABLE IF EXISTS edges;
                    DROP TABLE IF EXISTS symbols;
                    DROP TABLE IF EXISTS files;
                    DROP TABLE IF EXISTS task_specs;
                    DROP TABLE IF EXISTS task_specs_fts;
                    "#,
                )?;
            } else {
                // Repair a partially missing FTS5 corpus before the general
                // CREATE TABLE batch asks SQLite to initialize its broken
                // virtual-table module. The rebuilt corpus remains dirty until
                // a successful full index finalizes it.
                self.ensure_concept_schema()?;
            }
        }

        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS symbols (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                name         TEXT NOT NULL,
                kind         TEXT NOT NULL,
                file_path    TEXT NOT NULL,
                line_start   INTEGER NOT NULL,
                line_end     INTEGER NOT NULL,
                signature    TEXT,
                parent_id    INTEGER REFERENCES symbols(id) ON DELETE CASCADE,
                language     TEXT,
                decorators   TEXT,
                production   INTEGER NOT NULL DEFAULT 1
            );
            CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
            CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_path);
            CREATE INDEX IF NOT EXISTS idx_symbols_parent ON symbols(parent_id);
            CREATE INDEX IF NOT EXISTS idx_symbols_language ON symbols(language);
            CREATE INDEX IF NOT EXISTS idx_symbols_production_file
                ON symbols(production, file_path);
            CREATE INDEX IF NOT EXISTS idx_symbols_production_name
                ON symbols(production, name);

            CREATE TABLE IF NOT EXISTS edges (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                from_id   INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
                to_id     INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
                to_name   TEXT NOT NULL,
                to_path   TEXT,
                to_type   TEXT,
                kind      TEXT NOT NULL,
                line      INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_edges_from ON edges(from_id);
            CREATE INDEX IF NOT EXISTS idx_edges_to ON edges(to_id);
            CREATE INDEX IF NOT EXISTS idx_edges_to_name ON edges(to_name);
            CREATE INDEX IF NOT EXISTS idx_edges_to_path ON edges(to_path);
            CREATE INDEX IF NOT EXISTS idx_edges_to_type ON edges(to_type);
            CREATE INDEX IF NOT EXISTS idx_edges_kind ON edges(kind);
            CREATE INDEX IF NOT EXISTS idx_edges_calls_to_name
                ON edges(to_name, from_id) WHERE kind = 'calls';
            CREATE INDEX IF NOT EXISTS idx_edges_calls_to_type
                ON edges(to_type, from_id)
                WHERE kind = 'calls' AND to_type IS NOT NULL AND to_type <> '';

            CREATE TABLE IF NOT EXISTS files (
                path                    TEXT PRIMARY KEY,
                indexed_at              INTEGER NOT NULL,
                symbol_count            INTEGER NOT NULL,
                structural_fingerprint  TEXT NOT NULL DEFAULT '',
                content_sha256          TEXT NOT NULL DEFAULT '',
                production              INTEGER NOT NULL DEFAULT 1
            );
            CREATE INDEX IF NOT EXISTS idx_files_production_path
                ON files(production, path);

            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- Task-spec corpus, populated by the indexer from `.mastermind/tasks/<NNN>-<name>/spec.md`.
            -- `mmcg_tasks(query)` uses it so planners can recall past designs and
            -- verdicts. FTS5 gives BM25 ranking + snippet().
            -- `path` is UNINDEXED — we don't tokenize file paths.
            CREATE VIRTUAL TABLE IF NOT EXISTS task_specs_fts USING fts5(
                path UNINDEXED,
                title,
                body,
                tokenize = 'porter unicode61 remove_diacritics 2'
            );

            -- Rebuildable search view over durable project-history Markdown.
            -- `kind` is metadata for exact filtering, not tokenized evidence.
            CREATE VIRTUAL TABLE IF NOT EXISTS project_history_fts USING fts5(
                path UNINDEXED,
                kind UNINDEXED,
                title,
                body,
                tokenize = 'porter unicode61 remove_diacritics 2'
            );

            -- Cross-agent scratchpad. Live in-session channel between Mastermind
            -- subagents (planner → executor → auditor); counterpart to the
            -- cross-session `.mastermind/tasks/_lessons.md`.
            -- Additive table — no SCHEMA_VERSION bump needed; IF NOT EXISTS lets
            -- existing DBs adopt it without a rebuild.
            CREATE TABLE IF NOT EXISTS scratchpad (
                id    INTEGER PRIMARY KEY AUTOINCREMENT,
                ts    INTEGER NOT NULL,
                agent TEXT NOT NULL,
                kind  TEXT NOT NULL,
                body  TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_scratchpad_ts ON scratchpad(ts);
            CREATE INDEX IF NOT EXISTS idx_scratchpad_agent ON scratchpad(agent);

            -- Optional SCIP semantic overlay. These tables are additive and
            -- deliberately separate from `symbols` / `edges`: Tree-sitter is
            -- still the default graph and remains usable when no overlay exists.
            CREATE TABLE IF NOT EXISTS semantic_sources (
                id                       INTEGER PRIMARY KEY CHECK (id = 1),
                tool_name                TEXT NOT NULL,
                tool_version             TEXT NOT NULL,
                project_root             TEXT NOT NULL,
                artifact_path            TEXT NOT NULL,
                artifact_sha256          TEXT NOT NULL,
                imported_at              INTEGER NOT NULL,
                document_count           INTEGER NOT NULL,
                definition_count         INTEGER NOT NULL,
                edge_count               INTEGER NOT NULL,
                text_verified_documents  INTEGER NOT NULL,
                repository_verified      INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS semantic_documents (
                path                  TEXT PRIMARY KEY,
                source_id             INTEGER NOT NULL REFERENCES semantic_sources(id) ON DELETE CASCADE,
                language              TEXT NOT NULL,
                position_encoding     TEXT NOT NULL,
                content_sha256        TEXT NOT NULL,
                source_text_verified  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_semantic_documents_source
                ON semantic_documents(source_id);
            CREATE TABLE IF NOT EXISTS semantic_definitions (
                id             INTEGER PRIMARY KEY AUTOINCREMENT,
                source_id      INTEGER NOT NULL REFERENCES semantic_sources(id) ON DELETE CASCADE,
                symbol         TEXT NOT NULL,
                display_name   TEXT NOT NULL,
                kind           TEXT NOT NULL,
                file_path      TEXT NOT NULL REFERENCES semantic_documents(path) ON DELETE CASCADE,
                line           INTEGER NOT NULL,
                character      INTEGER NOT NULL,
                end_line       INTEGER NOT NULL,
                end_character  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_semantic_definitions_symbol
                ON semantic_definitions(symbol);
            CREATE INDEX IF NOT EXISTS idx_semantic_definitions_display
                ON semantic_definitions(display_name);
            CREATE INDEX IF NOT EXISTS idx_semantic_definitions_file
                ON semantic_definitions(file_path);
            CREATE TABLE IF NOT EXISTS semantic_edges (
                id                 INTEGER PRIMARY KEY AUTOINCREMENT,
                source_id          INTEGER NOT NULL REFERENCES semantic_sources(id) ON DELETE CASCADE,
                from_symbol        TEXT,
                from_display_name  TEXT,
                from_file          TEXT NOT NULL REFERENCES semantic_documents(path) ON DELETE CASCADE,
                from_line          INTEGER NOT NULL,
                from_character     INTEGER NOT NULL,
                occurrence_line    INTEGER NOT NULL,
                occurrence_character INTEGER NOT NULL,
                to_symbol          TEXT NOT NULL,
                to_display_name    TEXT NOT NULL,
                to_file            TEXT REFERENCES semantic_documents(path) ON DELETE CASCADE,
                to_line            INTEGER,
                to_character       INTEGER,
                kind               TEXT NOT NULL,
                UNIQUE (
                    source_id, from_symbol, from_file, from_line, from_character,
                    occurrence_line, occurrence_character, to_symbol, to_file, kind
                )
            );
            CREATE INDEX IF NOT EXISTS idx_semantic_edges_from_symbol
                ON semantic_edges(from_symbol);
            CREATE INDEX IF NOT EXISTS idx_semantic_edges_to_symbol
                ON semantic_edges(to_symbol);
            CREATE INDEX IF NOT EXISTS idx_semantic_edges_from_file
                ON semantic_edges(from_file);
            CREATE INDEX IF NOT EXISTS idx_semantic_edges_to_file
                ON semantic_edges(to_file);
            CREATE INDEX IF NOT EXISTS idx_semantic_edges_kind
                ON semantic_edges(kind);

            -- Declarative community facts. Producers never receive SQLite
            -- access: Mastermind validates a v1 manifest, normalizes every
            -- record, and atomically owns these tables.
            CREATE TABLE IF NOT EXISTS fact_sources (
                id                   INTEGER PRIMARY KEY AUTOINCREMENT,
                api_version          TEXT NOT NULL,
                producer_name        TEXT NOT NULL,
                producer_version     TEXT NOT NULL,
                dataset              TEXT NOT NULL,
                provenance_kind      TEXT NOT NULL,
                capabilities         TEXT NOT NULL,
                repository_identity  TEXT NOT NULL,
                revision             TEXT NOT NULL,
                manifest_sha256      TEXT NOT NULL,
                manifest_bytes       INTEGER NOT NULL,
                signature_status     TEXT NOT NULL DEFAULT 'unsigned',
                signing_key_id       TEXT,
                signature_sha256     TEXT,
                signature_bytes      INTEGER,
                signing_public_key   TEXT,
                signature_value      TEXT,
                signed_manifest_digest TEXT,
                imported_at          INTEGER NOT NULL,
                file_count           INTEGER NOT NULL,
                annotation_count     INTEGER NOT NULL,
                relationship_count   INTEGER NOT NULL,
                UNIQUE (producer_name, dataset)
            );
            CREATE INDEX IF NOT EXISTS idx_fact_sources_revision
                ON fact_sources(repository_identity, revision);
            CREATE TABLE IF NOT EXISTS fact_files (
                source_id  INTEGER NOT NULL REFERENCES fact_sources(id) ON DELETE CASCADE,
                path       TEXT NOT NULL,
                sha256     TEXT NOT NULL,
                bytes      INTEGER NOT NULL,
                PRIMARY KEY (source_id, path)
            );
            CREATE INDEX IF NOT EXISTS idx_fact_files_path ON fact_files(path);
            CREATE TABLE IF NOT EXISTS fact_artifacts (
                source_id    INTEGER NOT NULL REFERENCES fact_sources(id) ON DELETE CASCADE,
                artifact_id  TEXT NOT NULL,
                path         TEXT NOT NULL,
                sha256       TEXT NOT NULL,
                bytes        INTEGER NOT NULL,
                PRIMARY KEY (source_id, artifact_id)
            );
            CREATE TABLE IF NOT EXISTS fact_annotations (
                source_id   INTEGER NOT NULL REFERENCES fact_sources(id) ON DELETE CASCADE,
                fact_id     TEXT NOT NULL,
                path        TEXT NOT NULL,
                line        INTEGER NOT NULL,
                column_no   INTEGER,
                end_line    INTEGER,
                end_column  INTEGER,
                severity    TEXT NOT NULL,
                category    TEXT NOT NULL,
                title       TEXT NOT NULL,
                message     TEXT NOT NULL,
                PRIMARY KEY (source_id, fact_id),
                FOREIGN KEY (source_id, path)
                    REFERENCES fact_files(source_id, path) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_fact_annotations_path
                ON fact_annotations(path, line);
            CREATE TABLE IF NOT EXISTS fact_relationships (
                source_id    INTEGER NOT NULL REFERENCES fact_sources(id) ON DELETE CASCADE,
                fact_id      TEXT NOT NULL,
                relation     TEXT NOT NULL,
                from_path    TEXT NOT NULL,
                from_line    INTEGER NOT NULL,
                from_column  INTEGER,
                to_path      TEXT NOT NULL,
                to_line      INTEGER NOT NULL,
                to_column    INTEGER,
                confidence   TEXT NOT NULL,
                label        TEXT NOT NULL,
                PRIMARY KEY (source_id, fact_id),
                FOREIGN KEY (source_id, from_path)
                    REFERENCES fact_files(source_id, path) ON DELETE CASCADE,
                FOREIGN KEY (source_id, to_path)
                    REFERENCES fact_files(source_id, path) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_fact_relationships_from
                ON fact_relationships(from_path, from_line);
            CREATE INDEX IF NOT EXISTS idx_fact_relationships_to
                ON fact_relationships(to_path, to_line);
            "#,
        )?;

        // Idempotent column add for pre-0.28 DBs (CREATE TABLE IF NOT EXISTS
        // above is a no-op once the table exists). SQLite raises `duplicate
        // column name` if already present — the steady-state case, so we discard.
        let _ = self.conn.execute(
            "ALTER TABLE files ADD COLUMN structural_fingerprint TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE files ADD COLUMN content_sha256 TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE semantic_sources ADD COLUMN repository_verified INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE fact_sources ADD COLUMN signature_status TEXT NOT NULL DEFAULT 'unsigned'",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE fact_sources ADD COLUMN signing_key_id TEXT",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE fact_sources ADD COLUMN signature_sha256 TEXT",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE fact_sources ADD COLUMN signature_bytes INTEGER",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE fact_sources ADD COLUMN signing_public_key TEXT",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE fact_sources ADD COLUMN signature_value TEXT",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE fact_sources ADD COLUMN signed_manifest_digest TEXT",
            [],
        );

        // Stamp the active version on first init and after a derived-schema
        // rebuild. Other metadata — especially index_root — remains intact.
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![SCHEMA_VERSION],
        )?;
        self.ensure_concept_schema()?;
        Ok(())
    }

    fn concept_schema_objects_current_on(connection: &Connection) -> SqlResult<bool> {
        for (object_type, name, _) in concept_schema_contract() {
            if !concept_schema_object_current_on(connection, object_type, name)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn concept_schema_objects_current(&self) -> SqlResult<bool> {
        Self::concept_schema_objects_current_on(&self.conn)
    }

    pub(crate) fn ensure_concept_schema(&self) -> SqlResult<()> {
        if self.concept_schema_objects_current()? {
            return Ok(());
        }
        let virtual_table_sql = schema_object_sql(&self.conn, "table", "symbol_concepts_fts")?;
        let virtual_table_uses_fts5 = virtual_table_sql.as_deref().is_some_and(|sql| {
            let normalized = normalized_schema_sql(sql);
            normalized.starts_with("createvirtualtable") && normalized.contains("usingfts5(")
        });
        let tx = self.conn.unchecked_transaction()?;
        if virtual_table_uses_fts5 {
            // SQLite cannot drop an FTS5 virtual table from a newly opened
            // connection when a generated shadow table is absent or malformed.
            // Retire wrong-type/wrong-DDL collisions, then recreate only the
            // disposable minimum needed for the constructor. This same
            // transaction drops and rebuilds the complete corpus.
            for shadow in CONCEPT_SHADOW_NAMES {
                if !concept_schema_object_current_on(&tx, "table", shadow)? {
                    retire_schema_object_name(&tx, shadow)?;
                }
            }
            tx.execute_batch(CONCEPT_SHADOW_REPAIR_SQL)?;
            #[cfg(test)]
            if FAIL_CONCEPT_SCHEMA_AFTER_SHADOW_REPAIR.with(Cell::get) {
                return Err(rusqlite::Error::InvalidQuery);
            }
        } else {
            // A damaged schema can lose or replace only the virtual-table row
            // while generated shadows remain. Every name is private derived
            // state, so retire any table/view/index collision before rebuild.
            retire_schema_object_name(&tx, "symbol_concepts_fts")?;
            for shadow in CONCEPT_SHADOW_NAMES {
                retire_schema_object_name(&tx, shadow)?;
            }
        }
        for trigger in CONCEPT_TRIGGER_NAMES {
            retire_schema_object_name(&tx, trigger)?;
        }
        retire_schema_object_name(&tx, "symbol_concepts_fts")?;
        for shadow in CONCEPT_SHADOW_NAMES {
            retire_schema_object_name(&tx, shadow)?;
        }
        retire_schema_object_name(&tx, "concept_documentation_file_stats")?;
        retire_schema_object_name(&tx, "symbol_concepts")?;
        tx.execute_batch(CONCEPT_SCHEMA_DDL)?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![CONCEPT_NORMALIZATION_META_KEY, CONCEPT_CONTRACT_DIRTY],
        )?;
        tx.commit()?;
        if !self.concept_schema_objects_current()? {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(())
    }

    fn semantic_tables_exist(&self) -> SqlResult<bool> {
        self.conn
            .query_row(
                "SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'semantic_sources'",
                [],
                |_| Ok(true),
            )
            .optional()
            .map(|value| value.unwrap_or(false))
    }

    fn semantic_repository_verified_column_exists(&self) -> SqlResult<bool> {
        self.conn
            .query_row(
                "SELECT 1 FROM pragma_table_info('semantic_sources')
                 WHERE name = 'repository_verified'",
                [],
                |_| Ok(true),
            )
            .optional()
            .map(|value| value.unwrap_or(false))
    }

    /// Metadata for the currently imported compiler-resolved overlay.
    /// `None` is the normal Tree-sitter-only state, including a v7 database
    /// created by an older binary before the additive semantic tables existed.
    pub fn semantic_source(&self) -> SqlResult<Option<SemanticSource>> {
        if !self.semantic_tables_exist()? {
            return Ok(None);
        }
        let repository_verified = if self.semantic_repository_verified_column_exists()? {
            "repository_verified"
        } else {
            "0 AS repository_verified"
        };
        self.conn
            .query_row(
                &format!(
                    "SELECT tool_name, tool_version, project_root, artifact_path,
                        artifact_sha256, imported_at, document_count,
                        definition_count, edge_count, text_verified_documents,
                        {repository_verified}
                     FROM semantic_sources WHERE id = 1"
                ),
                [],
                |row| {
                    let documents = row.get::<_, u32>(6)?;
                    let text_verified_documents = row.get::<_, u32>(9)?;
                    Ok(SemanticSource {
                        format: "scip",
                        tool_name: row.get(0)?,
                        tool_version: row.get(1)?,
                        project_root: row.get(2)?,
                        artifact_path: row.get(3)?,
                        artifact_sha256: row.get(4)?,
                        imported_at: row.get(5)?,
                        documents,
                        definitions: row.get(7)?,
                        edges: row.get(8)?,
                        text_verified_documents,
                        repository_verified: row.get(10)?,
                        revision_verified: documents > 0 && documents == text_verified_documents,
                    })
                },
            )
            .optional()
    }

    /// Atomically replace the entire SCIP snapshot after it has been decoded
    /// and validated in memory. Any parse/path/range error therefore leaves the
    /// previous known-good overlay intact.
    pub(crate) fn replace_semantic_overlay(&self, batch: &SemanticImportBatch) -> SqlResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM semantic_sources", [])?;
        tx.execute(
            "INSERT INTO semantic_sources(
                id, tool_name, tool_version, project_root, artifact_path,
                artifact_sha256, imported_at, document_count, definition_count,
                edge_count, text_verified_documents, repository_verified
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                batch.source.tool_name,
                batch.source.tool_version,
                batch.source.project_root,
                batch.source.artifact_path,
                batch.source.artifact_sha256,
                batch.source.imported_at,
                batch.source.documents,
                batch.source.definitions,
                batch.source.edges,
                batch.source.text_verified_documents,
                batch.source.repository_verified,
            ],
        )?;
        {
            let mut statement = tx.prepare(
                "INSERT INTO semantic_documents(
                    path, source_id, language, position_encoding,
                    content_sha256, source_text_verified
                 ) VALUES (?1, 1, ?2, ?3, ?4, ?5)",
            )?;
            for document in &batch.documents {
                statement.execute(params![
                    document.path,
                    document.language,
                    document.position_encoding,
                    document.content_sha256,
                    document.source_text_verified,
                ])?;
            }
        }
        {
            let mut statement = tx.prepare(
                "INSERT INTO semantic_definitions(
                    source_id, symbol, display_name, kind, file_path, line,
                    character, end_line, end_character
                 ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for definition in &batch.definitions {
                statement.execute(params![
                    definition.symbol,
                    definition.display_name,
                    definition.kind,
                    definition.file,
                    definition.line,
                    definition.character,
                    definition.end_line,
                    definition.end_character,
                ])?;
            }
        }
        {
            let mut statement = tx.prepare(
                "INSERT OR IGNORE INTO semantic_edges(
                    source_id, from_symbol, from_display_name, from_file,
                    from_line, from_character, occurrence_line,
                    occurrence_character, to_symbol, to_display_name,
                    to_file, to_line, to_character, kind
                 ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?;
            for edge in &batch.edges {
                statement.execute(params![
                    edge.from_symbol,
                    edge.from_display_name,
                    edge.from_file,
                    edge.from_line,
                    edge.from_character,
                    edge.occurrence_line,
                    edge.occurrence_character,
                    edge.to_symbol,
                    edge.to_display_name,
                    edge.to_file,
                    edge.to_line,
                    edge.to_character,
                    edge.kind,
                ])?;
            }
        }
        tx.commit()
    }

    pub(crate) fn semantic_definitions(
        &self,
        query: Option<&str>,
        limit: usize,
    ) -> SqlResult<(Vec<SemanticDefinition>, bool)> {
        if !self.semantic_tables_exist()? {
            return Ok((Vec::new(), false));
        }
        let limit = limit.clamp(1, 5_000);
        let mut sql = String::from(
            "SELECT symbol, display_name, kind, file_path, line, character,
                    end_line, end_character
             FROM semantic_definitions",
        );
        let mut values = Vec::<SqlValue>::new();
        if let Some(query) = query {
            sql.push_str(" WHERE instr(lower(symbol || ' ' || display_name), lower(?)) > 0");
            values.push(query.to_string().into());
        }
        sql.push_str(" ORDER BY file_path, line, character, symbol LIMIT ?");
        values.push(
            i64::try_from(limit.saturating_add(1))
                .unwrap_or(i64::MAX)
                .into(),
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values.iter()), |row| {
            Ok(SemanticDefinition {
                symbol: row.get(0)?,
                display_name: row.get(1)?,
                kind: row.get(2)?,
                file: row.get(3)?,
                line: row.get(4)?,
                character: row.get(5)?,
                end_line: row.get(6)?,
                end_character: row.get(7)?,
                provenance: "scip",
                confidence: "high",
            })
        })?;
        let mut items = rows.collect::<SqlResult<Vec<_>>>()?;
        let truncated = items.len() > limit;
        items.truncate(limit);
        Ok((items, truncated))
    }

    pub(crate) fn semantic_edges(
        &self,
        query: Option<&str>,
        relevant_paths: &[String],
        limit: usize,
    ) -> SqlResult<(Vec<SemanticEdge>, bool)> {
        if !self.semantic_tables_exist()? {
            return Ok((Vec::new(), false));
        }
        let limit = limit.clamp(1, 10_000);
        let relevant_paths = relevant_paths.iter().take(2_000).collect::<Vec<_>>();
        let mut values = Vec::<SqlValue>::new();
        let mut sql = String::new();
        if !relevant_paths.is_empty() {
            sql.push_str("WITH relevant(path) AS (VALUES ");
            for (index, path) in relevant_paths.iter().enumerate() {
                if index > 0 {
                    sql.push(',');
                }
                sql.push_str("(?)");
                values.push((*path).clone().into());
            }
            sql.push_str(") ");
        }
        sql.push_str(
            "SELECT from_symbol, from_display_name, from_file, from_line,
                    from_character, occurrence_line, occurrence_character,
                    to_symbol, to_display_name, to_file, to_line, to_character, kind
             FROM semantic_edges",
        );
        let mut predicates = Vec::new();
        if !relevant_paths.is_empty() {
            predicates.push(
                "(from_file IN (SELECT path FROM relevant)
                  AND to_file IN (SELECT path FROM relevant))",
            );
        }
        if let Some(query) = query {
            predicates.push(
                "instr(lower(
                    coalesce(from_symbol, '') || ' ' ||
                    coalesce(from_display_name, '') || ' ' ||
                    to_symbol || ' ' || to_display_name
                 ), lower(?)) > 0",
            );
            values.push(query.to_string().into());
        }
        if !predicates.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&predicates.join(" AND "));
        }
        sql.push_str(" ORDER BY from_file, from_line, from_character, to_symbol, kind LIMIT ?");
        values.push(
            i64::try_from(limit.saturating_add(1))
                .unwrap_or(i64::MAX)
                .into(),
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values.iter()), |row| {
            Ok(SemanticEdge {
                from_symbol: row.get(0)?,
                from_display_name: row.get(1)?,
                from_file: row.get(2)?,
                from_line: row.get(3)?,
                from_character: row.get(4)?,
                occurrence_line: row.get(5)?,
                occurrence_character: row.get(6)?,
                to_symbol: row.get(7)?,
                to_display_name: row.get(8)?,
                to_file: row.get(9)?,
                to_line: row.get(10)?,
                to_character: row.get(11)?,
                kind: row.get(12)?,
                provenance: "scip",
                confidence: "high",
            })
        })?;
        let mut items = rows.collect::<SqlResult<Vec<_>>>()?;
        let truncated = items.len() > limit;
        items.truncate(limit);
        Ok((items, truncated))
    }

    pub(crate) fn semantic_document_hashes(
        &self,
        paths: &[String],
    ) -> SqlResult<HashMap<String, String>> {
        if !self.semantic_tables_exist()? || paths.is_empty() {
            return Ok(HashMap::new());
        }
        let mut hashes = HashMap::new();
        for chunk in paths.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT path, content_sha256 FROM semantic_documents
                 WHERE path IN ({placeholders})"
            );
            let values = chunk
                .iter()
                .map(|path| SqlValue::from((*path).clone()))
                .collect::<Vec<_>>();
            let mut statement = self.conn.prepare(&sql)?;
            let rows = statement.query_map(rusqlite::params_from_iter(values.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (path, digest) = row?;
                hashes.insert(path, digest);
            }
        }
        Ok(hashes)
    }

    fn fact_tables_exist(&self) -> SqlResult<bool> {
        self.conn
            .query_row(
                "SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'fact_sources'",
                [],
                |_| Ok(true),
            )
            .optional()
            .map(|value| value.unwrap_or(false))
    }

    pub(crate) fn fact_source_count(&self) -> SqlResult<u32> {
        if !self.fact_tables_exist()? {
            return Ok(0);
        }
        self.conn
            .query_row("SELECT count(*) FROM fact_sources", [], |row| row.get(0))
    }

    pub(crate) fn fact_source_exists(&self, producer: &str, dataset: &str) -> SqlResult<bool> {
        if !self.fact_tables_exist()? {
            return Ok(false);
        }
        self.conn
            .query_row(
                "SELECT 1 FROM fact_sources
                 WHERE producer_name = ?1 AND dataset = ?2",
                params![producer, dataset],
                |_| Ok(true),
            )
            .optional()
            .map(|value| value.unwrap_or(false))
    }

    /// Replace exactly one producer dataset after the facts module has fully
    /// validated the manifest in memory. A failed insert rolls back to the
    /// previous known-good dataset.
    pub(crate) fn replace_fact_dataset(&self, batch: &FactImportBatch) -> SqlResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        for file in &batch.files {
            let indexed = tx
                .query_row(
                    "SELECT content_sha256 FROM files WHERE path = ?1",
                    params![file.path],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten();
            if indexed.as_deref().filter(|value| !value.is_empty()) != Some(file.sha256.as_str()) {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
        tx.execute(
            "DELETE FROM fact_sources WHERE producer_name = ?1 AND dataset = ?2",
            params![batch.source.producer_name, batch.source.dataset],
        )?;
        tx.execute(
            "INSERT INTO fact_sources(
                api_version, producer_name, producer_version, dataset,
                provenance_kind, capabilities, repository_identity, revision,
                manifest_sha256, manifest_bytes, signature_status,
                signing_key_id, signature_sha256, signature_bytes,
                signing_public_key, signature_value, signed_manifest_digest,
                imported_at, file_count,
                annotation_count, relationship_count
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
            params![
                batch.source.api_version,
                batch.source.producer_name,
                batch.source.producer_version,
                batch.source.dataset,
                batch.source.provenance_kind,
                batch.source.capabilities,
                batch.source.repository_identity,
                batch.source.revision,
                batch.source.manifest_sha256,
                i64::try_from(batch.source.manifest_bytes).unwrap_or(i64::MAX),
                batch.source.signature_status,
                batch.source.signing_key_id,
                batch.source.signature_sha256,
                batch.source
                    .signature_bytes
                    .and_then(|value| i64::try_from(value).ok()),
                batch.source.signing_public_key,
                batch.source.signature_value,
                batch.source.signed_manifest_digest,
                batch.source.imported_at,
                batch.source.file_count,
                batch.source.annotation_count,
                batch.source.relationship_count,
            ],
        )?;
        let source_id = tx.last_insert_rowid();
        {
            let mut statement = tx.prepare(
                "INSERT INTO fact_files(source_id, path, sha256, bytes)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for file in &batch.files {
                statement.execute(params![
                    source_id,
                    file.path,
                    file.sha256,
                    i64::try_from(file.bytes).unwrap_or(i64::MAX),
                ])?;
            }
        }
        {
            let mut statement = tx.prepare(
                "INSERT INTO fact_artifacts(source_id, artifact_id, path, sha256, bytes)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for artifact in &batch.artifacts {
                statement.execute(params![
                    source_id,
                    artifact.id,
                    artifact.path,
                    artifact.sha256,
                    i64::try_from(artifact.bytes).unwrap_or(i64::MAX),
                ])?;
            }
        }
        {
            let mut statement = tx.prepare(
                "INSERT INTO fact_annotations(
                    source_id, fact_id, path, line, column_no, end_line,
                    end_column, severity, category, title, message
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            for fact in &batch.annotations {
                statement.execute(params![
                    source_id,
                    fact.fact_id,
                    fact.path,
                    fact.line,
                    fact.column,
                    fact.end_line,
                    fact.end_column,
                    fact.severity,
                    fact.category,
                    fact.title,
                    fact.message,
                ])?;
            }
        }
        {
            let mut statement = tx.prepare(
                "INSERT INTO fact_relationships(
                    source_id, fact_id, relation, from_path, from_line,
                    from_column, to_path, to_line, to_column, confidence, label
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            for fact in &batch.relationships {
                statement.execute(params![
                    source_id,
                    fact.fact_id,
                    fact.relation,
                    fact.from_path,
                    fact.from_line,
                    fact.from_column,
                    fact.to_path,
                    fact.to_line,
                    fact.to_column,
                    fact.confidence,
                    fact.label,
                ])?;
            }
        }
        tx.commit()
    }

    pub(crate) fn fact_sources(&self, limit: usize) -> SqlResult<Vec<FactSourceRecord>> {
        if !self.fact_tables_exist()? {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, 1_000);
        let signature_columns = self
            .conn
            .prepare("SELECT signature_status FROM fact_sources LIMIT 0")
            .is_ok();
        let sql = if signature_columns {
            "SELECT id, api_version, producer_name, producer_version, dataset,
                    provenance_kind, capabilities, repository_identity, revision,
                    manifest_sha256, manifest_bytes, signature_status,
                    signing_key_id, signature_sha256, signature_bytes,
                    signing_public_key, signature_value, signed_manifest_digest,
                    imported_at, file_count,
                    annotation_count, relationship_count
             FROM fact_sources
             ORDER BY producer_name, dataset
             LIMIT ?1"
        } else {
            "SELECT id, api_version, producer_name, producer_version, dataset,
                    provenance_kind, capabilities, repository_identity, revision,
                    manifest_sha256, manifest_bytes, 'unsigned', NULL, NULL, NULL,
                    NULL, NULL, NULL, imported_at, file_count, annotation_count, relationship_count
             FROM fact_sources
             ORDER BY producer_name, dataset
             LIMIT ?1"
        };
        let mut statement = self.conn.prepare(sql)?;
        let rows = statement.query_map(params![limit as i64], |row| {
            Ok(FactSourceRecord {
                id: row.get(0)?,
                api_version: row.get(1)?,
                producer_name: row.get(2)?,
                producer_version: row.get(3)?,
                dataset: row.get(4)?,
                provenance_kind: row.get(5)?,
                capabilities: row.get(6)?,
                repository_identity: row.get(7)?,
                revision: row.get(8)?,
                manifest_sha256: row.get(9)?,
                manifest_bytes: row_u64(row, 10)?,
                signature_status: row.get(11)?,
                signing_key_id: row.get(12)?,
                signature_sha256: row.get(13)?,
                signature_bytes: row
                    .get::<_, Option<i64>>(14)?
                    .and_then(|value| value.try_into().ok()),
                signing_public_key: row.get(15)?,
                signature_value: row.get(16)?,
                signed_manifest_digest: row.get(17)?,
                imported_at: row.get(18)?,
                file_count: row.get(19)?,
                annotation_count: row.get(20)?,
                relationship_count: row.get(21)?,
            })
        })?;
        rows.collect()
    }

    pub(crate) fn fact_files(
        &self,
        source_id: i64,
        limit: usize,
    ) -> SqlResult<Vec<FactFileRecord>> {
        if !self.fact_tables_exist()? {
            return Ok(Vec::new());
        }
        let mut statement = self.conn.prepare(
            "SELECT path, sha256, bytes FROM fact_files
             WHERE source_id = ?1 ORDER BY path LIMIT ?2",
        )?;
        let rows =
            statement.query_map(params![source_id, limit.clamp(1, 20_000) as i64], |row| {
                Ok(FactFileRecord {
                    path: row.get(0)?,
                    sha256: row.get(1)?,
                    bytes: row_u64(row, 2)?,
                })
            })?;
        rows.collect()
    }

    pub(crate) fn fact_artifacts(
        &self,
        source_ids: &[i64],
        limit: usize,
    ) -> SqlResult<(Vec<FactArtifact>, bool)> {
        if !self.fact_tables_exist()? || source_ids.is_empty() {
            return Ok((Vec::new(), false));
        }
        let limit = limit.clamp(1, 1_000);
        let mut values = Vec::<SqlValue>::new();
        let predicate = Self::fact_source_filter_sql(source_ids, &mut values);
        let sql = format!(
            "SELECT fs.producer_name, fs.dataset, fa.artifact_id, fa.path,
                    fa.sha256, fa.bytes
             FROM fact_artifacts fa
             JOIN fact_sources fs ON fs.id = fa.source_id
             WHERE {predicate}
             ORDER BY fs.producer_name, fs.dataset, fa.artifact_id
             LIMIT ?"
        );
        values.push((limit.saturating_add(1) as i64).into());
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values.iter()), |row| {
            let producer: String = row.get(0)?;
            let dataset: String = row.get(1)?;
            Ok(FactArtifact {
                source_id: source_public_id(&producer, &dataset),
                id: row.get(2)?,
                path: row.get(3)?,
                sha256: row.get(4)?,
                bytes: row_u64(row, 5)?,
            })
        })?;
        let mut items = rows.collect::<SqlResult<Vec<_>>>()?;
        let truncated = items.len() > limit;
        items.truncate(limit);
        Ok((items, truncated))
    }

    fn fact_source_filter_sql(source_ids: &[i64], values: &mut Vec<SqlValue>) -> String {
        let placeholders = std::iter::repeat_n("?", source_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        values.extend(source_ids.iter().copied().map(SqlValue::Integer));
        format!("source_id IN ({placeholders})")
    }

    fn fact_path_filter_sql(
        filter: &FactQueryFilter,
        path_columns: &[&str],
        values: &mut Vec<SqlValue>,
    ) -> Option<String> {
        match filter {
            FactQueryFilter::Scope(scope) if scope == "." => None,
            FactQueryFilter::Scope(scope) => {
                let predicates = path_columns
                    .iter()
                    .map(|column| {
                        values.push(scope.clone().into());
                        values.push(scope.clone().into());
                        values.push(scope.clone().into());
                        format!("({column} = ? OR substr({column}, 1, length(?) + 1) = ? || '/')")
                    })
                    .collect::<Vec<_>>();
                Some(format!("({})", predicates.join(" OR ")))
            }
            FactQueryFilter::Paths(paths) if paths.is_empty() => Some("0 = 1".into()),
            FactQueryFilter::Paths(paths) => {
                let placeholders = std::iter::repeat_n("?", paths.len())
                    .collect::<Vec<_>>()
                    .join(",");
                let predicates = path_columns
                    .iter()
                    .map(|column| {
                        values.extend(paths.iter().cloned().map(SqlValue::Text));
                        format!("{column} IN ({placeholders})")
                    })
                    .collect::<Vec<_>>();
                let joiner = if path_columns.len() > 1 {
                    " AND "
                } else {
                    " OR "
                };
                Some(format!("({})", predicates.join(joiner)))
            }
        }
    }

    pub(crate) fn fact_annotations(
        &self,
        source_ids: &[i64],
        filter: &FactQueryFilter,
        limit: usize,
    ) -> SqlResult<(Vec<FactAnnotation>, bool)> {
        if !self.fact_tables_exist()? || source_ids.is_empty() {
            return Ok((Vec::new(), false));
        }
        let limit = limit.clamp(1, 10_000);
        let mut values = Vec::<SqlValue>::new();
        let mut predicates = vec![Self::fact_source_filter_sql(source_ids, &mut values)];
        if let Some(predicate) = Self::fact_path_filter_sql(filter, &["fa.path"], &mut values) {
            predicates.push(predicate);
        }
        let sql = format!(
            "SELECT fs.producer_name, fs.dataset, fa.fact_id, fa.path, fa.line,
                    fa.column_no, fa.end_line, fa.end_column, fa.severity,
                    fa.category, fa.title, fa.message
             FROM fact_annotations fa
             JOIN fact_sources fs ON fs.id = fa.source_id
             WHERE {}
             ORDER BY fa.path, fa.line, fa.fact_id, fs.producer_name, fs.dataset
             LIMIT ?",
            predicates.join(" AND ")
        );
        values.push((limit.saturating_add(1) as i64).into());
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values.iter()), |row| {
            let producer: String = row.get(0)?;
            let dataset: String = row.get(1)?;
            Ok(FactAnnotation {
                source_id: source_public_id(&producer, &dataset),
                fact_id: row.get(2)?,
                path: row.get(3)?,
                line: row.get(4)?,
                column: row.get(5)?,
                end_line: row.get(6)?,
                end_column: row.get(7)?,
                severity: row.get(8)?,
                category: row.get(9)?,
                title: row.get(10)?,
                message: row.get(11)?,
            })
        })?;
        let mut items = rows.collect::<SqlResult<Vec<_>>>()?;
        let truncated = items.len() > limit;
        items.truncate(limit);
        Ok((items, truncated))
    }

    pub(crate) fn fact_relationships(
        &self,
        source_ids: &[i64],
        filter: &FactQueryFilter,
        limit: usize,
    ) -> SqlResult<(Vec<FactRelationship>, bool)> {
        if !self.fact_tables_exist()? || source_ids.is_empty() {
            return Ok((Vec::new(), false));
        }
        let limit = limit.clamp(1, 10_000);
        let mut values = Vec::<SqlValue>::new();
        let mut predicates = vec![Self::fact_source_filter_sql(source_ids, &mut values)];
        if let Some(predicate) =
            Self::fact_path_filter_sql(filter, &["fr.from_path", "fr.to_path"], &mut values)
        {
            predicates.push(predicate);
        }
        let sql = format!(
            "SELECT fs.producer_name, fs.dataset, fr.fact_id, fr.relation,
                    fr.from_path, fr.from_line, fr.from_column, fr.to_path,
                    fr.to_line, fr.to_column, fr.confidence, fr.label
             FROM fact_relationships fr
             JOIN fact_sources fs ON fs.id = fr.source_id
             WHERE {}
             ORDER BY fr.from_path, fr.from_line, fr.to_path, fr.to_line,
                      fr.fact_id, fs.producer_name, fs.dataset
             LIMIT ?",
            predicates.join(" AND ")
        );
        values.push((limit.saturating_add(1) as i64).into());
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values.iter()), |row| {
            let producer: String = row.get(0)?;
            let dataset: String = row.get(1)?;
            Ok(FactRelationship {
                source_id: source_public_id(&producer, &dataset),
                fact_id: row.get(2)?,
                relation: row.get(3)?,
                from_path: row.get(4)?,
                from_line: row.get(5)?,
                from_column: row.get(6)?,
                to_path: row.get(7)?,
                to_line: row.get(8)?,
                to_column: row.get(9)?,
                confidence: row.get(10)?,
                label: row.get(11)?,
            })
        })?;
        let mut items = rows.collect::<SqlResult<Vec<_>>>()?;
        let truncated = items.len() > limit;
        items.truncate(limit);
        Ok((items, truncated))
    }

    /// Wipe everything related to a single file before re-indexing it.
    /// Foreign keys CASCADE delete edges + child symbols.
    pub fn purge_file(&self, file_path: &str) -> SqlResult<()> {
        let result = (|| {
            let tx = self.conn.unchecked_transaction()?;
            let restore_contract = Self::incremental_concept_contract_current_on(&tx)?;
            tx.execute(
                "DELETE FROM symbols WHERE file_path = ?1",
                params![file_path],
            )?;
            tx.execute("DELETE FROM files WHERE path = ?1", params![file_path])?;
            if restore_contract && Self::concept_schema_objects_current_on(&tx)? {
                update_concept_documentation_meta_on(&tx)?;
            }
            if restore_contract {
                Self::set_concept_contract_current(&tx)?;
            }
            tx.commit()
        })();
        if result.is_err() {
            // A failing delete trigger rolls its dirty-marker write back with the
            // file transaction. Persist the marker separately so a later file
            // commit cannot make an unpurged path queryable as current.
            let _ = self.mark_concept_contract_dirty();
        }
        result
    }

    /// Commit a parsed file's symbols and edges in a single transaction.
    /// Hot path during indexing — keep it batched.
    pub fn commit_file(&mut self, pending: PendingFile) -> SqlResult<()> {
        let corpus = if concept_documentation_language_supported(&pending.language) {
            None
        } else {
            Some(PendingConceptCorpus::default())
        };
        let result = self.commit_file_inner(pending, corpus);
        if result.is_err() {
            let _ = self.mark_concept_contract_dirty();
        }
        result
    }

    pub(crate) fn commit_file_with_concepts(
        &mut self,
        pending: PendingFile,
        corpus: PendingConceptCorpus,
    ) -> SqlResult<()> {
        if corpus.language_supported != concept_documentation_language_supported(&pending.language)
        {
            return Err(rusqlite::Error::InvalidParameterName(
                "concept documentation language support mismatch".to_string(),
            ));
        }
        let result = self.commit_file_inner(pending, Some(corpus));
        if result.is_err() {
            let _ = self.mark_concept_contract_dirty();
        }
        result
    }

    fn commit_file_inner(
        &mut self,
        pending: PendingFile,
        corpus: Option<PendingConceptCorpus>,
    ) -> SqlResult<()> {
        let mut documentation_by_symbol = vec![None; pending.symbols.len()];
        if let Some(corpus) = corpus.as_ref() {
            for document in &corpus.documents {
                if document.symbol_index >= documentation_by_symbol.len()
                    || documentation_by_symbol[document.symbol_index].is_some()
                    || !valid_documentation_search(&document.documentation_search)
                {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "invalid normalized concept documentation".to_string(),
                    ));
                }
                documentation_by_symbol[document.symbol_index] =
                    Some(document.documentation_search.as_str());
            }
        }

        let tx = self.conn.transaction()?;
        let restore_contract = Self::incremental_concept_contract_current_on(&tx)?;

        // Purge any existing data for this file.
        tx.execute(
            "DELETE FROM symbols WHERE file_path = ?1",
            params![&pending.path],
        )?;
        tx.execute("DELETE FROM files WHERE path = ?1", params![&pending.path])?;

        // Insert symbols, remembering each rowid.
        let language = if pending.language.is_empty() {
            None
        } else {
            Some(pending.language.as_str())
        };
        let production = is_production_path(&pending.path);
        let mut symbol_ids: Vec<i64> = Vec::with_capacity(pending.symbols.len());
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO symbols(name, kind, file_path, line_start, line_end, signature, parent_id, language, decorators, production)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for (symbol_index, s) in pending.symbols.iter().enumerate() {
                let parent_id = s.parent_index.map(|i| symbol_ids[i]);
                stmt.execute(params![
                    s.name,
                    s.kind,
                    &pending.path,
                    s.line_start,
                    s.line_end,
                    s.signature,
                    parent_id,
                    language,
                    s.decorators,
                    production
                ])?;
                let symbol_id = tx.last_insert_rowid();
                insert_concept_document(
                    &tx,
                    symbol_id,
                    &s.name,
                    &pending.path,
                    s.signature.as_deref(),
                    documentation_by_symbol[symbol_index].unwrap_or_default(),
                )?;
                symbol_ids.push(symbol_id);
            }
        }

        // Insert edges (to_id left NULL — resolved by name/type during queries).
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO edges(from_id, to_id, to_name, to_path, to_type, kind, line)
                 VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for e in &pending.edges {
                let from_id = symbol_ids[e.from_index];
                stmt.execute(params![
                    from_id, e.to_name, e.to_path, e.to_type, e.kind, e.line
                ])?;
            }
        }

        // Stamp the file.
        let fingerprint = crate::fingerprint::compute_structural_fingerprint(&pending);
        tx.execute(
            "INSERT INTO files(path, indexed_at, symbol_count, structural_fingerprint, content_sha256, production) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &pending.path,
                pending.mtime,
                pending.symbols.len() as u32,
                &fingerprint,
                &pending.content_sha256,
                production
            ],
        )?;

        if let Some(corpus) = corpus.as_ref() {
            tx.execute(
                "INSERT INTO concept_documentation_file_stats(
                     path, language_supported, indexed_documents,
                     secret_omitted, size_omitted
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    &pending.path,
                    i64::from(corpus.language_supported),
                    corpus.documents.len() as u32,
                    corpus.secret_omitted,
                    corpus.size_omitted
                ],
            )?;
            if restore_contract {
                update_concept_documentation_meta_on(&tx)?;
            }
        }

        if restore_contract && corpus.is_some() {
            Self::set_concept_contract_current(&tx)?;
        }
        tx.commit()?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_symbol(
        &self,
        name: &str,
        kind: &str,
        file_path: &str,
        line_start: u32,
        line_end: u32,
        signature: Option<&str>,
        parent_id: Option<i64>,
    ) -> SqlResult<i64> {
        let sequence = INSERT_SYMBOL_SAVEPOINT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let savepoint = format!("mmcg_insert_symbol_{sequence}");
        self.conn.execute_batch(&format!("SAVEPOINT {savepoint}"))?;
        let result = (|| {
            let restore_contract = Self::incremental_concept_contract_current_on(&self.conn)?;
            self.conn.execute(
                "INSERT INTO symbols(name, kind, file_path, line_start, line_end, signature, parent_id, production)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    name,
                    kind,
                    file_path,
                    line_start,
                    line_end,
                    signature,
                    parent_id,
                    is_production_path(file_path)
                ],
            )?;
            let symbol_id = self.conn.last_insert_rowid();
            insert_concept_document(&self.conn, symbol_id, name, file_path, signature, "")?;
            if restore_contract {
                Self::set_concept_contract_current(&self.conn)?;
            }
            Ok(symbol_id)
        })();
        match result {
            Ok(symbol_id) => {
                if let Err(error) = self.conn.execute_batch(&format!("RELEASE {savepoint}")) {
                    let _ = self
                        .conn
                        .execute_batch(&format!("ROLLBACK TO {savepoint}; RELEASE {savepoint}"));
                    return Err(error);
                }
                Ok(symbol_id)
            }
            Err(error) => {
                let _ = self
                    .conn
                    .execute_batch(&format!("ROLLBACK TO {savepoint}; RELEASE {savepoint}"));
                Err(error)
            }
        }
    }

    pub fn insert_edge(
        &self,
        from_id: i64,
        to_id: Option<i64>,
        to_name: &str,
        kind: &str,
        line: u32,
    ) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO edges(from_id, to_id, to_name, to_path, to_type, kind, line) VALUES (?1, ?2, ?3, NULL, NULL, ?4, ?5)",
            params![from_id, to_id, to_name, kind, line],
        )?;
        Ok(())
    }

    /// Same as [`Store::insert_edge`] but with an explicit `to_type` —
    /// `insert_edge` always inserts `to_type = NULL`, so tests that need a
    /// `Type::method()`-shaped edge (e.g. the equivalence property tests)
    /// need this instead of hand-building a `PendingFile`.
    #[cfg(test)]
    pub fn insert_edge_with_type(
        &self,
        from_id: i64,
        to_name: &str,
        to_type: Option<&str>,
        kind: &str,
        line: u32,
    ) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO edges(from_id, to_id, to_name, to_path, to_type, kind, line) VALUES (?1, NULL, ?2, NULL, ?3, ?4, ?5)",
            params![from_id, to_name, to_type, kind, line],
        )?;
        Ok(())
    }

    pub fn upsert_file(&self, path: &str, mtime: i64, symbol_count: u32) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO files(path, indexed_at, symbol_count, production) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET indexed_at=?2, symbol_count=?3, production=?4",
            params![path, mtime, symbol_count, is_production_path(path)],
        )?;
        Ok(())
    }

    pub fn file_mtime(&self, path: &str) -> SqlResult<Option<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT indexed_at FROM files WHERE path = ?1")?;
        let mut rows = stmt.query(params![path])?;
        rows.next()?.map(|r| r.get(0)).transpose()
    }

    pub(crate) fn file_mtimes_bounded(&self, cap: usize) -> SqlResult<Option<Vec<(String, i64)>>> {
        let fetch = cap.saturating_add(1).min(i64::MAX as usize) as i64;
        let mut statement = self
            .conn
            .prepare("SELECT path, indexed_at FROM files ORDER BY path LIMIT ?1")?;
        let rows = statement.query_map([fetch], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut entries = rows.collect::<SqlResult<Vec<_>>>()?;
        if entries.len() > cap {
            return Ok(None);
        }
        entries.shrink_to_fit();
        Ok(Some(entries))
    }

    /// Stored structural fingerprint for a file path, or `None` if never indexed.
    /// Files indexed before 0.28 return `Some("")` (column backfilled with `''`);
    /// callers should treat that as `first-seen`.
    pub fn file_fingerprint(&self, path: &str) -> SqlResult<Option<String>> {
        self.conn
            .query_row(
                "SELECT structural_fingerprint FROM files WHERE path = ?1",
                params![path],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn file_content_sha256(&self, path: &str) -> SqlResult<Option<String>> {
        self.conn
            .query_row(
                "SELECT content_sha256 FROM files WHERE path = ?1",
                params![path],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn meta_value(&self, key: &str) -> SqlResult<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn extractor_contract_current(&self) -> SqlResult<bool> {
        Ok(self
            .meta_value(crate::indexer::EXTRACTOR_CONTRACT_META_KEY)?
            .as_deref()
            == Some(crate::indexer::EXTRACTOR_CONTRACT_VERSION))
    }

    pub fn concept_contract_current(&self) -> SqlResult<bool> {
        Ok(self.concept_schema_objects_current()?
            && self.meta_value(CONCEPT_NORMALIZATION_META_KEY)?.as_deref()
                == Some(CONCEPT_NORMALIZATION_VERSION))
    }

    fn incremental_concept_contract_current_on(connection: &Connection) -> SqlResult<bool> {
        if !Self::concept_schema_objects_current_on(connection)? {
            return Ok(false);
        }
        let extractor_contract = connection
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                [crate::indexer::EXTRACTOR_CONTRACT_META_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let concept_contract = connection
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                [CONCEPT_NORMALIZATION_META_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(
            extractor_contract.as_deref() == Some(crate::indexer::EXTRACTOR_CONTRACT_VERSION)
                && concept_contract.as_deref() == Some(CONCEPT_NORMALIZATION_VERSION),
        )
    }

    pub(crate) fn mark_concept_contract_dirty(&self) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![CONCEPT_NORMALIZATION_META_KEY, CONCEPT_CONTRACT_DIRTY],
        )?;
        Ok(())
    }

    fn set_concept_contract_current(connection: &Connection) -> SqlResult<()> {
        connection.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![
                CONCEPT_NORMALIZATION_META_KEY,
                CONCEPT_NORMALIZATION_VERSION
            ],
        )?;
        Ok(())
    }

    pub(crate) fn finalize_index_contracts_current(&self) -> SqlResult<ConceptFinalizeStats> {
        self.ensure_concept_schema()?;
        let tx = self.conn.unchecked_transaction()?;
        let orphans_purged = tx.execute(
            "DELETE FROM symbol_concepts
             WHERE NOT EXISTS (
                 SELECT 1 FROM symbols WHERE symbols.id = symbol_concepts.symbol_id
             )",
            [],
        )?;
        let missing: i64 = tx.query_row(
            "SELECT COUNT(*)
             FROM symbols s
             LEFT JOIN symbol_concepts c ON c.symbol_id = s.id
             WHERE c.symbol_id IS NULL",
            [],
            |row| row.get(0),
        )?;
        if missing != 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                Some(format!(
                    "concept finalization found {missing} symbols without concept rows"
                )),
            ));
        }
        let missing_documentation_stats: i64 = tx.query_row(
            "SELECT COUNT(*)
             FROM files f
             LEFT JOIN concept_documentation_file_stats d ON d.path = f.path
             WHERE d.path IS NULL",
            [],
            |row| row.get(0),
        )?;
        let mismatched_documentation_counts: i64 = tx.query_row(
            "SELECT COUNT(*)
             FROM concept_documentation_file_stats d
             WHERE d.indexed_documents <> (
                 SELECT COUNT(*)
                 FROM symbols s
                 JOIN symbol_concepts c ON c.symbol_id = s.id
                 WHERE s.file_path = d.path AND c.documentation_search <> ''
             )",
            [],
            |row| row.get(0),
        )?;
        if missing_documentation_stats != 0 || mismatched_documentation_counts != 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                Some(format!(
                    "concept finalization found {missing_documentation_stats} files without documentation stats and {mismatched_documentation_counts} mismatched documentation counts"
                )),
            ));
        }
        tx.execute(
            "INSERT INTO symbol_concepts_fts(symbol_concepts_fts) VALUES ('rebuild')",
            [],
        )?;
        tx.execute(
            "INSERT INTO symbol_concepts_fts(symbol_concepts_fts, rank)
             VALUES ('integrity-check', 1)",
            [],
        )?;
        update_concept_documentation_meta_on(&tx)?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![
                crate::indexer::EXTRACTOR_CONTRACT_META_KEY,
                crate::indexer::EXTRACTOR_CONTRACT_VERSION
            ],
        )?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![
                CONCEPT_NORMALIZATION_META_KEY,
                CONCEPT_NORMALIZATION_VERSION
            ],
        )?;
        let rows: u32 =
            tx.query_row("SELECT COUNT(*) FROM symbol_concepts", [], |row| row.get(0))?;
        tx.commit()?;
        Ok(ConceptFinalizeStats {
            rows,
            orphans_purged: u32::try_from(orphans_purged).unwrap_or(u32::MAX),
        })
    }

    pub fn concept_count(&self) -> SqlResult<u32> {
        self.conn
            .query_row("SELECT COUNT(*) FROM symbol_concepts", [], |row| row.get(0))
    }

    pub(crate) fn concept_documentation_stats(&self) -> SqlResult<ConceptDocumentationStats> {
        concept_documentation_stats_on(&self.conn)
    }

    pub(crate) fn purge_orphan_concepts(&self) -> SqlResult<u32> {
        let changed = self.conn.execute(
            "DELETE FROM symbol_concepts
             WHERE NOT EXISTS (
                 SELECT 1 FROM symbols WHERE symbols.id = symbol_concepts.symbol_id
             )",
            [],
        )?;
        Ok(u32::try_from(changed).unwrap_or(u32::MAX))
    }

    pub fn data_version(&self) -> SqlResult<u64> {
        let value: i64 = self
            .conn
            .query_row("PRAGMA data_version", [], |row| row.get(0))?;
        u64::try_from(value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })
    }

    /// Metadata token for the canonical source database and active WAL, used
    /// by long read-only analyses to reject a result assembled while an
    /// external watcher advanced the index.
    pub(crate) fn source_index_state(&self) -> SqlResult<IndexFileState> {
        index_file_state(&self.db_path)
    }

    pub fn begin_read_snapshot(&self) -> SqlResult<()> {
        self.conn
            .execute_batch("BEGIN DEFERRED; SELECT 1 FROM meta LIMIT 1")
    }

    pub fn end_read_snapshot(&self) -> SqlResult<()> {
        self.conn.execute_batch("ROLLBACK")
    }

    /// All paths currently in the index. The indexer uses this to detect
    /// deletions — paths no longer on disk get purged at the end of an index run.
    pub fn indexed_paths(&self) -> SqlResult<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT path FROM files ORDER BY path")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect()
    }

    pub(crate) fn indexed_paths_bounded(&self, limit: usize) -> SqlResult<(Vec<String>, bool)> {
        let requested = limit.saturating_add(1).min(1_000_001);
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM files ORDER BY path LIMIT ?1")?;
        let rows = stmt.query_map(params![requested as i64], |row| row.get::<_, String>(0))?;
        let mut paths = rows.collect::<SqlResult<Vec<_>>>()?;
        let truncated = paths.len() > limit;
        paths.truncate(limit);
        Ok((paths, truncated))
    }

    /// Find symbols whose name matches exactly. Optional `kind` and `language` filters.
    pub fn search_symbols(
        &self,
        name: &str,
        kind: Option<&str>,
        language: Option<&str>,
    ) -> SqlResult<Vec<Symbol>> {
        let sql = format!(
            "SELECT {SYMBOL_COLS}
             FROM symbols
             WHERE name = ?1
               AND (?2 IS NULL OR kind = ?2)
               AND (?3 IS NULL OR language = ?3)
             ORDER BY file_path, line_start"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![name, kind, language], Self::row_to_symbol)?;
        rows.collect()
    }

    /// Fully-qualified namespace ancestor chain for a symbol. Extractors record
    /// nested namespaces as ordinary parent symbols, so `AppA.Common` must not
    /// collapse with `AppB.Common` merely because the nearest node is named
    /// `Common` in both files.
    pub fn enclosing_namespace(&self, symbol_id: i64) -> SqlResult<Option<String>> {
        self.conn.query_row(
            "WITH RECURSIVE ancestors(id, name, kind, parent_id, depth) AS (
                     SELECT id, name, kind, parent_id, 0
                     FROM symbols
                     WHERE id = ?1
                   UNION ALL
                     SELECT parent.id, parent.name, parent.kind, parent.parent_id,
                            ancestors.depth + 1
                     FROM ancestors
                     JOIN symbols parent ON parent.id = ancestors.parent_id
                 )
                 SELECT group_concat(name, '.')
                 FROM (
                     SELECT name
                     FROM ancestors
                     WHERE kind = 'namespace'
                     ORDER BY depth DESC
                 )",
            params![symbol_id],
            |row| row.get(0),
        )
    }

    /// Callers of a symbol — symbols joined to it via an edge matching `to_name`
    /// OR `to_type`. The `to_type` match catches Rust constructor /
    /// associated-function calls like `SessionStore::new()` that would otherwise
    /// hide under the leaf name (`new`). Optional `language` filter (defends
    /// against cross-language name collisions in monorepos).
    ///
    /// `edge_kind`:
    ///   - `None` → `'calls'` (historical "who calls X")
    ///   - `Some("imports")` → who imports X (returns module pseudo-symbols)
    ///   - `Some("inherits")` → who inherits from X (when extractors emit inherit edges)
    pub fn callers_of(
        &self,
        name: &str,
        language: Option<&str>,
        edge_kind: Option<&str>,
    ) -> SqlResult<Vec<Symbol>> {
        let sql = format!(
            "SELECT DISTINCT {SYMBOL_COLS_S}
             FROM symbols s
             JOIN edges e ON e.from_id = s.id
             WHERE e.kind = COALESCE(?3, 'calls')
               AND (e.to_name = ?1 OR e.to_type = ?1)
               AND (?2 IS NULL OR s.language = ?2)
             ORDER BY s.file_path, s.line_start"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![name, language, edge_kind], Self::row_to_symbol)?;
        rows.collect()
    }

    /// Callees of a symbol-id — names it references via the given edge kind.
    /// `edge_kind = None` defaults to `'calls'`.
    pub fn callees_of(
        &self,
        symbol_id: i64,
        edge_kind: Option<&str>,
    ) -> SqlResult<Vec<(String, u32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT to_name, line FROM edges
             WHERE from_id = ?1 AND kind = COALESCE(?2, 'calls')
             ORDER BY line",
        )?;
        let rows = stmt.query_map(params![symbol_id, edge_kind], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?))
        })?;
        rows.collect()
    }

    /// Transitive callers up to `max_depth` for one or more seed names — the
    /// guarded visited-set walk backing both `mmcg_impact` (single seed) and
    /// `change_impact` (many seeds). Matches `to_name OR to_type` to catch
    /// type-method calls like `SessionStore::new()`. Bounded on three axes:
    /// seed count (≤ 200), `max_depth` (1..=10 — mirrors the `mmcg_impact`
    /// tool's advertised cap), and `row_limit` (≤ 5001, the caller's row cap).
    /// Optional `language` restricts every step of the walk (not just the
    /// final rows) to that language, matching `mmcg_impact`'s documented
    /// filter — `change_impact` always passes `None`. Additionally wrapped in
    /// its own tight `with_work_budget` (2s / 250k ticks) so a dense
    /// name-collision graph can't run away even under a generous outer guard
    /// — nested budgets compose by min, so an outer guard installed by the
    /// MCP/CLI dispatch boundary only ever *tightens* this, never loosens it.
    pub fn impact_of_many(
        &self,
        names: &[String],
        max_depth: u32,
        row_limit: usize,
        language: Option<&str>,
    ) -> SqlResult<Vec<SeedImpact>> {
        if names.is_empty() || names.len() > 200 {
            return Err(rusqlite::Error::InvalidParameterName(
                "seed_count".to_string(),
            ));
        }
        if !(1..=10).contains(&max_depth) {
            return Err(rusqlite::Error::InvalidParameterName(
                "max_depth".to_string(),
            ));
        }
        if !(1..=5001).contains(&row_limit) {
            return Err(rusqlite::Error::InvalidParameterName(
                "row_limit".to_string(),
            ));
        }

        let placeholders = (1..=names.len())
            .map(|index| format!("(?{index})"))
            .collect::<Vec<_>>()
            .join(",");
        let depth_param = names.len() + 1;
        let limit_param = names.len() + 2;
        let lang_param = names.len() + 3;
        let sql = format!(
            "WITH RECURSIVE seed(seed) AS (VALUES {placeholders}),
             walk(seed, sym_id, name, depth, visited) AS (
                 SELECT seed.seed, s.id, s.name, 1, ',' || s.id || ','
                 FROM seed
                 JOIN edges e INDEXED BY idx_edges_calls_to_name
                   ON e.to_name = seed.seed AND e.kind = 'calls'
                 JOIN symbols s ON s.id = e.from_id
                 WHERE (?{lang_param} IS NULL OR s.language = ?{lang_param})
               UNION ALL
                 SELECT seed.seed, s.id, s.name, 1, ',' || s.id || ','
                 FROM seed
                 JOIN edges e INDEXED BY idx_edges_calls_to_type
                   ON e.to_type = seed.seed
                  AND e.kind = 'calls'
                  AND e.to_type IS NOT NULL
                  AND e.to_type <> ''
                 JOIN symbols s ON s.id = e.from_id
                 WHERE (?{lang_param} IS NULL OR s.language = ?{lang_param})
               UNION ALL
                 SELECT walk.seed, s.id, s.name, walk.depth + 1,
                        walk.visited || s.id || ','
                 FROM walk
                 JOIN edges e INDEXED BY idx_edges_calls_to_name
                   ON e.to_name = walk.name AND e.kind = 'calls'
                 JOIN symbols s ON s.id = e.from_id
                 WHERE walk.depth < ?{depth_param}
                   AND instr(walk.visited, ',' || s.id || ',') = 0
                   AND (?{lang_param} IS NULL OR s.language = ?{lang_param})
               UNION ALL
                 SELECT walk.seed, s.id, s.name, walk.depth + 1,
                        walk.visited || s.id || ','
                 FROM walk
                 JOIN edges e INDEXED BY idx_edges_calls_to_type
                   ON e.to_type = walk.name
                  AND e.kind = 'calls'
                  AND e.to_type IS NOT NULL
                  AND e.to_type <> ''
                 JOIN symbols s ON s.id = e.from_id
                 WHERE walk.depth < ?{depth_param}
                   AND instr(walk.visited, ',' || s.id || ',') = 0
                   AND (?{lang_param} IS NULL OR s.language = ?{lang_param})
             ), minimum AS (
                 SELECT seed, sym_id, MIN(depth) AS depth
                 FROM walk
                 GROUP BY seed, sym_id
             )
             SELECT minimum.seed, {SYMBOL_COLS_S}, minimum.depth
             FROM minimum
             JOIN symbols s ON s.id = minimum.sym_id
             ORDER BY minimum.depth, s.file_path, s.line_start, minimum.seed,
                      s.name, s.kind, s.id
             LIMIT ?{limit_param}"
        );
        let mut values: Vec<SqlValue> = names.iter().cloned().map(SqlValue::Text).collect();
        values.push(SqlValue::Integer(max_depth as i64));
        values.push(SqlValue::Integer(row_limit as i64));
        values.push(match language {
            Some(lang) => SqlValue::Text(lang.to_string()),
            None => SqlValue::Null,
        });

        let budget = impact_precision_budget();
        self.with_local_work_budget(budget, || {
            let mut statement = self.conn.prepare(&sql)?;
            let rows = statement.query_map(rusqlite::params_from_iter(values), |row| {
                Ok(SeedImpact {
                    seed: row.get(0)?,
                    symbol: Symbol {
                        id: row.get(1)?,
                        name: row.get(2)?,
                        kind: row.get(3)?,
                        file_path: row.get(4)?,
                        line_start: row.get(5)?,
                        line_end: row.get(6)?,
                        signature: row.get(7)?,
                        parent_id: row.get(8)?,
                        decorators: row.get(9)?,
                    },
                    depth: row.get(10)?,
                })
            })?;
            rows.collect()
        })
    }

    pub fn scoped_paths_in_components(
        &self,
        components: &[String],
        row_limit: usize,
    ) -> SqlResult<Vec<String>> {
        if components.is_empty() || row_limit == 0 || row_limit > 50_001 {
            return Ok(Vec::new());
        }
        let placeholders = (1..=components.len())
            .map(|index| format!("(?{index})"))
            .collect::<Vec<_>>()
            .join(",");
        let limit_param = components.len() + 1;
        let sql = format!(
            "WITH component(path) AS (VALUES {placeholders})
             SELECT f.path
             FROM files f
             WHERE EXISTS (
                 SELECT 1 FROM component c
                 WHERE (c.path = '.' AND instr(f.path, '/') = 0)
                    OR (c.path != '.' AND (
                        f.path = c.path OR
                        substr(f.path, 1, length(c.path) + 1) = c.path || '/'
                    ))
             )
             ORDER BY f.path
             LIMIT ?{limit_param}"
        );
        let mut values: Vec<SqlValue> = components.iter().cloned().map(SqlValue::Text).collect();
        values.push(SqlValue::Integer(row_limit as i64));
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values), |row| row.get(0))?;
        rows.collect()
    }

    pub fn test_symbols_in_components(
        &self,
        components: &[String],
        row_limit: usize,
    ) -> SqlResult<Vec<Symbol>> {
        if components.is_empty() || row_limit == 0 || row_limit > 501 {
            return Ok(Vec::new());
        }
        let placeholders = (1..=components.len())
            .map(|index| format!("(?{index})"))
            .collect::<Vec<_>>()
            .join(",");
        let limit_param = components.len() + 1;
        let sql = format!(
            "WITH component(path) AS (VALUES {placeholders})
             SELECT DISTINCT {SYMBOL_COLS_S}
             FROM symbols s
             WHERE s.kind != 'module'
               AND EXISTS (
                   SELECT 1 FROM component c
                   WHERE (c.path = '.' AND instr(s.file_path, '/') = 0)
                      OR (c.path != '.' AND (
                          s.file_path = c.path OR
                          substr(s.file_path, 1, length(c.path) + 1) = c.path || '/'
                      ))
               )
               AND (
                   lower(s.file_path) LIKE 'test_%'
                   OR lower(s.file_path) LIKE '%/test_%'
                   OR lower(s.file_path) LIKE '%/tests/%'
                   OR lower(s.file_path) LIKE '%/test/%'
                   OR lower(s.file_path) LIKE '%/spec/%'
                   OR lower(s.file_path) LIKE '%.test.%'
                   OR lower(s.file_path) LIKE '%.spec.%'
                   OR lower(s.file_path) LIKE '%_test.rs'
                   OR lower(s.file_path) LIKE '%tests.rs'
               )
               AND (
                   lower(s.name) LIKE 'test%'
                   OR lower(s.name) IN ('it', 'spec')
                   OR instr(COALESCE(s.decorators, ''), ',test,') > 0
                   OR instr(COALESCE(s.decorators, ''), ',tokio::test,') > 0
                   OR instr(COALESCE(s.decorators, ''), ',async_std::test,') > 0
                   OR instr(COALESCE(s.decorators, ''), ',Fact,') > 0
                   OR instr(COALESCE(s.decorators, ''), ',Theory,') > 0
                   OR instr(COALESCE(s.decorators, ''), ',TestMethod,') > 0
                   OR instr(COALESCE(s.decorators, ''), ',TestCase,') > 0
                   OR instr(COALESCE(s.decorators, ''), ',ParameterizedTest,') > 0
               )
               AND lower(s.name) NOT IN (
                   'setup', 'teardown', 'setup_method', 'teardown_method',
                   'beforeeach', 'aftereach', 'beforeall', 'afterall',
                   'testinitialize', 'testcleanup'
               )
               AND instr(COALESCE(s.decorators, ''), ',fixture,') = 0
               AND instr(COALESCE(s.decorators, ''), ',pytest.fixture,') = 0
               AND instr(COALESCE(s.decorators, ''), ',SetUp,') = 0
               AND instr(COALESCE(s.decorators, ''), ',TearDown,') = 0
             ORDER BY s.file_path, s.line_start, s.name, s.kind, s.id
             LIMIT ?{limit_param}"
        );
        let mut values: Vec<SqlValue> = components.iter().cloned().map(SqlValue::Text).collect();
        values.push(SqlValue::Integer(row_limit as i64));
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values), Self::row_to_symbol)?;
        rows.collect()
    }

    /// Incoming-edge count per file: for every defining file, how many
    /// call/import/inherit edges resolve (by leaf name or type prefix) to a
    /// symbol declared there. This is the file-level centrality signal — the
    /// most depended-on files. Name-based like `callers`/`impact`, so it pools
    /// across same-named symbols in different files; a ranking heuristic, not an
    /// exact reference count. Bounded by `limit`; `module` synthetics excluded.
    pub fn file_in_degrees(
        &self,
        production_only: bool,
        limit: usize,
    ) -> SqlResult<Vec<FileInDegree>> {
        self.file_in_degrees_scoped("", "root", production_only, limit)
    }

    pub(crate) fn file_in_degrees_scoped(
        &self,
        scope: &str,
        kind: &str,
        production_only: bool,
        limit: usize,
    ) -> SqlResult<Vec<FileInDegree>> {
        let sql = "WITH raw_refs AS (
                 SELECT to_name AS nm, COUNT(*) AS edge_count
                 FROM edges
                 WHERE to_name <> ''
                 GROUP BY to_name
                 UNION ALL
                 SELECT to_type AS nm, COUNT(*) AS edge_count
                 FROM edges
                 WHERE to_type IS NOT NULL AND to_type <> ''
                 GROUP BY to_type
             ), refs AS (
                 SELECT nm, SUM(edge_count) AS edge_count
                 FROM raw_refs
                 GROUP BY nm
             )
             SELECT s.file_path AS file, SUM(r.edge_count) AS deg
             FROM refs r
             JOIN symbols s ON s.name = r.nm
             WHERE s.kind != 'module'
               AND (
                   ?2 = 'root'
                   OR (?2 = 'file' AND s.file_path = ?1)
                   OR (
                       ?2 = 'directory'
                       AND substr(s.file_path, 1, length(?1) + 1) = ?1 || '/'
                   )
               )
               AND (?3 = 0 OR s.production = 1)
             GROUP BY s.file_path
             ORDER BY deg DESC, s.file_path
             LIMIT ?4";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![scope, kind, production_only, limit as i64], |r| {
            Ok(FileInDegree {
                file: r.get(0)?,
                in_degree: r.get::<_, i64>(1)? as u32,
            })
        })?;
        rows.collect()
    }

    /// Largest files by their last symbol's end line. Exact line count is not
    /// stored, so this is a maintainability ranking proxy. Honors
    /// `production_only`; bounded by `limit`.
    pub fn largest_files(&self, production_only: bool, limit: usize) -> SqlResult<Vec<FileSize>> {
        self.largest_files_scoped("", "root", production_only, limit)
    }

    pub(crate) fn largest_files_scoped(
        &self,
        scope: &str,
        kind: &str,
        production_only: bool,
        limit: usize,
    ) -> SqlResult<Vec<FileSize>> {
        let sql = "SELECT file_path AS file, MAX(line_end) AS lines
             FROM symbols
             WHERE line_end > 0
               AND (
                   ?2 = 'root'
                   OR (?2 = 'file' AND file_path = ?1)
                   OR (
                       ?2 = 'directory'
                       AND substr(file_path, 1, length(?1) + 1) = ?1 || '/'
                   )
               )
               AND (?3 = 0 OR production = 1)
             GROUP BY file_path
             ORDER BY lines DESC, file_path
             LIMIT ?4";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![scope, kind, production_only, limit as i64], |r| {
            Ok(FileSize {
                file: r.get(0)?,
                lines: r.get::<_, i64>(1)? as u32,
            })
        })?;
        rows.collect()
    }

    /// Symbols no edge references by `to_name` or `to_type`. Excludes synthetic
    /// `<module>` rows (never "called") and symbols with framework-registered
    /// decorators (pytest, FastAPI/Flask routes, Triton/Numba JIT, Click
    /// commands, Celery tasks, Rust `#[test]` / `#[tokio::main]`), plus
    /// pytest-convention test functions (`test_*` in test files).
    ///
    /// **Remaining false-positives** (caller responsibility):
    /// - Entry points (`main`, framework handlers without decorators)
    /// - Dynamic dispatch / reflection / trait objects whose calls don't surface
    /// - Cross-language calls
    /// - Functions registered via dict / list at runtime
    ///
    /// Optional `kind` (e.g. "function") and `language` filters.
    pub fn unreferenced(
        &self,
        kind: Option<&str>,
        language: Option<&str>,
    ) -> SqlResult<Vec<Symbol>> {
        let candidates = unreferenced_candidates_sql();
        let sql = format!(
            "{candidates} SELECT {SYMBOL_COLS} FROM candidates
             ORDER BY file_path, line_start, name, kind, id"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![kind, language, "", "root", false],
            Self::row_to_symbol,
        )?;
        rows.collect()
    }

    pub(crate) fn unreferenced_bounded(
        &self,
        kind: Option<&str>,
        language: Option<&str>,
        scope: &str,
        scope_kind: &str,
        production_only: bool,
        limit: usize,
    ) -> SqlResult<(u32, Vec<Symbol>)> {
        let candidates = unreferenced_candidates_sql();
        let rows_sql = format!(
            "{candidates} SELECT {SYMBOL_COLS}, COUNT(*) OVER() AS total FROM candidates
             ORDER BY file_path, line_start, name, kind, id
             LIMIT ?6"
        );
        let mut stmt = self.conn.prepare(&rows_sql)?;
        let rows = stmt.query_map(
            params![
                kind,
                language,
                scope,
                scope_kind,
                production_only,
                i64::try_from(limit.max(1)).unwrap_or(i64::MAX)
            ],
            |row| Ok((Self::row_to_symbol(row)?, row.get::<_, i64>(9)?)),
        )?;
        let mut count = 0;
        let mut symbols = Vec::with_capacity(limit);
        for row in rows {
            let (symbol, total) = row?;
            count = total;
            if symbols.len() < limit {
                symbols.push(symbol);
            }
        }
        Ok((count.clamp(0, i64::from(u32::MAX)) as u32, symbols))
    }

    /// Symbols defined under `path_prefix` referenced from at least one file
    /// OUTSIDE the prefix. "Empirical API surface" — independent of declared
    /// visibility (which mmcg doesn't extract).
    ///
    /// `path_prefix` matched via SQL `LIKE` — pass without `%`; we append it.
    /// Optional `language` filter.
    pub fn api_surface(&self, path_prefix: &str, language: Option<&str>) -> SqlResult<Vec<Symbol>> {
        let pattern = if path_prefix.ends_with('%') {
            path_prefix.to_string()
        } else {
            format!("{path_prefix}%")
        };
        let sql = format!(
            "WITH external_refs AS (
                 SELECT DISTINCT e.to_name AS nm
                 FROM edges e
                 JOIN symbols caller ON caller.id = e.from_id
                 WHERE caller.file_path NOT LIKE ?1
                 UNION
                 SELECT DISTINCT e.to_type AS nm
                 FROM edges e
                 JOIN symbols caller ON caller.id = e.from_id
                 WHERE caller.file_path NOT LIKE ?1
                   AND e.to_type IS NOT NULL AND e.to_type <> ''
             )
             SELECT DISTINCT {SYMBOL_COLS_S}
             FROM symbols s
             JOIN external_refs r ON r.nm = s.name
             WHERE s.file_path LIKE ?1
               AND (?2 IS NULL OR s.language = ?2)
               AND s.kind != 'module'
             ORDER BY s.file_path, s.line_start"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![pattern, language], Self::row_to_symbol)?;
        rows.collect()
    }

    /// Rank symbols by **in-degree** — how many distinct symbols call them
    /// (matched by `to_name` or `to_type`, like `callers_of`). The top is the
    /// codebase's structural attractor surface: utilities everyone depends on,
    /// core domain types, framework registration points.
    ///
    /// Planner pre-flight on an unfamiliar codebase or path prefix: "the 20
    /// most-referenced symbols in `src/auth/`?" cheaply answers "read first".
    ///
    /// - `path_prefix`: limit to `file_path` starting with this prefix. `None` =
    ///   whole index. Trailing `%` accepted, otherwise appended.
    /// - `language`, `kind`: standard filters.
    /// - `top`: result count (caller decides — no hard cap).
    ///
    /// Excludes synthetic `<module>` symbols (always-zero in-degree under
    /// name-matched edges) and symbols referenced nowhere (in-degree 0).
    pub fn centrality(
        &self,
        path_prefix: Option<&str>,
        language: Option<&str>,
        kind: Option<&str>,
        top: u32,
    ) -> SqlResult<Vec<(Symbol, u32, u32)>> {
        let pattern = path_prefix.map(|p| {
            if p.ends_with('%') {
                p.to_string()
            } else {
                format!("{p}%")
            }
        });
        // In-degree = distinct CALLER symbols, not call sites. Mirrors
        // `mmcg_callers` — 5 calls to `foo` from the same caller count once.
        let sql = format!(
            "WITH deg AS (
                 SELECT nm, COUNT(DISTINCT from_id) AS d FROM (
                     SELECT to_name AS nm, from_id FROM edges WHERE kind = 'calls'
                     UNION ALL
                     SELECT to_type AS nm, from_id FROM edges
                       WHERE kind = 'calls' AND to_type IS NOT NULL AND to_type <> ''
                 ) GROUP BY nm
             ),
             defs AS (
                 SELECT name, COUNT(*) AS n FROM symbols WHERE kind != 'module' GROUP BY name
             )
             SELECT {SYMBOL_COLS_S}, deg.d AS in_degree, defs.n AS name_collision
             FROM symbols s
             JOIN deg  ON deg.nm = s.name
             JOIN defs ON defs.name = s.name
             WHERE s.kind != 'module'
               AND (?1 IS NULL OR s.file_path LIKE ?1)
               AND (?2 IS NULL OR s.language = ?2)
               AND (?3 IS NULL OR s.kind = ?3)
             ORDER BY in_degree DESC, s.file_path, s.line_start
             LIMIT ?4"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![pattern, language, kind, top], |r| {
            let sym = Self::row_to_symbol(r)?;
            // in_degree / name_collision follow the 9 SYMBOL_COLS_S columns.
            let in_degree: u32 = r.get(9)?;
            let name_collision: u32 = r.get(10)?;
            Ok((sym, in_degree, name_collision))
        })?;
        rows.collect()
    }

    pub fn map_paths(&self, scope: &str, kind: &str, limit: usize) -> SqlResult<Vec<String>> {
        self.map_paths_filtered(scope, kind, limit, false)
    }

    pub fn map_paths_filtered(
        &self,
        scope: &str,
        kind: &str,
        limit: usize,
        production_only: bool,
    ) -> SqlResult<Vec<String>> {
        let sql = "SELECT path
             FROM files
             WHERE (
                ?2 = 'root'
                OR (?2 = 'file' AND path = ?1)
                OR (
                    ?2 = 'directory'
                    AND substr(path, 1, length(?1) + 1) = ?1 || '/'
                )
             )
             AND (?4 = 0 OR production = 1)
             ORDER BY path
             LIMIT ?3";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![scope, kind, limit as i64, production_only], |row| {
            row.get(0)
        })?;
        rows.collect()
    }

    pub fn map_boundaries(
        &self,
        components: &[MapBoundaryScope],
        limit_per_component: usize,
        global_limit: usize,
    ) -> SqlResult<Vec<MapBoundaryRow>> {
        self.map_boundaries_filtered(components, limit_per_component, global_limit, false)
    }

    pub fn map_boundaries_filtered(
        &self,
        components: &[MapBoundaryScope],
        limit_per_component: usize,
        global_limit: usize,
        production_only: bool,
    ) -> SqlResult<Vec<MapBoundaryRow>> {
        if components.is_empty() || limit_per_component == 0 || global_limit == 0 {
            return Ok(Vec::new());
        }
        let symbol_filter = maybe_production_symbol_filter(production_only, "s");
        let caller_filter = maybe_production_symbol_filter(production_only, "caller");
        // Prefix range predicates keep recursive scopes on idx_symbols_file.
        // Repository paths are normalized, and every string beginning with
        // `path || '/'` sorts before the exclusive `path || '0'` bound.
        let in_component = |column: &str| {
            format!(
                "(
                    (?2 = 1 AND (
                        (?1 = '' AND instr({column}, '/') = 0)
                        OR (
                            ?1 != ''
                            AND {column} >= ?1 || '/'
                            AND {column} < ?1 || '0'
                            AND instr(substr({column}, length(?1) + 2), '/') = 0
                        )
                    ))
                    OR (
                        ?2 = 0
                        AND {column} >= ?1 || '/'
                        AND {column} < ?1 || '0'
                    )
                )"
            )
        };
        let symbol_scope = in_component("s.file_path");
        let caller_scope = in_component("caller.file_path");
        let sql = format!(
            "WITH scoped_names(name) AS MATERIALIZED (
                 SELECT DISTINCT s.name
                 FROM symbols s INDEXED BY idx_symbols_file
                 WHERE {symbol_scope}
                   AND s.kind != 'module'
                   {symbol_filter}
             ),
             boundary_names(name) AS MATERIALIZED (
                 SELECT n.name
                 FROM scoped_names n
                 CROSS JOIN edges e INDEXED BY idx_edges_calls_to_name
                 JOIN symbols caller ON caller.id = e.from_id
                 WHERE e.to_name = n.name
                   AND e.kind = 'calls'
                   AND NOT {caller_scope}
                   {caller_filter}
                 UNION
                 SELECT n.name
                 FROM scoped_names n
                 CROSS JOIN edges e INDEXED BY idx_edges_calls_to_type
                 JOIN symbols caller ON caller.id = e.from_id
                 WHERE e.to_type = n.name
                   AND e.kind = 'calls'
                   AND e.to_type IS NOT NULL
                   AND e.to_type <> ''
                   AND NOT {caller_scope}
                   {caller_filter}
             )
             SELECT {SYMBOL_COLS_S},
                    COALESCE(parent.file_path, '') AS parent_file_path,
                    COALESCE(parent.line_start, -1) AS parent_line_start,
                    COALESCE(parent.name, '') AS parent_name,
                    COALESCE(parent.kind, '') AS parent_kind,
                    COALESCE(parent.line_end, -1) AS parent_line_end,
                    COALESCE(parent.signature, '') AS parent_signature,
                    COALESCE(parent.decorators, '') AS parent_decorators
             FROM symbols s INDEXED BY idx_symbols_file
             LEFT JOIN symbols parent ON parent.id = s.parent_id
             WHERE {symbol_scope}
               AND s.kind != 'module'
               AND s.name IN (SELECT name FROM boundary_names)
               {symbol_filter}
             ORDER BY s.file_path, s.line_start, s.name, s.kind, s.line_end,
                      COALESCE(s.signature, ''), COALESCE(s.decorators, ''),
                      parent_file_path, parent_line_start, parent_name, parent_kind,
                      parent_line_end, parent_signature, parent_decorators
             LIMIT ?3"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut ordered = components.iter().collect::<Vec<_>>();
        ordered.sort_by(|left, right| left.label.cmp(&right.label));
        let mut result = Vec::new();
        for component in ordered {
            let remaining = global_limit.saturating_sub(result.len());
            if remaining == 0 {
                break;
            }
            let direct_only = match component.match_mode {
                MapBoundaryMatch::Direct => 1,
                MapBoundaryMatch::Recursive => 0,
            };
            let row_limit = limit_per_component.min(remaining) as i64;
            let rows = stmt.query_map(params![component.path, direct_only, row_limit], |row| {
                Ok(MapBoundaryRow {
                    component: component.label.clone(),
                    symbol: Self::row_to_symbol(row)?,
                })
            })?;
            result.extend(rows.collect::<SqlResult<Vec<_>>>()?);
        }
        Ok(result)
    }

    pub fn map_centrality(
        &self,
        scope: &str,
        kind: &str,
        top_probe: usize,
    ) -> SqlResult<Vec<MapCentralityRow>> {
        self.map_centrality_filtered(scope, kind, top_probe, false)
    }

    pub fn map_centrality_filtered(
        &self,
        scope: &str,
        kind: &str,
        top_probe: usize,
        production_only: bool,
    ) -> SqlResult<Vec<MapCentralityRow>> {
        if kind == "root" {
            // A whole-index map already contains every definition and every
            // caller, so joining each call edge back through scoped_names (and
            // then through caller) only multiplies work. Aggregate the two
            // target indexes directly, choose at most `top_probe` names by
            // their first deterministic definition, and expand definitions
            // only for those candidate names.
            let caller_join = if production_only {
                "JOIN symbols caller ON caller.id = edges.from_id"
            } else {
                ""
            };
            let caller_filter = maybe_production_symbol_filter(production_only, "caller");
            let collision_filter = maybe_production_symbol_filter(production_only, "collision");
            let first_filter = maybe_production_symbol_filter(production_only, "first");
            let result_filter = maybe_production_symbol_filter(production_only, "s");
            let sql = format!(
                "WITH callers(nm, from_id) AS (
                     SELECT edges.to_name, edges.from_id
                     FROM edges INDEXED BY idx_edges_calls_to_name
                     {caller_join}
                     WHERE edges.kind = 'calls'
                       {caller_filter}
                     UNION
                     SELECT edges.to_type, edges.from_id
                     FROM edges INDEXED BY idx_edges_calls_to_type
                     {caller_join}
                     WHERE edges.kind = 'calls'
                       AND edges.to_type IS NOT NULL
                       AND edges.to_type <> ''
                       {caller_filter}
                 ),
                 name_degrees AS MATERIALIZED (
                     SELECT nm, COUNT(*) AS in_degree
                     FROM callers
                     GROUP BY nm
                 ),
                 collisions AS MATERIALIZED (
                     SELECT collision.name, COUNT(*) AS name_collision
                     FROM symbols collision INDEXED BY idx_symbols_name
                     WHERE collision.kind != 'module'
                       {collision_filter}
                     GROUP BY collision.name
                 ),
                 name_first AS MATERIALIZED (
                     SELECT collisions.name,
                            collisions.name_collision,
                            (
                                SELECT first.id
                                FROM symbols first INDEXED BY idx_symbols_name
                                LEFT JOIN symbols first_parent ON first_parent.id = first.parent_id
                                WHERE first.name = collisions.name
                                  AND first.kind != 'module'
                                  {first_filter}
                                ORDER BY first.file_path, first.line_start, first.name,
                                         first.kind, first.line_end,
                                         COALESCE(first.signature, ''),
                                         COALESCE(first.decorators, ''),
                                         COALESCE(first_parent.file_path, ''),
                                         COALESCE(first_parent.line_start, -1),
                                         COALESCE(first_parent.name, ''),
                                         COALESCE(first_parent.kind, ''),
                                         COALESCE(first_parent.line_end, -1),
                                         COALESCE(first_parent.signature, ''),
                                         COALESCE(first_parent.decorators, '')
                                LIMIT 1
                            ) AS first_id
                     FROM collisions
                 ),
                 candidate_names AS MATERIALIZED (
                     SELECT degrees.nm,
                            degrees.in_degree,
                            names.name_collision
                     FROM name_degrees degrees
                     JOIN name_first names ON names.name = degrees.nm
                     JOIN symbols first ON first.id = names.first_id
                     LEFT JOIN symbols first_parent ON first_parent.id = first.parent_id
                     ORDER BY (names.name_collision > 1), degrees.in_degree DESC,
                              first.file_path, first.line_start, first.name,
                              first.kind, first.line_end,
                              COALESCE(first.signature, ''),
                              COALESCE(first.decorators, ''),
                              COALESCE(first_parent.file_path, ''),
                              COALESCE(first_parent.line_start, -1),
                              COALESCE(first_parent.name, ''),
                              COALESCE(first_parent.kind, ''),
                              COALESCE(first_parent.line_end, -1),
                              COALESCE(first_parent.signature, ''),
                              COALESCE(first_parent.decorators, '')
                     LIMIT ?1
                 )
                 SELECT {SYMBOL_COLS_S},
                        candidates.in_degree,
                        candidates.name_collision
                 FROM candidate_names candidates
                 JOIN symbols s INDEXED BY idx_symbols_name ON s.name = candidates.nm
                 LEFT JOIN symbols parent ON parent.id = s.parent_id
                 WHERE s.kind != 'module'
                   {result_filter}
                 ORDER BY (candidates.name_collision > 1),
                          candidates.in_degree DESC,
                          s.file_path, s.line_start, s.name, s.kind, s.line_end,
                          COALESCE(s.signature, ''), COALESCE(s.decorators, ''),
                          COALESCE(parent.file_path, ''),
                          COALESCE(parent.line_start, -1),
                          COALESCE(parent.name, ''), COALESCE(parent.kind, ''),
                          COALESCE(parent.line_end, -1),
                          COALESCE(parent.signature, ''),
                          COALESCE(parent.decorators, '')
                 LIMIT ?1"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params![top_probe as i64], |row| {
                Ok(MapCentralityRow {
                    symbol: Self::row_to_symbol(row)?,
                    in_degree: row.get(9)?,
                    name_collision: row.get(10)?,
                })
            })?;
            return rows.collect();
        }

        let definition_filter = maybe_production_symbol_filter(production_only, "s");
        let caller_filter = maybe_production_symbol_filter(production_only, "caller");
        let sql = format!(
            "WITH scoped_defs AS (
                 SELECT {SYMBOL_COLS_S},
                        COALESCE(parent.file_path, '') AS parent_file_path,
                        COALESCE(parent.line_start, -1) AS parent_line_start,
                        COALESCE(parent.name, '') AS parent_name,
                        COALESCE(parent.kind, '') AS parent_kind,
                        COALESCE(parent.line_end, -1) AS parent_line_end,
                        COALESCE(parent.signature, '') AS parent_signature,
                        COALESCE(parent.decorators, '') AS parent_decorators
                 FROM symbols s
                 LEFT JOIN symbols parent ON parent.id = s.parent_id
                 WHERE s.kind != 'module'
                   {definition_filter}
                   AND (
                       ?2 = 'root'
                       OR (?2 = 'file' AND s.file_path = ?1)
                       OR (
                           ?2 = 'directory'
                           AND substr(s.file_path, 1, length(?1) + 1) = ?1 || '/'
                       )
                   )
             ),
             scoped_names AS (
                 SELECT DISTINCT name
                 FROM scoped_defs
             ),
             -- In-degree depends only on a definition's *name* (plus the
             -- uniform caller-side production_only filter), never on which
             -- specific def row it is — so aggregate once over the UNION ALL
             -- of the to_name and to_type edge branches (grouping by to_name
             -- alone would drop `Type::method()` callers) and join scoped
             -- names to the dedicated target indexes. This prevents a small
             -- map scope from scanning every call edge in a monorepo.
             name_degrees AS (
                 SELECT nm, COUNT(DISTINCT from_id) AS in_degree FROM (
                     SELECT e.to_name AS nm, e.from_id
                     FROM scoped_names n
                     JOIN edges e INDEXED BY idx_edges_calls_to_name
                       ON e.to_name = n.name AND e.kind = 'calls'
                     JOIN symbols caller ON caller.id = e.from_id
                     WHERE 1 = 1
                     {caller_filter}
                     UNION ALL
                     SELECT e.to_type AS nm, e.from_id
                     FROM scoped_names n
                     JOIN edges e INDEXED BY idx_edges_calls_to_type
                       ON e.to_type = n.name
                      AND e.kind = 'calls'
                      AND e.to_type IS NOT NULL
                      AND e.to_type <> ''
                     JOIN symbols caller ON caller.id = e.from_id
                     WHERE 1 = 1
                     {caller_filter}
                 ) GROUP BY nm
             ),
             collisions AS (
                 SELECT n.name,
                        (
                            SELECT COUNT(*)
                            FROM symbols s INDEXED BY idx_symbols_name
                            WHERE s.kind != 'module' AND s.name = n.name
                            {definition_filter}
                        ) AS name_collision
                 FROM scoped_names n
             )
             SELECT d.id, d.name, d.kind, d.file_path, d.line_start, d.line_end,
                    d.signature, d.parent_id, d.decorators,
                    degrees.in_degree, collisions.name_collision
             FROM scoped_defs d
             JOIN name_degrees degrees ON degrees.nm = d.name
             JOIN collisions ON collisions.name = d.name
             ORDER BY (collisions.name_collision > 1), degrees.in_degree DESC,
                      d.file_path, d.line_start, d.name,
                      d.kind, d.line_end, COALESCE(d.signature, ''),
                      COALESCE(d.decorators, ''), d.parent_file_path,
                      d.parent_line_start, d.parent_name, d.parent_kind,
                      d.parent_line_end, d.parent_signature, d.parent_decorators
             LIMIT ?3"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![scope, kind, top_probe as i64], |row| {
            Ok(MapCentralityRow {
                symbol: Self::row_to_symbol(row)?,
                in_degree: row.get(9)?,
                name_collision: row.get(10)?,
            })
        })?;
        rows.collect()
    }

    pub fn map_import_edges(
        &self,
        scope: &str,
        kind: &str,
        limit: usize,
    ) -> SqlResult<Vec<(String, String)>> {
        self.map_import_edges_filtered(scope, kind, limit, false)
    }

    pub fn map_import_edges_filtered(
        &self,
        scope: &str,
        kind: &str,
        limit: usize,
        production_only: bool,
    ) -> SqlResult<Vec<(String, String)>> {
        let rows =
            self.generic_map_import_edges_filtered(scope, kind, limit, production_only, true)?;
        let mut pairs: std::collections::BTreeSet<(String, String)> = rows.into_iter().collect();
        let (cpp_pairs, _) = self.cpp_include_pairs(Some((scope, kind)), limit, production_only)?;
        pairs.extend(cpp_pairs);
        Ok(pairs.into_iter().take(limit).collect())
    }

    /// Fetch the scoped import graph up to an exact work cap. Once more than
    /// `cap` distinct generic edges exist, callers only need the truncation
    /// fact: running an SCC algorithm on a partial graph would be misleading.
    /// Omitting ORDER BY lets SQLite stop as soon as that fact is known instead
    /// of sorting the entire monorepo graph.
    pub fn map_import_edges_capped_filtered(
        &self,
        scope: &str,
        kind: &str,
        cap: usize,
        production_only: bool,
    ) -> SqlResult<(Vec<(String, String)>, bool)> {
        let generic = self.generic_map_import_edges_filtered(
            scope,
            kind,
            cap.saturating_add(1),
            production_only,
            false,
        )?;
        if generic.len() > cap {
            return Ok((Vec::new(), true));
        }

        let (cpp_pairs, cpp_truncated) =
            self.cpp_include_pairs(Some((scope, kind)), cap, production_only)?;
        let mut pairs: std::collections::BTreeSet<(String, String)> = generic.into_iter().collect();
        pairs.extend(cpp_pairs);
        if cpp_truncated || pairs.len() > cap {
            return Ok((Vec::new(), true));
        }
        Ok((pairs.into_iter().collect(), false))
    }

    fn generic_map_import_edges_filtered(
        &self,
        scope: &str,
        kind: &str,
        limit: usize,
        production_only: bool,
        deterministic_top: bool,
    ) -> SqlResult<Vec<(String, String)>> {
        let source_filter = maybe_production_symbol_filter(production_only, "source");
        let target_filter = maybe_production_symbol_filter(production_only, "target");
        let order_clause =
            deterministic_top.then_some("ORDER BY source.file_path, target.file_path");
        let order_clause = order_clause.unwrap_or("");
        let sql = format!(
            "WITH target_files AS (
                 -- Pre-dedup (name, file_path) before joining import edges:
                 -- output is DISTINCT file pairs, so collapsing same-named
                 -- symbols within a file is identical to the previous
                 -- per-edge fanout join, one pass over `symbols` instead of
                 -- joining every edge to every same-named symbol.
                 SELECT DISTINCT target.name, target.file_path
                 FROM symbols target
                 WHERE 1 = 1
                 {target_filter}
             )
             SELECT DISTINCT source.file_path, target.file_path
             FROM edges e
             JOIN symbols source ON source.id = e.from_id
             JOIN target_files target ON target.name = e.to_name
             WHERE e.kind = 'imports'
               AND (source.language IS NULL OR source.language != 'cpp')
               AND source.file_path != target.file_path
               {source_filter}
               AND (
                   ?2 = 'root'
                   OR (?2 = 'file' AND source.file_path = ?1)
                   OR (
                       ?2 = 'directory'
                       AND substr(source.file_path, 1, length(?1) + 1) = ?1 || '/'
                   )
               )
               AND (
                   ?2 = 'root'
                   OR (?2 = 'file' AND target.file_path = ?1)
                   OR (
                       ?2 = 'directory'
                       AND substr(target.file_path, 1, length(?1) + 1) = ?1 || '/'
                   )
               )
             {order_clause}
             LIMIT ?3"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![scope, kind, limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        rows.collect()
    }

    /// Resolve syntactic C/C++ `#include` edges to indexed files. Includes are
    /// file dependencies, unlike `using` declarations, and therefore cannot
    /// be resolved through a target symbol's leaf name.
    fn cpp_include_pairs(
        &self,
        scope: Option<(&str, &str)>,
        pair_limit: usize,
        production_only: bool,
    ) -> SqlResult<(std::collections::BTreeSet<(String, String)>, bool)> {
        let source_filter = maybe_production_symbol_filter(production_only, "source");
        let target_filter = maybe_production_symbol_filter(production_only, "target");
        let scope_path = scope.map(|(path, _)| path);
        let scope_kind = scope.map(|(_, kind)| kind);

        let target_sql = format!(
            "SELECT DISTINCT target.file_path
             FROM symbols target
             WHERE target.language = 'cpp'
               {target_filter}
               AND (
                   ?2 IS NULL OR ?2 = 'root'
                   OR (?2 = 'file' AND target.file_path = ?1)
                   OR (
                       ?2 = 'directory'
                       AND substr(target.file_path, 1, length(?1) + 1) = ?1 || '/'
                   )
               )
             ORDER BY target.file_path"
        );
        let mut target_stmt = self.conn.prepare(&target_sql)?;
        let target_paths: Vec<String> = target_stmt
            .query_map(params![scope_path, scope_kind], |row| row.get(0))?
            .collect::<SqlResult<_>>()?;
        let target_set: std::collections::BTreeSet<String> = target_paths.iter().cloned().collect();

        let mut by_suffix: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for path in &target_paths {
            let lower = path.to_ascii_lowercase();
            let basename = lower.rsplit('/').next().unwrap_or(&lower).to_string();
            by_suffix.entry(basename).or_default().push(path.clone());
        }

        let raw_limit = pair_limit.saturating_add(1);
        let source_sql = format!(
            "SELECT DISTINCT source.file_path, e.to_path
             FROM edges e
             JOIN symbols source ON source.id = e.from_id
             WHERE e.kind = 'imports'
               AND source.language = 'cpp'
               AND e.to_name != '*'
               AND e.to_path IS NOT NULL
               AND e.to_path LIKE '%::*'
               {source_filter}
               AND (
                   ?2 IS NULL OR ?2 = 'root'
                   OR (?2 = 'file' AND source.file_path = ?1)
                   OR (
                       ?2 = 'directory'
                       AND substr(source.file_path, 1, length(?1) + 1) = ?1 || '/'
                   )
               )
             ORDER BY source.file_path, e.to_path
             LIMIT ?3"
        );
        let mut source_stmt = self.conn.prepare(&source_sql)?;
        let mut raw: Vec<(String, String)> = source_stmt
            .query_map(params![scope_path, scope_kind, raw_limit as i64], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<SqlResult<_>>()?;
        let raw_truncated = raw.len() > pair_limit;
        raw.truncate(pair_limit);

        let mut pairs = std::collections::BTreeSet::new();
        for (source, encoded_include) in raw {
            let include = encoded_include
                .strip_suffix("::*")
                .unwrap_or(&encoded_include)
                .replace('\\', "/");
            let mut resolved = std::collections::BTreeSet::new();

            if let Some(path) = normalize_repo_relative(&include) {
                if target_set.contains(&path) {
                    resolved.insert(path);
                }
            }
            if let Some(parent) = source.rsplit_once('/').map(|(parent, _)| parent) {
                if let Some(path) = normalize_repo_relative(&format!("{parent}/{include}")) {
                    if target_set.contains(&path) {
                        resolved.insert(path);
                    }
                }
            }

            // Project include roots are not available to the syntactic
            // indexer. Fall back deterministically to matching suffixes; this
            // deliberately over-approximates when several headers share a
            // basename, while still preserving the actual target file path.
            if resolved.is_empty() {
                let lower = include.to_ascii_lowercase();
                let basename = lower.rsplit('/').next().unwrap_or(&lower);
                if let Some(candidates) = by_suffix.get(basename) {
                    for candidate in candidates {
                        let candidate_lower = candidate.to_ascii_lowercase();
                        if !lower.contains('/')
                            || candidate_lower == lower
                            || candidate_lower.ends_with(&format!("/{lower}"))
                        {
                            resolved.insert(candidate.clone());
                        }
                    }
                }
            }

            for target in resolved {
                if source != target {
                    pairs.insert((source.clone(), target));
                }
            }
        }

        let pair_truncated = pairs.len() > pair_limit;
        if pair_truncated {
            pairs = pairs.into_iter().take(pair_limit).collect();
        }
        Ok((pairs, raw_truncated || pair_truncated))
    }

    /// Replace the entire task-spec corpus with the supplied entries. Called by
    /// `Indexer::index_task_specs` after scanning `.mastermind/tasks/<NNN>-<name>/spec.md`.
    /// Single transaction — atomic to readers.
    pub fn replace_task_specs(&mut self, entries: &[TaskSpecEntry]) -> SqlResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM task_specs_fts", [])?;
        {
            let mut stmt =
                tx.prepare("INSERT INTO task_specs_fts(path, title, body) VALUES (?1, ?2, ?3)")?;
            for entry in entries {
                stmt.execute(params![entry.path, entry.title, entry.body])?;
            }
        }
        tx.commit()
    }

    /// Atomically replace the derived project-history corpus. The indexer calls
    /// this after scanning the supported Markdown sources, which also removes
    /// stale rows after a rename or deletion.
    pub fn replace_project_history(&mut self, entries: &[ProjectHistoryEntry]) -> SqlResult<()> {
        self.replace_project_history_snapshot(entries, 0, false, "")
    }

    /// Replace the derived history rows and their inventory contract in one
    /// transaction. Readers can therefore never observe a new FTS corpus with
    /// the previous freshness token (or the reverse).
    pub(crate) fn replace_project_history_snapshot(
        &mut self,
        entries: &[ProjectHistoryEntry],
        skipped: u32,
        truncated: bool,
        inventory_token: &str,
    ) -> SqlResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM project_history_fts", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO project_history_fts(path, kind, title, body)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for entry in entries {
                stmt.execute(params![entry.path, entry.kind, entry.title, entry.body])?;
            }
        }
        for (key, value) in [
            ("project_history_skipped", skipped.to_string()),
            (
                "project_history_truncated",
                if truncated { "true" } else { "false" }.to_string(),
            ),
            (
                "project_history_inventory_token",
                inventory_token.to_string(),
            ),
        ] {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )?;
        }
        tx.commit()
    }

    /// Strongly-connected components of size ≥ `min_size` in the file-level
    /// import graph. A returned SCC = a circular-import group.
    ///
    /// Non-C++ edges are resolved by leaf-name match. C/C++ `#include` edges
    /// resolve to indexed header paths (exact, source-relative, then a
    /// deterministic suffix fallback); `using` declarations are excluded from
    /// the C++ file graph.
    ///
    /// Self-edges excluded (`from_file = to_file`).
    ///
    /// `min_size` defaults to 2 (smallest cycle). Higher surfaces only larger
    /// problems (min_size=3 hides trivial A→B→A).
    ///
    /// Work-capped: fetches at most [`DEPENDENCY_CYCLE_PAIR_LIMIT`] + 1
    /// distinct file pairs under a deterministic `ORDER BY` (so truncation is
    /// reproducible). Above the cap, Tarjan is **not** run — capping a graph
    /// algorithm's input can split or hide real cycles, so the second tuple
    /// element (`true`) marks the result "incomplete and possibly inaccurate",
    /// not merely "more available".
    pub fn dependency_cycles(
        &self,
        language: Option<&str>,
        min_size: usize,
    ) -> SqlResult<(Vec<Vec<String>>, bool)> {
        // Pre-dedup (name, file_path, language) over symbols before joining
        // import edges: output is DISTINCT file pairs, so collapsing
        // same-named symbols within a file is identical to the previous
        // per-edge fanout join, one pass over `symbols` instead of joining
        // every edge to every same-named symbol.
        let mut stmt = self.conn.prepare(
            "WITH to_files AS (
                 SELECT DISTINCT name, file_path, language
                 FROM symbols
                 WHERE (?1 IS NULL OR language = ?1)
             )
             SELECT DISTINCT
                s_from.file_path AS from_file,
                t.file_path      AS to_file
             FROM edges e
             JOIN symbols s_from ON s_from.id = e.from_id
             JOIN to_files t     ON t.name = e.to_name
             WHERE e.kind = 'imports'
               AND s_from.file_path != t.file_path
               AND (?1 IS NULL OR s_from.language = ?1)
               AND (s_from.language IS NULL OR s_from.language != 'cpp')
             ORDER BY from_file, to_file
             LIMIT ?2",
        )?;
        let cap = DEPENDENCY_CYCLE_PAIR_LIMIT;
        let rows = stmt.query_map(params![language, (cap + 1) as i64], |r| {
            let from: String = r.get(0)?;
            let to: String = r.get(1)?;
            Ok((from, to))
        })?;
        let generic_pairs: Vec<(String, String)> = rows.collect::<SqlResult<_>>()?;
        let generic_truncated = generic_pairs.len() > cap;
        let (cpp_pairs, cpp_truncated) = if language.is_none_or(|value| value == "cpp") {
            self.cpp_include_pairs(None, cap, false)?
        } else {
            (std::collections::BTreeSet::new(), false)
        };
        let mut pairs: std::collections::BTreeSet<(String, String)> =
            generic_pairs.into_iter().take(cap).collect();
        pairs.extend(cpp_pairs);
        if generic_truncated || cpp_truncated || pairs.len() > cap {
            return Ok((Vec::new(), true));
        }

        let mut adj: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for (from, to) in pairs {
            adj.entry(from).or_default().push(to);
        }

        let cycles = tarjan_scc(&adj);
        let mut out: Vec<Vec<String>> =
            cycles.into_iter().filter(|c| c.len() >= min_size).collect();
        // Stable order: largest cycles first, lex within.
        out.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        Ok((out, false))
    }

    /// Full-text search over the task-spec corpus. `query` is an FTS5 MATCH
    /// expression — bare words AND-joined, phrases double-quoted, `NOT`/`OR`
    /// supported. Returns `(path, title, snippet)` by BM25 rank. Empty /
    /// whitespace queries return nothing (FTS5 errors otherwise).
    pub fn search_task_specs(&self, query: &str, top: u32) -> SqlResult<Vec<TaskSpecHit>> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT path,
                    title,
                    snippet(task_specs_fts, 2, '«', '»', '…', 16) AS excerpt,
                    bm25(task_specs_fts) AS score
             FROM task_specs_fts
             WHERE task_specs_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![trimmed, top], |r| {
            Ok(TaskSpecHit {
                path: r.get(0)?,
                title: r.get(1)?,
                excerpt: r.get(2)?,
                score: r.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// Count of task specs currently indexed — for `mastermind status` diagnostics.
    pub fn task_specs_count(&self) -> SqlResult<u32> {
        self.conn
            .query_row("SELECT COUNT(*) FROM task_specs_fts", [], |r| r.get(0))
    }

    /// Full-text retrieval over durable project-history artifacts. This method
    /// returns observed matches only; callers must not treat rank as confidence
    /// or infer causality from co-occurrence.
    pub fn search_project_history(
        &self,
        query: &str,
        kind: Option<&str>,
        top: u32,
    ) -> SqlResult<Vec<ProjectHistoryHit>> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let kind = kind.map(str::trim).filter(|value| !value.is_empty());
        let mut stmt = self.conn.prepare(
            "SELECT path,
                    kind,
                    title,
                    snippet(project_history_fts, 3, '«', '»', '…', 16) AS excerpt,
                    bm25(project_history_fts) AS score,
                    snippet(project_history_fts, 2, char(30), char(31), '…', 16)
                        AS title_evidence,
                    snippet(project_history_fts, 3, char(30), char(31), '…', 16)
                        AS body_evidence,
                    CASE WHEN instr(title, char(30)) > 0
                               OR instr(title, char(31)) > 0
                               OR instr(body, char(30)) > 0
                               OR instr(body, char(31)) > 0
                         THEN 1 ELSE 0 END AS evidence_marker_collision
             FROM project_history_fts
             WHERE project_history_fts MATCH ?1
               AND (?2 IS NULL OR kind = ?2)
             ORDER BY rank
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![trimmed, kind, top], |row| {
            let title_evidence: String = row.get(5)?;
            let body_evidence: String = row.get(6)?;
            let marker_collision = row.get::<_, i64>(7)? != 0;
            Ok(ProjectHistoryHit {
                path: row.get(0)?,
                kind: row.get(1)?,
                title: row.get(2)?,
                excerpt: row.get(3)?,
                score: row.get(4)?,
                matched_terms: if marker_collision {
                    Vec::new()
                } else {
                    highlighted_fts_terms([&title_evidence, &body_evidence])
                },
            })
        })?;
        rows.collect()
    }

    pub(crate) fn search_concepts(
        &self,
        match_query: &str,
        top: u32,
    ) -> SqlResult<Vec<ConceptStoreHit>> {
        let mut statement = self.conn.prepare(
            "SELECT s.name,
                    s.kind,
                    s.language,
                    s.file_path,
                    s.line_start,
                    c.signature_search,
                    instr(
                        highlight(symbol_concepts_fts, 0, char(30), char(31)),
                        char(30)
                    ) > 0 AS name_matched,
                    instr(
                        highlight(symbol_concepts_fts, 1, char(30), char(31)),
                        char(30)
                    ) > 0 AS path_matched,
                    instr(
                        highlight(symbol_concepts_fts, 2, char(30), char(31)),
                        char(30)
                    ) > 0 AS signature_matched,
                    instr(
                        highlight(symbol_concepts_fts, 3, char(30), char(31)),
                        char(30)
                    ) > 0 AS documentation_matched,
                    bm25(symbol_concepts_fts, 10.0, 4.0, 2.0, 1.0) AS score
             FROM symbol_concepts_fts
             JOIN symbol_concepts c ON c.symbol_id = symbol_concepts_fts.rowid
             JOIN symbols s ON s.id = c.symbol_id
             WHERE symbol_concepts_fts MATCH ?1
             ORDER BY score ASC,
                      c.path_sort COLLATE BINARY ASC,
                      s.line_start ASC,
                      s.kind COLLATE BINARY ASC,
                      s.name COLLATE BINARY ASC,
                      s.id ASC
            LIMIT ?2",
        )?;
        let rows = statement.query_map(params![match_query, top], |row| {
            Ok(ConceptStoreHit {
                name: row.get(0)?,
                kind: row.get(1)?,
                language: row.get(2)?,
                path: row.get(3)?,
                line: row.get(4)?,
                signature_shape: row.get(5)?,
                name_matched: row.get(6)?,
                path_matched: row.get(7)?,
                signature_matched: row.get(8)?,
                documentation_matched: row.get(9)?,
                score: row.get(10)?,
            })
        })?;
        rows.collect()
    }

    /// Deterministic bounded read of the derived project-history corpus for
    /// local evidence correlation. Markdown remains authoritative.
    pub fn project_history_entries_bounded(
        &self,
        limit: usize,
        byte_limit: usize,
    ) -> SqlResult<(Vec<ProjectHistoryEntry>, bool)> {
        let query_limit = i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT path, kind, title, body
             FROM project_history_fts
             ORDER BY path, kind, title
             LIMIT ?1",
        )?;
        let mut rows = stmt.query(params![query_limit])?;
        let mut entries = Vec::new();
        let mut bytes = 0usize;
        let mut truncated = false;
        while let Some(row) = rows.next()? {
            if entries.len() >= limit {
                truncated = true;
                break;
            }
            let entry = ProjectHistoryEntry {
                path: row.get(0)?,
                kind: row.get(1)?,
                title: row.get(2)?,
                body: row.get(3)?,
            };
            let entry_bytes = entry
                .path
                .len()
                .saturating_add(entry.kind.len())
                .saturating_add(entry.title.len())
                .saturating_add(entry.body.len());
            if entry_bytes > byte_limit.saturating_sub(bytes) {
                truncated = true;
                break;
            }
            bytes = bytes.saturating_add(entry_bytes);
            entries.push(entry);
        }
        Ok((entries, truncated))
    }

    pub fn project_history_count(&self) -> SqlResult<u32> {
        self.conn
            .query_row("SELECT COUNT(*) FROM project_history_fts", [], |row| {
                row.get(0)
            })
    }

    /// Files with `indexed_at >= threshold_unix`. Backs `mmcg_recent_changes`
    /// ("what has the watcher touched lately").
    pub fn files_indexed_since(&self, threshold_unix: i64) -> SqlResult<Vec<FileEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, indexed_at, symbol_count FROM files
             WHERE indexed_at >= ?1
             ORDER BY indexed_at DESC",
        )?;
        let rows = stmt.query_map(params![threshold_unix], |r| {
            Ok(FileEntry {
                path: r.get(0)?,
                indexed_at: r.get(1)?,
                symbol_count: r.get(2)?,
            })
        })?;
        rows.collect()
    }

    /// Files indexed under a path prefix (None = everything). Optional `language`
    /// filter via EXISTS on symbols (language lives there, not on files). When
    /// set, zero-symbol files are excluded — a no-op in practice, since every
    /// indexed file has at least the synthetic `<module>` symbol.
    pub fn files_under(
        &self,
        prefix: Option<&str>,
        language: Option<&str>,
    ) -> SqlResult<Vec<FileEntry>> {
        let row_to_file = |r: &rusqlite::Row| {
            Ok(FileEntry {
                path: r.get(0)?,
                indexed_at: r.get(1)?,
                symbol_count: r.get(2)?,
            })
        };
        let mut stmt = self.conn.prepare(
            "SELECT f.path, f.indexed_at, f.symbol_count FROM files f
             WHERE (?1 IS NULL OR f.path LIKE ?1)
               AND (?2 IS NULL OR EXISTS (
                       SELECT 1 FROM symbols s
                       WHERE s.file_path = f.path AND s.language = ?2 LIMIT 1
                   ))
             ORDER BY f.path",
        )?;
        let rows = stmt.query_map(params![prefix, language], row_to_file)?;
        rows.collect()
    }

    /// All symbols defined in a given file, ordered by line.
    pub fn symbols_in_file(&self, file_path: &str) -> SqlResult<Vec<Symbol>> {
        let sql = format!(
            "SELECT {SYMBOL_COLS}
             FROM symbols WHERE file_path = ?1 ORDER BY line_start"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![file_path], Self::row_to_symbol)?;
        rows.collect()
    }

    /// Imports declared by a file. Returns (name, path, line).
    pub fn imports_of(&self, file_path: &str) -> SqlResult<Vec<(String, Option<String>, u32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.to_name, e.to_path, e.line FROM edges e
             JOIN symbols s ON s.id = e.from_id
             WHERE s.file_path = ?1 AND s.kind = 'module' AND e.kind = 'imports'
             ORDER BY e.line",
        )?;
        let rows = stmt.query_map(params![file_path], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, u32>(2)?,
            ))
        })?;
        rows.collect()
    }

    /// Files whose module imports the given name. Matches `to_name` (leaf
    /// binding). Optional `language` filter.
    pub fn imported_by_name(&self, name: &str, language: Option<&str>) -> SqlResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT s.file_path FROM edges e
             JOIN symbols s ON s.id = e.from_id
             WHERE e.kind = 'imports'
               AND e.to_name = ?1
               AND s.kind = 'module'
               AND (?2 IS NULL OR s.language = ?2)
             ORDER BY s.file_path",
        )?;
        let rows = stmt.query_map(params![name, language], |r| r.get::<_, String>(0))?;
        rows.collect()
    }

    /// Files whose module imports exactly this fully-qualified path. Matches
    /// `to_path` precisely — use when the same leaf name is imported from
    /// multiple modules and you want only one. Optional `language` filter.
    pub fn imported_by_path(&self, path: &str, language: Option<&str>) -> SqlResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT s.file_path FROM edges e
             JOIN symbols s ON s.id = e.from_id
             WHERE e.kind = 'imports'
               AND e.to_path = ?1
               AND s.kind = 'module'
               AND (?2 IS NULL OR s.language = ?2)
             ORDER BY s.file_path",
        )?;
        let rows = stmt.query_map(params![path, language], |r| r.get::<_, String>(0))?;
        rows.collect()
    }

    /// Back-compat name — defaults to leaf-name lookup. No language filter.
    pub fn imported_by(&self, name: &str) -> SqlResult<Vec<String>> {
        self.imported_by_name(name, None)
    }

    /// Total symbol count (for status checks).
    pub fn symbol_count(&self) -> SqlResult<u32> {
        let count: u32 = self
            .conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))?;
        Ok(count)
    }

    pub fn file_count(&self) -> SqlResult<u32> {
        let count: u32 = self
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        Ok(count)
    }

    /// How many non-module symbols share this exact name — the over-approximation
    /// factor for name-resolved edges (`callers` / `impact`). High = results pool
    /// call sites across many same-named definitions.
    pub fn definition_count(&self, name: &str) -> SqlResult<u32> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM symbols WHERE name = ?1 AND kind != 'module'",
            [name],
            |r| r.get(0),
        )
    }

    fn row_to_symbol(r: &rusqlite::Row) -> SqlResult<Symbol> {
        Ok(Symbol {
            id: r.get(0)?,
            name: r.get(1)?,
            kind: r.get(2)?,
            file_path: r.get(3)?,
            line_start: r.get(4)?,
            line_end: r.get(5)?,
            signature: r.get(6)?,
            parent_id: r.get(7)?,
            decorators: r.get(8)?,
        })
    }

    /// Append a scratchpad entry. Returns the inserted row id + unix timestamp.
    /// `kind` is freeform, conventionally `intent`/`note`/`handoff`/`risk`. The
    /// ≤ 8 KiB `body` bound is enforced by the caller (MCP layer); Store accepts
    /// whatever it's passed.
    pub fn scratchpad_append(
        &mut self,
        agent: &str,
        kind: &str,
        body: &str,
    ) -> SqlResult<(i64, i64)> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn.execute(
            "INSERT INTO scratchpad(ts, agent, kind, body) VALUES (?1, ?2, ?3, ?4)",
            params![ts, agent, kind, body],
        )?;
        Ok((self.conn.last_insert_rowid(), ts))
    }

    /// Read scratchpad entries, newest first. All filters optional.
    pub fn scratchpad_read(
        &self,
        since_ts: Option<i64>,
        agent: Option<&str>,
        kind: Option<&str>,
        limit: u32,
    ) -> SqlResult<Vec<ScratchpadEntry>> {
        let mut sql = String::from("SELECT id, ts, agent, kind, body FROM scratchpad WHERE 1=1");
        let mut params_dyn: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(ts) = since_ts {
            sql.push_str(" AND ts >= ?");
            params_dyn.push(Box::new(ts));
        }
        if let Some(a) = agent {
            sql.push_str(" AND agent = ?");
            params_dyn.push(Box::new(a.to_string()));
        }
        if let Some(k) = kind {
            sql.push_str(" AND kind = ?");
            params_dyn.push(Box::new(k.to_string()));
        }
        sql.push_str(" ORDER BY ts DESC, id DESC LIMIT ?");
        params_dyn.push(Box::new(limit as i64));

        let bound: Vec<&dyn rusqlite::ToSql> = params_dyn.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(bound), |r| {
                Ok(ScratchpadEntry {
                    id: r.get(0)?,
                    ts: r.get(1)?,
                    agent: r.get(2)?,
                    kind: r.get(3)?,
                    body: r.get(4)?,
                })
            })?
            .collect::<SqlResult<Vec<_>>>()?;
        Ok(rows)
    }
}

/// Tarjan's SCC algorithm — iterative form to avoid stack overflow on deep
/// import chains. O(V + E).
///
/// Returns every SCC including singletons; `dependency_cycles()` filters by
/// `min_size`. Nodes within an SCC are sorted lexicographically for determinism.
fn tarjan_scc(adj: &std::collections::BTreeMap<String, Vec<String>>) -> Vec<Vec<String>> {
    use std::collections::HashMap;

    // Intern node names → dense usize ids. Include edge-target-only nodes (no
    // outgoing edges but still belong to SCCs).
    let mut all_names: Vec<&str> = adj.keys().map(|s| s.as_str()).collect();
    for vs in adj.values() {
        for v in vs {
            all_names.push(v.as_str());
        }
    }
    all_names.sort();
    all_names.dedup();

    let mut name_to_id: HashMap<&str, usize> = HashMap::new();
    let mut id_to_name: Vec<&str> = Vec::with_capacity(all_names.len());
    for name in &all_names {
        name_to_id.insert(*name, id_to_name.len());
        id_to_name.push(*name);
    }
    let n = id_to_name.len();

    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (from, tos) in adj {
        let f = name_to_id[from.as_str()];
        for to in tos {
            succ[f].push(name_to_id[to.as_str()]);
        }
    }

    // Iterative Tarjan via explicit work stack.
    let mut index: Vec<i64> = vec![-1; n];
    let mut lowlink: Vec<i64> = vec![0; n];
    let mut on_stack: Vec<bool> = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut next_index: i64 = 0;
    let mut sccs: Vec<Vec<String>> = Vec::new();

    // Work stack: each frame is a node plus its successor-iteration position.
    enum Action {
        Enter(usize),
        Resume(usize, usize),
    }
    let mut work: Vec<Action> = Vec::new();

    for start in 0..n {
        if index[start] >= 0 {
            continue;
        }
        work.push(Action::Enter(start));
        while let Some(action) = work.pop() {
            match action {
                Action::Enter(v) => {
                    index[v] = next_index;
                    lowlink[v] = next_index;
                    next_index += 1;
                    stack.push(v);
                    on_stack[v] = true;
                    work.push(Action::Resume(v, 0));
                }
                Action::Resume(v, i) => {
                    if i < succ[v].len() {
                        let w = succ[v][i];
                        // Re-queue at i+1 to resume after w finishes.
                        work.push(Action::Resume(v, i + 1));
                        if index[w] < 0 {
                            work.push(Action::Enter(w));
                        } else if on_stack[w] {
                            lowlink[v] = lowlink[v].min(index[w]);
                        }
                    } else {
                        // Successors done — propagate lowlink to parent (the next
                        // Resume on the work stack, if any).
                        if let Some(Action::Resume(parent, _)) = work.last() {
                            let p = *parent;
                            lowlink[p] = lowlink[p].min(lowlink[v]);
                        }
                        if lowlink[v] == index[v] {
                            let mut component: Vec<String> = Vec::new();
                            loop {
                                let w = stack.pop().expect("stack non-empty");
                                on_stack[w] = false;
                                component.push(id_to_name[w].to_string());
                                if w == v {
                                    break;
                                }
                            }
                            component.sort();
                            sccs.push(component);
                        }
                    }
                }
            }
        }
    }
    sccs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::env;
    #[cfg(unix)]
    use std::process::Command;

    /// Unique path per test — parallel tests can't share the file.
    fn tmp_db(test_name: &str) -> PathBuf {
        let mut p = env::temp_dir();
        p.push(format!("mmcg-test-{}-{}.db", std::process::id(), test_name));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn directory_bytes(path: &Path) -> BTreeMap<std::ffi::OsString, Vec<u8>> {
        std::fs::read_dir(path)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), std::fs::read(entry.path()).unwrap())
            })
            .collect()
    }

    #[test]
    fn largest_files_rank_by_size_and_honor_production() {
        let path = tmp_db("largest_files");
        let store = Store::open(&path).unwrap();
        for f in ["src/god.rs", "src/small.rs", "tests/big_test.rs"] {
            store.upsert_file(f, 1, 1).unwrap();
        }
        store
            .insert_symbol("g", "function", "src/god.rs", 1, 900, None, None)
            .unwrap();
        store
            .insert_symbol("s", "function", "src/small.rs", 1, 20, None, None)
            .unwrap();
        store
            .insert_symbol("t", "function", "tests/big_test.rs", 1, 2000, None, None)
            .unwrap();

        let all = store.largest_files(false, 10).unwrap();
        assert_eq!(all[0].file, "tests/big_test.rs");
        assert_eq!(all[0].lines, 2000);
        assert!(all.iter().any(|r| r.file == "src/god.rs" && r.lines == 900));

        let prod = store.largest_files(true, 10).unwrap();
        assert_eq!(prod[0].file, "src/god.rs");
        assert!(!prod.iter().any(|r| r.file == "tests/big_test.rs"));

        assert_eq!(store.largest_files(false, 1).unwrap().len(), 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn file_in_degrees_rank_by_incoming_edges_and_honor_production() {
        let path = tmp_db("file_in_degrees");
        let store = Store::open(&path).unwrap();
        for f in ["src/core.rs", "src/caller.rs", "tests/helper.rs"] {
            store.upsert_file(f, 1, 1).unwrap();
        }
        let hub = store
            .insert_symbol("hub", "function", "src/core.rs", 1, 2, None, None)
            .unwrap();
        let helper = store
            .insert_symbol("helper", "function", "tests/helper.rs", 1, 2, None, None)
            .unwrap();
        let a = store
            .insert_symbol("a", "function", "src/caller.rs", 3, 4, None, None)
            .unwrap();
        let b = store
            .insert_symbol("b", "function", "src/caller.rs", 5, 6, None, None)
            .unwrap();
        store.insert_edge(a, Some(hub), "hub", "calls", 3).unwrap();
        store.insert_edge(b, Some(hub), "hub", "calls", 5).unwrap();
        store
            .insert_edge(a, Some(helper), "helper", "calls", 4)
            .unwrap();

        let all = store.file_in_degrees(false, 10).unwrap();
        assert_eq!(all[0].file, "src/core.rs");
        assert_eq!(all[0].in_degree, 2);
        assert!(all
            .iter()
            .any(|r| r.file == "tests/helper.rs" && r.in_degree == 1));

        let prod = store.file_in_degrees(true, 10).unwrap();
        assert!(prod.iter().any(|r| r.file == "src/core.rs"));
        assert!(!prod.iter().any(|r| r.file == "tests/helper.rs"));

        assert_eq!(store.file_in_degrees(false, 1).unwrap().len(), 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn file_in_degrees_preaggregation_matches_reference_semantics() {
        for seed in 0..12u64 {
            let path = tmp_db(&format!("file_in_degrees_equiv_{seed}"));
            let store = Store::open(&path).unwrap();
            let (symbols, edges) = gen_calls_graph(seed, 24, 40);
            seed_calls_graph(&store, &symbols, &edges);

            for production_only in [false, true] {
                let actual = store.file_in_degrees(production_only, 1000).unwrap();
                let mut expected = BTreeMap::<String, u32>::new();
                for symbol in &symbols {
                    if symbol.kind == "module"
                        || production_only && production_excluded(&symbol.file_path)
                    {
                        continue;
                    }
                    for edge in &edges {
                        if edge.to_name == symbol.name {
                            *expected.entry(symbol.file_path.clone()).or_default() += 1;
                        }
                        if edge.to_type.as_deref() == Some(symbol.name.as_str()) {
                            *expected.entry(symbol.file_path.clone()).or_default() += 1;
                        }
                    }
                }
                let mut expected = expected.into_iter().collect::<Vec<_>>();
                expected
                    .sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
                let actual = actual
                    .into_iter()
                    .map(|row| (row.file, row.in_degree))
                    .collect::<Vec<_>>();
                assert_eq!(
                    actual, expected,
                    "seed {seed} production_only={production_only}"
                );
            }
            std::fs::remove_file(&path).ok();
        }
    }

    #[test]
    fn schema_initializes() {
        let path = tmp_db("schema_initializes");
        let store = Store::open(&path).unwrap();
        assert_eq!(store.symbol_count().unwrap(), 0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn read_only_legacy_semantic_source_fails_closed_without_identity_column() {
        let path = tmp_db("legacy_semantic_source");
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE semantic_sources (
                        id INTEGER PRIMARY KEY,
                        tool_name TEXT NOT NULL,
                        tool_version TEXT NOT NULL,
                        project_root TEXT NOT NULL,
                        artifact_path TEXT NOT NULL,
                        artifact_sha256 TEXT NOT NULL,
                        imported_at INTEGER NOT NULL,
                        document_count INTEGER NOT NULL,
                        definition_count INTEGER NOT NULL,
                        edge_count INTEGER NOT NULL,
                        text_verified_documents INTEGER NOT NULL
                    );
                    INSERT INTO semantic_sources VALUES
                        (1, 'legacy', '1', '/repo', '/tmp/index.scip', 'sha', 1, 1, 1, 1, 1);",
                )
                .unwrap();
        }
        let store = Store::open_read_only(&path).unwrap();
        let source = store.semantic_source().unwrap().unwrap();
        assert!(!source.repository_verified);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn read_only_open_never_creates_or_mutates_an_index() {
        let missing = tmp_db("read_only_missing");
        assert!(Store::open_read_only(&missing).is_err());
        assert!(!missing.exists());

        let path = tmp_db("read_only_existing");
        {
            let store = Store::open(&path).unwrap();
            store
                .insert_symbol("existing", "function", "src/lib.rs", 1, 2, None, None)
                .unwrap();
        }

        let read_only = Store::open_read_only(&path).unwrap();
        let query_only: i64 = read_only
            .conn
            .query_row("PRAGMA query_only", [], |row| row.get(0))
            .unwrap();
        assert_eq!(query_only, 1);
        assert_eq!(read_only.symbol_count().unwrap(), 1);
        assert!(read_only
            .insert_symbol("forbidden", "function", "src/lib.rs", 3, 4, None, None)
            .is_err());
        assert_eq!(read_only.symbol_count().unwrap(), 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn private_writable_snapshot_isolated_from_read_only_source() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("index.db");
        {
            let source = Store::open(&path).unwrap();
            source.set_meta("temporal_probe", "source").unwrap();
        }
        let before = directory_bytes(directory.path());
        let source = Store::open_read_only(&path).unwrap();

        let snapshot = source.private_writable_snapshot().unwrap();
        snapshot.set_meta("temporal_probe", "snapshot").unwrap();

        assert_eq!(
            source.meta_value("temporal_probe").unwrap().as_deref(),
            Some("source")
        );
        assert_eq!(
            snapshot.meta_value("temporal_probe").unwrap().as_deref(),
            Some("snapshot")
        );
        assert!(!snapshot.db_path().starts_with(directory.path()));
        assert_eq!(directory_bytes(directory.path()), before);
    }

    #[test]
    fn private_temporal_snapshot_adds_concepts_only_to_old_v7_clone_and_stays_dirty() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("old-v7.db");
        {
            let source = Store::open(&path).unwrap();
            source
                .insert_symbol("legacy", "function", "src/lib.rs", 1, 2, None, None)
                .unwrap();
            source.upsert_file("src/lib.rs", 1, 1).unwrap();
            source.conn.execute_batch(CONCEPT_SCHEMA_DROP_SQL).unwrap();
            source
                .conn
                .execute(
                    "DELETE FROM meta WHERE key = ?1",
                    [CONCEPT_NORMALIZATION_META_KEY],
                )
                .unwrap();
            assert!(!source.concept_schema_objects_current().unwrap());
        }
        let before = directory_bytes(directory.path());
        let source = Store::open_read_only(&path).unwrap();
        let snapshot = source.private_writable_snapshot().unwrap();

        assert!(snapshot.concept_schema_objects_current().unwrap());
        assert!(!snapshot.concept_contract_current().unwrap());
        snapshot.purge_file("src/lib.rs").unwrap();
        assert_eq!(snapshot.symbol_count().unwrap(), 0);
        assert!(!snapshot.concept_contract_current().unwrap());
        assert_eq!(source.symbol_count().unwrap(), 1);
        assert!(!source.concept_schema_objects_current().unwrap());
        assert_eq!(directory_bytes(directory.path()), before);
    }

    #[test]
    fn read_only_open_handles_uri_reserved_filename_characters() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("index # %.db");
        {
            let store = Store::open(&path).unwrap();
            store
                .insert_symbol("existing", "function", "src/lib.rs", 1, 2, None, None)
                .unwrap();
        }
        let before = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<BTreeSet<_>>();

        let read_only = Store::open_read_only(&path).unwrap();
        assert_eq!(read_only.symbol_count().unwrap(), 1);
        drop(read_only);

        let after = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<BTreeSet<_>>();
        assert_eq!(after, before);
    }

    #[test]
    fn read_only_open_sees_an_active_wal_without_touching_its_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("active.db");
        let writer = Store::open(&path).unwrap();
        writer
            .insert_symbol("in_wal", "function", "src/lib.rs", 1, 2, None, None)
            .unwrap();
        let before = directory_bytes(directory.path());

        let read_only = Store::open_read_only(&path).unwrap();
        assert!(read_only
            .search_symbols("in_wal", None, None)
            .unwrap()
            .iter()
            .any(|symbol| symbol.name == "in_wal"));
        drop(read_only);

        assert_eq!(directory_bytes(directory.path()), before);
        drop(writer);
    }

    #[cfg(unix)]
    #[test]
    fn read_only_open_rejects_a_symlinked_database() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("real.db");
        let alias = directory.path().join("alias.db");
        let writer = Store::open(&path).unwrap();
        writer
            .insert_symbol(
                "only_in_target_wal",
                "function",
                "src/lib.rs",
                1,
                2,
                None,
                None,
            )
            .unwrap();
        symlink(&path, &alias).unwrap();
        let before = directory_bytes(directory.path());

        assert!(Store::open_read_only(&alias).is_err());

        assert_eq!(directory_bytes(directory.path()), before);
        drop(writer);
    }

    #[test]
    fn managed_serve_open_authorizes_only_the_repository_index() {
        let root = tempfile::tempdir().unwrap();
        let expected = root.path().join(".mastermind/mmcg.db");
        let managed = Store::open_for_serve(&expected, Some(root.path())).unwrap();
        let canonical_root = root.path().canonicalize().unwrap();

        assert_eq!(managed.managed_root(), Some(canonical_root.as_path()));
        assert_eq!(managed.serve_root(), Some(canonical_root.as_path()));
        assert!(managed.schema_current().unwrap());
        assert!(expected.is_file());
    }

    #[test]
    fn managed_serve_open_preserves_mismatched_existing_database() {
        let root = tempfile::tempdir().unwrap();
        let unrelated = tempfile::tempdir().unwrap();
        let state = root.path().join(".mastermind");
        std::fs::create_dir(&state).unwrap();
        let expected = state.join("mmcg.db");
        {
            let source = Store::open(&expected).unwrap();
            source
                .set_meta(
                    "index_root",
                    unrelated.path().canonicalize().unwrap().to_str().unwrap(),
                )
                .unwrap();
            source
                .insert_symbol("preserved", "function", "src/lib.rs", 1, 2, None, None)
                .unwrap();
        }
        let before = directory_bytes(&state);

        let served = Store::open_for_serve(&expected, Some(root.path())).unwrap();
        let canonical_root = root.path().canonicalize().unwrap();
        assert!(served.managed_root().is_none());
        assert_eq!(served.serve_root(), Some(canonical_root.as_path()));
        assert_eq!(served.symbol_count().unwrap(), 1);
        assert!(served
            .insert_symbol("forbidden", "function", "src/lib.rs", 3, 4, None, None)
            .is_err());
        drop(served);

        assert_eq!(directory_bytes(&state), before);
    }

    #[cfg(unix)]
    #[test]
    fn managed_serve_open_rejects_symlinked_state_and_database() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        symlink(victim.path(), root.path().join(".mastermind")).unwrap();
        let expected = root.path().join(".mastermind/mmcg.db");
        assert!(Store::open_for_serve(&expected, Some(root.path())).is_err());
        assert!(std::fs::read_dir(victim.path()).unwrap().next().is_none());

        let root = tempfile::tempdir().unwrap();
        let state = root.path().join(".mastermind");
        std::fs::create_dir(&state).unwrap();
        let victim_path = victim.path().join("victim.db");
        {
            let victim_store = Store::open(&victim_path).unwrap();
            victim_store.set_meta("sentinel", "unchanged").unwrap();
        }
        let before = std::fs::read(&victim_path).unwrap();
        let expected = state.join("mmcg.db");
        symlink(&victim_path, &expected).unwrap();
        assert!(Store::open_for_serve(&expected, Some(root.path())).is_err());
        assert_eq!(std::fs::read(&victim_path).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn managed_serve_open_rejects_fifo_without_blocking() {
        use std::os::unix::fs::FileTypeExt;

        let root = tempfile::tempdir().unwrap();
        let state = root.path().join(".mastermind");
        std::fs::create_dir(&state).unwrap();
        let expected = state.join("mmcg.db");
        assert!(Command::new("mkfifo")
            .arg(&expected)
            .status()
            .unwrap()
            .success());
        assert!(std::fs::symlink_metadata(&expected)
            .unwrap()
            .file_type()
            .is_fifo());

        let started = Instant::now();
        assert!(Store::open_for_serve(&expected, Some(root.path())).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn private_snapshot_copy_enforces_size_and_deadline_bounds() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bounded.db");
        let writer = Store::open(&path).unwrap();
        writer
            .insert_symbol("in_wal", "function", "src/lib.rs", 1, 2, None, None)
            .unwrap();
        let before = directory_bytes(directory.path());

        let too_large = copy_index_snapshot(
            &path,
            SnapshotCopyBudget {
                max_bytes: 1,
                deadline: Instant::now() + Duration::from_secs(1),
            },
        )
        .unwrap_err();
        assert_eq!(
            too_large.sqlite_error_code(),
            Some(rusqlite::ErrorCode::TooBig)
        );

        let timed_out = copy_index_snapshot(
            &path,
            SnapshotCopyBudget {
                max_bytes: u64::MAX,
                deadline: Instant::now(),
            },
        )
        .unwrap_err();
        assert_eq!(
            timed_out.sqlite_error_code(),
            Some(rusqlite::ErrorCode::OperationInterrupted)
        );

        let timed_out_open = match Store::open_read_only_with_deadline(&path, Some(Instant::now()))
        {
            Ok(_) => panic!("an expired request deadline must stop the WAL snapshot copy"),
            Err(error) => error,
        };
        assert_eq!(
            timed_out_open.sqlite_error_code(),
            Some(rusqlite::ErrorCode::OperationInterrupted)
        );
        assert_eq!(directory_bytes(directory.path()), before);
        drop(writer);
    }

    #[test]
    fn windows_immutable_uri_paths_reject_unc_and_normalize_drives() {
        assert_eq!(
            String::from_utf8(windows_sqlite_uri_path(r"C:\repo\mmcg.db").unwrap()).unwrap(),
            "/C:/repo/mmcg.db"
        );
        assert_eq!(
            String::from_utf8(windows_sqlite_uri_path(r"\\?\C:\repo\mmcg.db").unwrap()).unwrap(),
            "/C:/repo/mmcg.db"
        );
        assert!(windows_sqlite_uri_path(r"\\server\share\mmcg.db").is_none());
        assert!(windows_sqlite_uri_path(r"\\?\UNC\server\share\mmcg.db").is_none());
    }

    #[test]
    fn schema_rebuild_preserves_repository_identity_history_and_scratchpad() {
        let path = tmp_db("schema_rebuild_identity");
        {
            let mut store = Store::open(&path).unwrap();
            store.set_meta("index_root", "/tmp/example-repo").unwrap();
            store
                .replace_project_history(&[ProjectHistoryEntry {
                    path: "CONTEXT.md".into(),
                    kind: "context".into(),
                    title: "Context".into(),
                    body: "durable history".into(),
                }])
                .unwrap();
            store
                .scratchpad_append("planner", "handoff", "live note")
                .unwrap();
            store.upsert_file("src/lib.rs", 1, 1).unwrap();
            store
                .insert_symbol("entry", "function", "src/lib.rs", 1, 2, None, None)
                .unwrap();
            store.set_meta("schema_version", "6").unwrap();
        }

        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.meta_value("schema_version").unwrap().as_deref(),
            Some(SCHEMA_VERSION)
        );
        assert_eq!(
            store.meta_value("index_root").unwrap().as_deref(),
            Some("/tmp/example-repo")
        );
        assert_eq!(store.file_count().unwrap(), 0);
        assert_eq!(store.symbol_count().unwrap(), 0);
        assert_eq!(store.project_history_count().unwrap(), 1);
        assert_eq!(
            store.scratchpad_read(None, None, None, 10).unwrap().len(),
            1
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn insert_and_search() {
        let path = tmp_db("insert_and_search");
        let store = Store::open(&path).unwrap();
        let id = store
            .insert_symbol("foo", "function", "a.py", 1, 5, Some("def foo()"), None)
            .unwrap();
        assert!(id > 0);

        let found = store.search_symbols("foo", None, None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "foo");
        assert_eq!(found[0].kind, "function");

        let none = store.search_symbols("bar", None, None).unwrap();
        assert!(none.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn definition_count_counts_same_named_non_module_defs() {
        let path = tmp_db("definition_count");
        let store = Store::open(&path).unwrap();
        store
            .insert_symbol("get", "method", "a.rs", 1, 2, None, None)
            .unwrap();
        store
            .insert_symbol("get", "method", "b.rs", 1, 2, None, None)
            .unwrap();
        store
            .insert_symbol("unique", "function", "c.rs", 1, 2, None, None)
            .unwrap();
        assert_eq!(store.definition_count("get").unwrap(), 2);
        assert_eq!(store.definition_count("unique").unwrap(), 1);
        assert_eq!(store.definition_count("missing").unwrap(), 0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn callers_and_callees() {
        let path = tmp_db("callers_and_callees");
        let store = Store::open(&path).unwrap();
        let foo = store
            .insert_symbol("foo", "function", "a.py", 1, 5, None, None)
            .unwrap();
        let bar = store
            .insert_symbol("bar", "function", "a.py", 10, 15, None, None)
            .unwrap();
        // foo calls bar
        store
            .insert_edge(foo, Some(bar), "bar", "calls", 3)
            .unwrap();

        let callers = store.callers_of("bar", None, None).unwrap();
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].name, "foo");

        let callees = store.callees_of(foo, None).unwrap();
        assert_eq!(callees.len(), 1);
        assert_eq!(callees[0].0, "bar");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn edge_kind_filter() {
        let path = tmp_db("edge_kind_filter");
        let store = Store::open(&path).unwrap();
        let module = store
            .insert_symbol("<module>", "module", "x.py", 1, 1, None, None)
            .unwrap();
        let caller = store
            .insert_symbol("caller_fn", "function", "x.py", 10, 15, None, None)
            .unwrap();
        // Module imports `target`; caller_fn calls it. Same to_name, different kinds.
        store
            .insert_edge(module, None, "target", "imports", 2)
            .unwrap();
        store
            .insert_edge(caller, None, "target", "calls", 12)
            .unwrap();

        // Default (None) → 'calls' only — finds caller_fn, not module.
        let default_callers = store.callers_of("target", None, None).unwrap();
        assert_eq!(default_callers.len(), 1);
        assert_eq!(default_callers[0].name, "caller_fn");

        // edge_kind = 'imports' — finds module, not caller_fn.
        let import_callers = store.callers_of("target", None, Some("imports")).unwrap();
        assert_eq!(import_callers.len(), 1);
        assert_eq!(import_callers[0].name, "<module>");

        // callees: caller_fn calls target via 'calls'; module imports target via 'imports'
        let caller_callees = store.callees_of(caller, None).unwrap();
        assert_eq!(caller_callees.len(), 1);
        assert_eq!(caller_callees[0].0, "target");

        let module_imports = store.callees_of(module, Some("imports")).unwrap();
        assert_eq!(module_imports.len(), 1);
        assert_eq!(module_imports[0].0, "target");

        // module has no 'calls' edges
        let module_calls = store.callees_of(module, None).unwrap();
        assert!(module_calls.is_empty());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn unreferenced_excludes_decorated_and_tests() {
        let path = tmp_db("unreferenced_excludes_decorated");
        let store = Store::open(&path).unwrap();
        // Direct insert — insert_symbol can't set the decorators column.
        // 3 functions, none called by anything.
        let conn = &store.conn;
        conn.execute(
            "INSERT INTO symbols(name, kind, file_path, line_start, line_end, signature, parent_id, language, decorators)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params!["plain_dead", "function", "src/lib.py", 1, 3, None::<&str>, None::<i64>, "python", None::<&str>],
        ).unwrap();
        conn.execute(
            "INSERT INTO symbols(name, kind, file_path, line_start, line_end, signature, parent_id, language, decorators)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params!["db", "function", "src/fixtures.py", 10, 12, None::<&str>, None::<i64>, "python", ",pytest.fixture,"],
        ).unwrap();
        conn.execute(
            "INSERT INTO symbols(name, kind, file_path, line_start, line_end, signature, parent_id, language, decorators)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params!["test_foo", "function", "tests/test_lib.py", 5, 7, None::<&str>, None::<i64>, "python", None::<&str>],
        ).unwrap();

        let unref = store.unreferenced(None, None).unwrap();
        let names: Vec<&str> = unref.iter().map(|s| s.name.as_str()).collect();
        // plain_dead survives — unreferenced, no decorator, no test pattern.
        assert!(
            names.contains(&"plain_dead"),
            "plain_dead is genuinely unreferenced"
        );
        // db filtered — pytest.fixture decorator.
        assert!(
            !names.contains(&"db"),
            "db is filtered by @pytest.fixture decorator"
        );
        // test_foo filtered — test_* in test file.
        assert!(
            !names.contains(&"test_foo"),
            "test_foo is filtered by pytest convention"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn unreferenced_excludes_called() {
        let path = tmp_db("unreferenced_excludes_called");
        let store = Store::open(&path).unwrap();
        let _module = store
            .insert_symbol("<module>", "module", "x.py", 1, 1, None, None)
            .unwrap();
        let foo = store
            .insert_symbol("foo", "function", "x.py", 5, 10, None, None)
            .unwrap();
        let _bar = store
            .insert_symbol("bar", "function", "x.py", 12, 16, None, None)
            .unwrap();
        let _orphan = store
            .insert_symbol("orphan", "function", "x.py", 20, 22, None, None)
            .unwrap();
        // foo calls bar — bar referenced; foo and orphan have no incoming edges.
        store.insert_edge(foo, None, "bar", "calls", 7).unwrap();

        let unref = store.unreferenced(None, None).unwrap();
        let names: Vec<&str> = unref.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"foo"),
            "foo has no callers — should be unreferenced"
        );
        assert!(names.contains(&"orphan"), "orphan never referenced");
        assert!(
            !names.contains(&"bar"),
            "bar is called by foo — should NOT be unreferenced"
        );
        assert!(
            !names.contains(&"<module>"),
            "module pseudo-symbols excluded"
        );

        // Filter by kind
        let funcs_only = store.unreferenced(Some("function"), None).unwrap();
        assert_eq!(funcs_only.len(), 2); // foo, orphan
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn bounded_unreferenced_applies_scope_production_and_limit_in_sql() {
        let path = tmp_db("bounded_unreferenced");
        let store = Store::open(&path).unwrap();
        for (name, file) in [
            ("alpha", "src/core/a.rs"),
            ("beta", "src/core/b.rs"),
            ("gamma", "src/core/c.rs"),
            ("fixture_one", "src/core/tests/one.rs"),
            ("fixture_two", "src/core/fixtures/two.rs"),
            ("outside", "src/other.rs"),
        ] {
            store
                .insert_symbol(name, "function", file, 1, 2, None, None)
                .unwrap();
        }

        let (all_total, all_rows) = store
            .unreferenced_bounded(None, None, "src/core", "directory", false, 2)
            .unwrap();
        assert_eq!(all_total, 5);
        assert_eq!(all_rows.len(), 2);

        let (production_total, production_rows) = store
            .unreferenced_bounded(None, None, "src/core", "directory", true, 2)
            .unwrap();
        assert_eq!(production_total, 3);
        assert_eq!(production_rows.len(), 2);
        assert!(production_rows
            .iter()
            .all(|symbol| symbol.file_path.starts_with("src/core/")
                && is_production_path(&symbol.file_path)));
        let (zero_limit_total, zero_limit_rows) = store
            .unreferenced_bounded(None, None, "src/core", "directory", false, 0)
            .unwrap();
        assert_eq!(zero_limit_total, all_total);
        assert!(zero_limit_rows.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn api_surface_external_only() {
        let path = tmp_db("api_surface_external_only");
        let store = Store::open(&path).unwrap();
        // pub_fn in src/api/, called from src/main.rs (OUTSIDE prefix).
        let main_mod = store
            .insert_symbol("<module>", "module", "src/main.rs", 1, 1, None, None)
            .unwrap();
        let _pub_fn = store
            .insert_symbol("pub_fn", "function", "src/api/lib.rs", 3, 5, None, None)
            .unwrap();
        store
            .insert_edge(main_mod, None, "pub_fn", "calls", 10)
            .unwrap();

        // internal_fn in src/api/, called only from src/api/util.rs (INSIDE prefix).
        let util_mod = store
            .insert_symbol("<module>", "module", "src/api/util.rs", 1, 1, None, None)
            .unwrap();
        let _internal_fn = store
            .insert_symbol(
                "internal_fn",
                "function",
                "src/api/lib.rs",
                8,
                10,
                None,
                None,
            )
            .unwrap();
        store
            .insert_edge(util_mod, None, "internal_fn", "calls", 5)
            .unwrap();

        let surface = store.api_surface("src/api/", None).unwrap();
        let names: Vec<&str> = surface.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"pub_fn"),
            "called from src/main.rs (outside prefix)"
        );
        assert!(
            !names.contains(&"internal_fn"),
            "only called from inside src/api/"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn impact_transitive() {
        let path = tmp_db("impact_transitive");
        let store = Store::open(&path).unwrap();
        // a -> b -> c
        let a = store
            .insert_symbol("a", "function", "x.py", 1, 5, None, None)
            .unwrap();
        let b = store
            .insert_symbol("b", "function", "x.py", 10, 15, None, None)
            .unwrap();
        let c = store
            .insert_symbol("c", "function", "x.py", 20, 25, None, None)
            .unwrap();
        store.insert_edge(a, Some(b), "b", "calls", 3).unwrap();
        store.insert_edge(b, Some(c), "c", "calls", 12).unwrap();

        // impact of c should include b (depth 1) and a (depth 2).
        let imp = store
            .impact_of_many(&["c".to_string()], 5, 5001, None)
            .unwrap();
        let names: Vec<&str> = imp.iter().map(|row| row.symbol.name.as_str()).collect();
        assert!(names.contains(&"b"));
        assert!(names.contains(&"a"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn centrality_ranks_by_in_degree() {
        let path = tmp_db("centrality_basic");
        let store = Store::open(&path).unwrap();
        // popular has 3 distinct callers; medium has 1; lonely has 0.
        let popular = store
            .insert_symbol("popular", "function", "x.py", 1, 5, None, None)
            .unwrap();
        let medium = store
            .insert_symbol("medium", "function", "x.py", 10, 15, None, None)
            .unwrap();
        let _lonely = store
            .insert_symbol("lonely", "function", "x.py", 20, 25, None, None)
            .unwrap();
        let c1 = store
            .insert_symbol("c1", "function", "x.py", 30, 35, None, None)
            .unwrap();
        let c2 = store
            .insert_symbol("c2", "function", "x.py", 40, 45, None, None)
            .unwrap();
        let c3 = store
            .insert_symbol("c3", "function", "x.py", 50, 55, None, None)
            .unwrap();
        // Same caller twice → still in_degree=1 (DISTINCT callers).
        store
            .insert_edge(c1, Some(popular), "popular", "calls", 31)
            .unwrap();
        store
            .insert_edge(c1, Some(popular), "popular", "calls", 32)
            .unwrap();
        store
            .insert_edge(c2, Some(popular), "popular", "calls", 41)
            .unwrap();
        store
            .insert_edge(c3, Some(popular), "popular", "calls", 51)
            .unwrap();
        store
            .insert_edge(c1, Some(medium), "medium", "calls", 33)
            .unwrap();

        let ranked = store.centrality(None, None, None, 10).unwrap();
        let by_name: std::collections::HashMap<&str, u32> = ranked
            .iter()
            .map(|(s, deg, _coll)| (s.name.as_str(), *deg))
            .collect();
        assert_eq!(
            by_name["popular"], 3,
            "3 distinct callers, dup call ignored"
        );
        assert_eq!(by_name["medium"], 1);
        assert!(ranked.iter().all(|(_, _, coll)| *coll == 1));
        // lonely has zero callers — excluded by the JOIN.
        assert!(!by_name.contains_key("lonely"));
        // popular ranks above medium.
        assert_eq!(ranked[0].0.name, "popular");

        // top=1 returns only the top symbol.
        let top1 = store.centrality(None, None, None, 1).unwrap();
        assert_eq!(top1.len(), 1);
        assert_eq!(top1[0].0.name, "popular");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn task_specs_full_text_search() {
        let path = tmp_db("task_specs_fts");
        let mut store = Store::open(&path).unwrap();
        let entries = vec![
            TaskSpecEntry {
                path: ".mastermind/tasks/001-rate-limiter/spec.md".into(),
                title: "Add rate limiter to API".into(),
                body: "We need to rate-limit POST /api/orders. \
                       Token bucket with Redis backing."
                    .into(),
            },
            TaskSpecEntry {
                path: ".mastermind/tasks/002-cache-invalidation/spec.md".into(),
                title: "Cache invalidation strategy".into(),
                body: "On user update, evict cached user records. \
                       LRU with TTL fallback."
                    .into(),
            },
        ];
        store.replace_task_specs(&entries).unwrap();
        assert_eq!(store.task_specs_count().unwrap(), 2);

        // Single-term query matches body content.
        let hits = store.search_task_specs("rate", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.contains("001-rate-limiter"));
        assert!(hits[0].excerpt.contains("«rate"));

        // Implicit AND — rate AND bucket match the first spec only.
        let combo = store.search_task_specs("rate bucket", 10).unwrap();
        assert_eq!(combo.len(), 1);

        // Stemming: porter maps "invalidate" to the "invalidation" root.
        let stem = store.search_task_specs("invalidate", 10).unwrap();
        assert_eq!(stem.len(), 1);
        assert!(stem[0].path.contains("002-cache-invalidation"));

        // Empty / whitespace query → no results, no FTS5 syntax error.
        assert!(store.search_task_specs("", 10).unwrap().is_empty());
        assert!(store.search_task_specs("   ", 10).unwrap().is_empty());

        // Replace is wholesale — a smaller set wipes the old.
        store.replace_task_specs(&entries[..1]).unwrap();
        assert_eq!(store.task_specs_count().unwrap(), 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn project_history_search_is_filterable_and_wholesale() {
        let path = tmp_db("project_history_fts");
        let mut store = Store::open(&path).unwrap();
        let entries = vec![
            ProjectHistoryEntry {
                path: "CONTEXT.md".into(),
                kind: "context".into(),
                title: "Project context".into(),
                body: "Decision: use an idempotency key at the runtime boundary.".into(),
            },
            ProjectHistoryEntry {
                path: ".mastermind/tasks/_lessons.md".into(),
                kind: "lesson".into(),
                title: "Lessons".into(),
                body: "A prior token bucket attempt failed under clock skew.".into(),
            },
        ];
        store.replace_project_history(&entries).unwrap();
        assert_eq!(store.project_history_count().unwrap(), 2);

        let all = store.search_project_history("token", None, 10).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].kind, "lesson");
        assert_eq!(all[0].matched_terms, ["token"]);

        let stemmed = store
            .search_project_history("idempotent", None, 10)
            .unwrap();
        assert_eq!(stemmed.len(), 1);
        assert_eq!(stemmed[0].matched_terms, ["idempotency"]);

        let filtered = store
            .search_project_history("token", Some("context"), 10)
            .unwrap();
        assert!(filtered.is_empty());
        assert!(store
            .search_project_history("   ", None, 10)
            .unwrap()
            .is_empty());

        store
            .replace_project_history(&[ProjectHistoryEntry {
                path: "CONTEXT.md".into(),
                kind: "context".into(),
                title: "Collision".into(),
                body: "token \u{001e}forged_history_excerpt\u{001f}".into(),
            }])
            .unwrap();
        let collision = store.search_project_history("token", None, 10).unwrap();
        assert_eq!(collision.len(), 1);
        assert!(collision[0].matched_terms.is_empty());

        store.replace_project_history(&entries[..1]).unwrap();
        assert_eq!(store.project_history_count().unwrap(), 1);
        assert!(store
            .search_project_history("clock skew", None, 10)
            .unwrap()
            .is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn tarjan_finds_simple_cycle() {
        // A → B → A : one cycle of size 2
        let mut adj: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        adj.insert("A".into(), vec!["B".into()]);
        adj.insert("B".into(), vec!["A".into()]);
        let sccs = super::tarjan_scc(&adj);
        let cycle: Vec<&Vec<String>> = sccs.iter().filter(|c| c.len() >= 2).collect();
        assert_eq!(cycle.len(), 1);
        assert_eq!(cycle[0], &vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn tarjan_distinguishes_cycles_from_dag() {
        // X → Y → Z + B → C → B (only B,C cycle)
        let mut adj: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        adj.insert("X".into(), vec!["Y".into()]);
        adj.insert("Y".into(), vec!["Z".into()]);
        adj.insert("B".into(), vec!["C".into()]);
        adj.insert("C".into(), vec!["B".into()]);
        let sccs: Vec<Vec<String>> = super::tarjan_scc(&adj)
            .into_iter()
            .filter(|c| c.len() >= 2)
            .collect();
        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0], vec!["B".to_string(), "C".to_string()]);
    }

    #[test]
    fn tarjan_handles_three_cycle() {
        // A → B → C → A
        let mut adj: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        adj.insert("A".into(), vec!["B".into()]);
        adj.insert("B".into(), vec!["C".into()]);
        adj.insert("C".into(), vec!["A".into()]);
        let sccs: Vec<Vec<String>> = super::tarjan_scc(&adj)
            .into_iter()
            .filter(|c| c.len() >= 2)
            .collect();
        assert_eq!(sccs.len(), 1);
        assert_eq!(
            sccs[0],
            vec!["A".to_string(), "B".to_string(), "C".to_string()]
        );
    }

    #[test]
    fn dependency_cycles_end_to_end() {
        let path = tmp_db("dep_cycles");
        let store = Store::open(&path).unwrap();
        // a.py imports `bar` (in b.py); b.py imports `foo` (in a.py) → cycle a.py ↔ b.py.
        // c.py is acyclic (only imports `bar`).
        let a_mod = store
            .insert_symbol("<module>", "module", "a.py", 1, 100, None, None)
            .unwrap();
        let b_mod = store
            .insert_symbol("<module>", "module", "b.py", 1, 100, None, None)
            .unwrap();
        let c_mod = store
            .insert_symbol("<module>", "module", "c.py", 1, 100, None, None)
            .unwrap();
        store
            .insert_symbol("foo", "function", "a.py", 10, 20, None, None)
            .unwrap();
        store
            .insert_symbol("bar", "function", "b.py", 10, 20, None, None)
            .unwrap();

        store.insert_edge(a_mod, None, "bar", "imports", 1).unwrap();
        store.insert_edge(b_mod, None, "foo", "imports", 1).unwrap();
        store.insert_edge(c_mod, None, "bar", "imports", 1).unwrap();

        let (cycles, truncated) = store.dependency_cycles(None, 2).unwrap();
        assert!(!truncated);
        assert_eq!(cycles.len(), 1, "exactly one cycle expected");
        assert_eq!(cycles[0], vec!["a.py".to_string(), "b.py".to_string()]);

        // min_size=3 hides the 2-node cycle entirely.
        let (bigger, bigger_truncated) = store.dependency_cycles(None, 3).unwrap();
        assert!(!bigger_truncated);
        assert!(bigger.is_empty());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn dependency_cycles_resolve_cpp_includes_to_header_files() {
        let path = tmp_db("dep_cycles_cpp_includes");
        let store = Store::open(&path).unwrap();
        let a_module = store
            .insert_symbol("<module>", "module", "src/a.h", 1, 10, None, None)
            .unwrap();
        let b_module = store
            .insert_symbol("<module>", "module", "src/b.h", 1, 10, None, None)
            .unwrap();
        store
            .insert_symbol("A", "class", "src/a.h", 2, 4, None, Some(a_module))
            .unwrap();
        store
            .insert_symbol("B", "class", "src/b.h", 2, 4, None, Some(b_module))
            .unwrap();
        store
            .conn
            .execute("UPDATE symbols SET language = 'cpp'", [])
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO edges(from_id, to_name, to_path, kind, line) VALUES (?1, 'b.h', 'b.h::*', 'imports', 1)",
                params![a_module],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO edges(from_id, to_name, to_path, kind, line) VALUES (?1, 'a.h', 'a.h::*', 'imports', 1)",
                params![b_module],
            )
            .unwrap();

        let (cycles, truncated) = store.dependency_cycles(Some("cpp"), 2).unwrap();
        assert!(!truncated);
        assert_eq!(
            cycles,
            vec![vec!["src/a.h".to_string(), "src/b.h".to_string()]]
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn centrality_filters_prefix_and_kind() {
        let path = tmp_db("centrality_filters");
        let store = Store::open(&path).unwrap();
        let api_fn = store
            .insert_symbol("api_target", "function", "src/api/x.py", 1, 5, None, None)
            .unwrap();
        let core_cls = store
            .insert_symbol("CoreClass", "class", "src/core/x.py", 1, 5, None, None)
            .unwrap();
        let caller = store
            .insert_symbol("c", "function", "src/api/y.py", 1, 5, None, None)
            .unwrap();
        store
            .insert_edge(caller, Some(api_fn), "api_target", "calls", 2)
            .unwrap();
        store
            .insert_edge(caller, Some(core_cls), "CoreClass", "calls", 3)
            .unwrap();

        // Prefix src/api/ excludes the class in src/core/.
        let api_only = store.centrality(Some("src/api/"), None, None, 10).unwrap();
        let names: Vec<&str> = api_only.iter().map(|(s, _, _)| s.name.as_str()).collect();
        assert!(names.contains(&"api_target"));
        assert!(!names.contains(&"CoreClass"));

        // Kind filter: class only.
        let classes = store.centrality(None, None, Some("class"), 10).unwrap();
        let class_names: Vec<&str> = classes.iter().map(|(s, _, _)| s.name.as_str()).collect();
        assert!(class_names.contains(&"CoreClass"));
        assert!(!class_names.contains(&"api_target"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn map_scoped_queries_treat_percent_and_underscore_literally() {
        let path = tmp_db("map_literal_scope");
        let store = Store::open(&path).unwrap();
        for file in [
            "src/%dir/a.rs",
            "src/%directory/b.rs",
            "src/_dir/c.rs",
            "src/xdir/d.rs",
            "src/%file.rs",
            "src/other.rs",
        ] {
            store.upsert_file(file, 1, 1).unwrap();
        }

        assert_eq!(
            store.map_paths("src/%dir", "directory", 10).unwrap(),
            vec!["src/%dir/a.rs"]
        );
        assert_eq!(
            store.map_paths("src/_dir", "directory", 10).unwrap(),
            vec!["src/_dir/c.rs"]
        );
        assert_eq!(
            store.map_paths("src/%file.rs", "file", 10).unwrap(),
            vec!["src/%file.rs"]
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn production_classification_is_shared_by_files_and_symbols() {
        let path = tmp_db("production_classification_shared");
        let store = Store::open(&path).unwrap();
        for file in [
            "src/live.py",
            "src/test_helpers/service.py",
            "src/tests/service.py",
            "src/service_test.py",
        ] {
            store.upsert_file(file, 1, 1).unwrap();
            store
                .insert_symbol("entry", "function", file, 1, 2, None, None)
                .unwrap();
        }

        let files = store.map_paths_filtered("", "root", 10, true).unwrap();
        let symbols = store
            .conn
            .prepare(
                "SELECT DISTINCT file_path FROM symbols WHERE production = 1 ORDER BY file_path",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<SqlResult<Vec<String>>>()
            .unwrap();
        assert_eq!(files, symbols);
        assert_eq!(files, vec!["src/live.py", "src/test_helpers/service.py"]);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn map_boundaries_are_deterministic_and_exact_external_scope() {
        let path = tmp_db("map_boundaries");
        let store = Store::open(&path).unwrap();
        let app_target = store
            .insert_symbol(
                "app_target",
                "function",
                "src/app/lib.rs",
                10,
                12,
                None,
                None,
            )
            .unwrap();
        let core_target = store
            .insert_symbol(
                "core_target",
                "function",
                "src/core/lib.rs",
                20,
                22,
                None,
                None,
            )
            .unwrap();
        let app_internal = store
            .insert_symbol(
                "app_internal",
                "function",
                "src/app/internal.rs",
                1,
                3,
                None,
                None,
            )
            .unwrap();
        let app_sibling = store
            .insert_symbol(
                "app_sibling",
                "function",
                "src/application/caller.rs",
                1,
                3,
                None,
                None,
            )
            .unwrap();
        let core_external = store
            .insert_symbol("core_external", "function", "src/main.rs", 1, 3, None, None)
            .unwrap();
        store
            .insert_edge(app_internal, Some(app_target), "app_target", "calls", 2)
            .unwrap();
        store
            .insert_edge(app_sibling, Some(app_target), "app_target", "calls", 2)
            .unwrap();
        store
            .insert_edge(core_external, Some(core_target), "core_target", "calls", 2)
            .unwrap();

        let rows = store
            .map_boundaries(
                &[
                    MapBoundaryScope {
                        label: "src/app".into(),
                        path: "src/app".into(),
                        match_mode: MapBoundaryMatch::Recursive,
                    },
                    MapBoundaryScope {
                        label: "src/core".into(),
                        path: "src/core".into(),
                        match_mode: MapBoundaryMatch::Recursive,
                    },
                ],
                20,
                400,
            )
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].component, "src/app");
        assert_eq!(rows[0].symbol.name, "app_target");
        assert_eq!(rows[1].component, "src/core");
        assert_eq!(rows[1].symbol.name, "core_target");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn map_boundaries_do_not_scan_unrelated_call_edges() {
        let path = tmp_db("map_boundary_target_indexes");
        let store = Store::open(&path).unwrap();
        let target = store
            .insert_symbol("boundary", "function", "src/app/lib.rs", 1, 3, None, None)
            .unwrap();
        let caller = store
            .insert_symbol("caller", "function", "outside.rs", 1, 3, None, None)
            .unwrap();
        store
            .insert_edge(caller, Some(target), "boundary", "calls", 2)
            .unwrap();
        store.conn.execute_batch("BEGIN").unwrap();
        for _ in 0..20_000 {
            store
                .insert_edge(caller, None, "unrelated", "calls", 3)
                .unwrap();
        }
        store.conn.execute_batch("COMMIT").unwrap();

        let components = [MapBoundaryScope {
            label: "src/app".into(),
            path: "src/app".into(),
            match_mode: MapBoundaryMatch::Recursive,
        }];
        let rows = store
            .with_work_budget(
                WorkBudget {
                    deadline: None,
                    op_ticks: Some(30),
                },
                || store.map_boundaries(&components, 10, 10),
            )
            .expect("boundary lookup must use target indexes");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].symbol.name, "boundary");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn map_boundaries_deduplicate_scoped_names_before_edge_lookup() {
        let path = tmp_db("map_boundary_duplicate_names");
        let store = Store::open(&path).unwrap();
        store.conn.execute_batch("BEGIN").unwrap();
        for index in 0..5_000 {
            store
                .insert_symbol(
                    "shared",
                    "function",
                    &format!("src/app/{index:05}.rs"),
                    1,
                    2,
                    None,
                    None,
                )
                .unwrap();
        }
        store.conn.execute_batch("COMMIT").unwrap();
        let caller = store
            .insert_symbol("caller", "function", "outside.rs", 1, 2, None, None)
            .unwrap();
        store
            .insert_edge(caller, None, "shared", "calls", 1)
            .unwrap();
        let components = [MapBoundaryScope {
            label: "src/app".into(),
            path: "src/app".into(),
            match_mode: MapBoundaryMatch::Recursive,
        }];

        let rows = store
            .with_work_budget(
                WorkBudget {
                    deadline: None,
                    op_ticks: Some(300),
                },
                || store.map_boundaries(&components, 5, 5),
            )
            .expect("duplicate definitions must share one boundary-name lookup");
        assert_eq!(rows.len(), 5);
        assert!(rows.iter().all(|row| row.symbol.name == "shared"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn map_queries_obey_probe_limits_and_name_tie_order() {
        let path = tmp_db("map_probe_limits");
        let store = Store::open(&path).unwrap();
        for file in ["src/a.rs", "src/b.rs", "src/c.rs"] {
            store.upsert_file(file, 1, 1).unwrap();
        }
        assert_eq!(
            store.map_paths("src", "directory", 2).unwrap(),
            vec!["src/a.rs", "src/b.rs"]
        );

        let zed = store
            .insert_symbol("zed", "function", "src/a.rs", 10, 12, None, None)
            .unwrap();
        let alpha = store
            .insert_symbol("alpha", "function", "src/a.rs", 10, 12, None, None)
            .unwrap();
        let caller = store
            .insert_symbol("caller", "function", "other.rs", 1, 3, None, None)
            .unwrap();
        store
            .insert_edge(caller, Some(zed), "zed", "calls", 2)
            .unwrap();
        store
            .insert_edge(caller, Some(alpha), "alpha", "calls", 3)
            .unwrap();

        let ranked = store.map_centrality("src", "directory", 1).unwrap();
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].symbol.name, "alpha");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn map_hotspots_rank_unambiguous_names_before_pooled_collisions() {
        let path = tmp_db("map_hotspot_collision_confidence");
        let store = Store::open(&path).unwrap();
        let unique = store
            .insert_symbol("unique", "function", "src/a.rs", 1, 2, None, None)
            .unwrap();
        let shared = store
            .insert_symbol("shared", "function", "src/b.rs", 1, 2, None, None)
            .unwrap();
        store
            .insert_symbol("shared", "function", "vendor/shared.rs", 1, 2, None, None)
            .unwrap();
        let first = store
            .insert_symbol("first", "function", "outside/a.rs", 1, 2, None, None)
            .unwrap();
        let second = store
            .insert_symbol("second", "function", "outside/b.rs", 1, 2, None, None)
            .unwrap();
        store
            .insert_edge(first, Some(unique), "unique", "calls", 1)
            .unwrap();
        store
            .insert_edge(first, Some(shared), "shared", "calls", 2)
            .unwrap();
        store
            .insert_edge(second, Some(shared), "shared", "calls", 2)
            .unwrap();

        let ranked = store.map_centrality("src", "directory", 1).unwrap();
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].symbol.name, "unique");
        assert_eq!(ranked[0].name_collision, 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn map_centrality_collision_work_is_seeded_by_scoped_names() {
        let path = tmp_db("map_scoped_collision_work");
        let store = Store::open(&path).unwrap();
        let target = store
            .insert_symbol("shared", "function", "src/app/lib.rs", 1, 3, None, None)
            .unwrap();
        store
            .insert_symbol("shared", "function", "vendor/shared.rs", 1, 3, None, None)
            .unwrap();
        let caller = store
            .insert_symbol("caller", "function", "outside.rs", 1, 3, None, None)
            .unwrap();
        store
            .insert_edge(caller, Some(target), "shared", "calls", 2)
            .unwrap();
        for index in 0..2_000 {
            store
                .insert_symbol(
                    &format!("unrelated_{index:04}"),
                    "function",
                    &format!("vendor/f{index:04}.rs"),
                    1,
                    1,
                    None,
                    None,
                )
                .unwrap();
        }

        let rows = store.map_centrality("src/app", "directory", 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].symbol.name, "shared");
        assert_eq!(rows[0].name_collision, 2);

        let mut stmt = store
            .conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 WITH scoped_defs AS (
                     SELECT name
                     FROM symbols
                     WHERE kind != 'module'
                       AND substr(file_path, 1, length(?1) + 1) = ?1 || '/'
                 ),
                 scoped_names AS (
                     SELECT DISTINCT name FROM scoped_defs
                 )
                 SELECT n.name,
                        (
                            SELECT COUNT(*)
                            FROM symbols s INDEXED BY idx_symbols_name
                            WHERE s.kind != 'module' AND s.name = n.name
                        )
                 FROM scoped_names n",
            )
            .unwrap();
        let plan = stmt
            .query_map(params!["src/app"], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<SqlResult<Vec<_>>>()
            .unwrap();
        assert!(plan
            .iter()
            .any(|detail| detail.contains("CORRELATED SCALAR SUBQUERY")));
        assert!(plan
            .iter()
            .any(|detail| detail.contains("idx_symbols_name")));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn map_centrality_does_not_scan_unrelated_call_edges() {
        let path = tmp_db("map_scoped_degree_work");
        let store = Store::open(&path).unwrap();
        let target = store
            .insert_symbol("needle", "function", "src/app/lib.rs", 1, 3, None, None)
            .unwrap();
        let caller = store
            .insert_symbol("caller", "function", "outside.rs", 1, 3, None, None)
            .unwrap();
        store
            .insert_edge(caller, Some(target), "needle", "calls", 2)
            .unwrap();
        store.conn.execute_batch("BEGIN").unwrap();
        for _ in 0..20_000 {
            store
                .insert_edge(caller, None, "unrelated", "calls", 3)
                .unwrap();
        }
        store.conn.execute_batch("COMMIT").unwrap();

        let rows = store
            .with_work_budget(
                WorkBudget {
                    deadline: None,
                    op_ticks: Some(200),
                },
                || store.map_centrality("src/app", "directory", 10),
            )
            .expect("scope-seeded degree lookup must stay inside the work cap");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].symbol.name, "needle");
        assert_eq!(rows[0].in_degree, 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn map_store_ordering_never_uses_database_id_as_observable_tiebreak() {
        type SymbolOrder = Vec<(String, Option<String>)>;

        fn snapshot(path: &Path, reverse: bool) -> (SymbolOrder, SymbolOrder) {
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

            let boundaries = store
                .map_boundaries(
                    &[MapBoundaryScope {
                        label: ".".into(),
                        path: "src".into(),
                        match_mode: MapBoundaryMatch::Direct,
                    }],
                    21,
                    401,
                )
                .unwrap()
                .into_iter()
                .map(|row| (row.symbol.kind, row.symbol.signature))
                .collect();
            let centrality = store
                .map_centrality("src", "directory", 10)
                .unwrap()
                .into_iter()
                .map(|row| (row.symbol.kind, row.symbol.signature))
                .collect();
            (boundaries, centrality)
        }

        let first_path = tmp_db("map_store_semantic_order_first");
        let second_path = tmp_db("map_store_semantic_order_second");
        let first = snapshot(&first_path, false);
        let second_before = snapshot(&second_path, true);
        assert_eq!(first, second_before);
        assert_eq!(
            first.0,
            vec![
                ("function".into(), Some("fn tied()".into())),
                ("method".into(), Some("fn tied(&self)".into())),
            ]
        );
        Store::open(&second_path)
            .unwrap()
            .conn
            .execute_batch("VACUUM")
            .unwrap();
        let second_after = {
            let store = Store::open(&second_path).unwrap();
            let boundaries = store
                .map_boundaries(
                    &[MapBoundaryScope {
                        label: ".".into(),
                        path: "src".into(),
                        match_mode: MapBoundaryMatch::Direct,
                    }],
                    21,
                    401,
                )
                .unwrap()
                .into_iter()
                .map(|row| (row.symbol.kind, row.symbol.signature))
                .collect::<Vec<_>>();
            let centrality = store
                .map_centrality("src", "directory", 10)
                .unwrap()
                .into_iter()
                .map(|row| (row.symbol.kind, row.symbol.signature))
                .collect::<Vec<_>>();
            (boundaries, centrality)
        };
        assert_eq!(first, second_after);
        std::fs::remove_file(&first_path).ok();
        std::fs::remove_file(&second_path).ok();
    }

    #[test]
    fn map_import_edges_are_scoped_before_fetch() {
        let path = tmp_db("map_import_edges");
        let store = Store::open(&path).unwrap();
        let inside_source = store
            .insert_symbol("<module>", "module", "src/app/a.rs", 1, 20, None, None)
            .unwrap();
        let outside_source = store
            .insert_symbol("<module>", "module", "src/other.rs", 1, 20, None, None)
            .unwrap();
        store
            .insert_symbol("target", "function", "src/app/b.rs", 1, 3, None, None)
            .unwrap();
        store
            .insert_symbol("target", "function", "src/outside/b.rs", 1, 3, None, None)
            .unwrap();
        store
            .insert_edge(inside_source, None, "target", "imports", 2)
            .unwrap();
        store
            .insert_edge(outside_source, None, "target", "imports", 2)
            .unwrap();

        assert_eq!(
            store.map_import_edges("src/app", "directory", 1).unwrap(),
            vec![("src/app/a.rs".into(), "src/app/b.rs".into())]
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn scratchpad_append_and_read_newest_first() {
        let path = tmp_db("scratchpad_basic");
        let mut store = Store::open(&path).unwrap();
        let (id1, ts1) = store
            .scratchpad_append("planner", "intent", "drafting spec 042")
            .unwrap();
        // 1-second separation makes ORDER BY ts DESC deterministic.
        std::thread::sleep(std::time::Duration::from_secs(1));
        let (id2, ts2) = store
            .scratchpad_append("executor", "handoff", "phase 1 done, ready for audit")
            .unwrap();
        assert!(id2 > id1);
        assert!(ts2 >= ts1);

        // No filters → newest first.
        let all = store.scratchpad_read(None, None, None, 10).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].agent, "executor");
        assert_eq!(all[1].agent, "planner");

        // Filter by agent.
        let only_planner = store
            .scratchpad_read(None, Some("planner"), None, 10)
            .unwrap();
        assert_eq!(only_planner.len(), 1);
        assert_eq!(only_planner[0].body, "drafting spec 042");

        // Filter by kind.
        let only_handoff = store
            .scratchpad_read(None, None, Some("handoff"), 10)
            .unwrap();
        assert_eq!(only_handoff.len(), 1);

        // Filter by since_ts — exclude the first entry.
        let since_second = store.scratchpad_read(Some(ts2), None, None, 10).unwrap();
        assert_eq!(since_second.len(), 1);
        assert_eq!(since_second[0].agent, "executor");

        // Limit.
        let only_one = store.scratchpad_read(None, None, None, 1).unwrap();
        assert_eq!(only_one.len(), 1);
        assert_eq!(only_one[0].agent, "executor");
    }

    #[test]
    fn scratchpad_table_idempotent_across_opens() {
        // Re-opening an existing DB must not error (CREATE TABLE IF NOT EXISTS).
        let path = tmp_db("scratchpad_idempotent");
        {
            let mut store = Store::open(&path).unwrap();
            store.scratchpad_append("a", "n", "hi").unwrap();
        }
        let store = Store::open(&path).unwrap();
        let rows = store.scratchpad_read(None, None, None, 10).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn impact_of_many_preserves_seed_evidence_and_minimum_depth() {
        let path = tmp_db("impact_many_evidence");
        let store = Store::open(&path).unwrap();
        let direct = store
            .insert_symbol("direct", "function", "src/direct.rs", 1, 2, None, None)
            .unwrap();
        let converged = store
            .insert_symbol("converged", "function", "tests/test.rs", 3, 4, None, None)
            .unwrap();
        store
            .insert_edge(direct, None, "alpha", "calls", 1)
            .unwrap();
        store.insert_edge(direct, None, "beta", "calls", 1).unwrap();
        store
            .insert_edge(converged, None, "direct", "calls", 3)
            .unwrap();

        let rows = store
            .impact_of_many(&["beta".to_string(), "alpha".to_string()], 3, 5001, None)
            .unwrap();
        assert!(rows
            .iter()
            .any(|row| { row.seed == "alpha" && row.symbol.name == "direct" && row.depth == 1 }));
        assert!(rows
            .iter()
            .any(|row| { row.seed == "beta" && row.symbol.name == "converged" && row.depth == 2 }));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn impact_of_many_enforces_seed_and_row_limits() {
        let path = tmp_db("impact_many_limits");
        let store = Store::open(&path).unwrap();
        assert!(store.impact_of_many(&[], 1, 1, None).is_err());
        assert!(store
            .impact_of_many(&vec!["seed".to_string(); 201], 1, 1, None)
            .is_err());
        assert!(store
            .impact_of_many(&["seed".to_string()], 0, 1, None)
            .is_err());
        assert!(store
            .impact_of_many(&["seed".to_string()], 11, 1, None)
            .is_err());
        assert!(store
            .impact_of_many(&["seed".to_string()], 1, 5002, None)
            .is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn impact_of_many_accepts_widened_depth_up_to_ten() {
        let path = tmp_db("impact_many_depth_ten");
        let store = Store::open(&path).unwrap();
        store
            .insert_symbol("seed", "function", "src/a.rs", 1, 2, None, None)
            .unwrap();
        // max_depth=10 must be accepted — it is the `mmcg_impact` tool's
        // advertised cap.
        assert!(store
            .impact_of_many(&["seed".to_string()], 10, 5001, None)
            .is_ok());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn impact_of_many_aborts_dense_collision_cycle_work() {
        let path = tmp_db("impact_many_dense_cycle");
        let store = Store::open(&path).unwrap();
        for index in 0..80 {
            let name = format!("node{}", index % 8);
            let id = store
                .insert_symbol(
                    &name,
                    "function",
                    &format!("src/{index}.rs"),
                    1,
                    2,
                    None,
                    None,
                )
                .unwrap();
            for target in 0..8 {
                store
                    .insert_edge(id, None, &format!("node{target}"), "calls", 1)
                    .unwrap();
            }
        }
        let result = store.impact_of_many(&["node0".to_string()], 5, 5001, None);
        assert!(result.is_err() || result.as_ref().is_ok_and(|rows| rows.len() <= 5001));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn impact_of_many_uses_target_indexes_for_a_unique_seed() {
        let path = tmp_db("impact_many_unique_seed_index");
        let store = Store::open(&path).unwrap();
        let caller = store
            .insert_symbol("caller", "function", "src/caller.rs", 1, 2, None, None)
            .unwrap();
        store
            .insert_edge(caller, None, "needle", "calls", 1)
            .unwrap();
        store.conn.execute_batch("BEGIN").unwrap();
        for _ in 0..20_000 {
            store
                .insert_edge(caller, None, "unrelated", "calls", 2)
                .unwrap();
        }
        store.conn.execute_batch("COMMIT").unwrap();

        let rows = store
            .with_work_budget(
                WorkBudget {
                    deadline: None,
                    op_ticks: Some(50),
                },
                || store.impact_of_many(&["needle".to_string()], 1, 10, None),
            )
            .expect("unique target lookup must not scan every call edge");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].symbol.name, "caller");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn open_sets_busy_timeout_and_cache_size() {
        let path = tmp_db("pragmas");
        let store = Store::open(&path).unwrap();
        let busy_timeout: i64 = store
            .conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(busy_timeout, 5000);
        let cache_size: i64 = store
            .conn
            .query_row("PRAGMA cache_size", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cache_size, -65536);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn default_budgets_match_documented_contract() {
        // Default budget for MCP serve: 10,000ms; CLI queries: 60,000ms;
        // 0 = unlimited (both contexts share the same env var, only the
        // fallback default differs).
        assert_eq!(DEFAULT_SERVE_BUDGET_MS, 10_000);
        assert_eq!(DEFAULT_CLI_BUDGET_MS, 60_000);
        assert!(WorkBudget::from_millis(0).deadline.is_none());
        assert_eq!(
            WorkBudget::from_millis(5).deadline,
            Some(Duration::from_millis(5))
        );
        let path = tmp_db("default_budget_wiring");
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.default_work_budget().deadline,
            Some(Duration::from_millis(DEFAULT_SERVE_BUDGET_MS))
        );
        std::fs::remove_file(&path).ok();
    }

    fn assert_sqlite_interrupted(result: &SqlResult<i64>) {
        assert!(
            matches!(
                result,
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == rusqlite::ErrorCode::OperationInterrupted
            ),
            "expected a work-budget interrupt, got {result:?}"
        );
    }

    #[test]
    fn work_budget_interrupts_sqlite_at_a_deterministic_op_cap() {
        let path = tmp_db("work_budget_op_cap");
        let store = Store::open(&path).unwrap();
        let result = store.run_interrupt_probe(WorkBudget {
            deadline: None,
            op_ticks: Some(1),
        });

        assert_sqlite_interrupted(&result);
        assert_eq!(store.take_interrupt_source(), Some(InterruptSource::Budget));
        assert_eq!(
            store.ops_counter.load(Ordering::Relaxed),
            1,
            "the first 1,000-instruction progress tick must raise the interrupt"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn fifty_millisecond_work_budget_interrupts_with_scheduler_tolerance() {
        const BUDGET: Duration = Duration::from_millis(50);
        const SCHEDULER_TOLERANCE: Duration = Duration::from_secs(1);

        let path = tmp_db("work_budget_deadline");
        let store = Store::open(&path).unwrap();
        let budget = WorkBudget::from_millis(BUDGET.as_millis() as u64);
        assert_eq!(budget.deadline, Some(BUDGET));
        assert_eq!(budget.op_ticks, None);

        let started = Instant::now();
        let result = store.run_interrupt_probe(budget);
        let elapsed = started.elapsed();

        // Correct SQLite interrupt propagation is proven without wall-clock
        // scheduling in the op-cap test above. This measurement independently
        // keeps the production 50ms deadline exact while allowing a descheduled
        // CI worker a bounded amount of wall-clock delay.
        assert_sqlite_interrupted(&result);
        assert_eq!(store.take_interrupt_source(), Some(InterruptSource::Budget));
        assert!(
            elapsed >= BUDGET,
            "the deadline fired earlier than its 50ms contract: {elapsed:?}"
        );
        assert!(
            elapsed < BUDGET + SCHEDULER_TOLERANCE,
            "50ms deadline exceeded the 1s scheduler allowance: {elapsed:?}"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn work_budget_nesting_takes_min() {
        let path = tmp_db("work_budget_nesting");
        let store = Store::open(&path).unwrap();

        // A budget composes with an already-installed parent via min — a
        // generous inner budget can never outlive a tighter outer one.
        let outer = WorkBudget {
            deadline: Some(Duration::from_millis(1)),
            op_ticks: None,
        };
        assert!(!store.push_work_budget(outer));
        std::thread::sleep(Duration::from_millis(5));
        let generous_inner = WorkBudget {
            deadline: Some(Duration::from_secs(5)),
            op_ticks: None,
        };
        assert!(
            store.push_work_budget(generous_inner),
            "inner budget must not extend past the already-expired outer one"
        );
        store.pop_work_budget();
        store.pop_work_budget();

        // Same for op ticks: an inner cap higher than the outer's *remaining*
        // ticks is clamped down, not honored at face value.
        let outer_ticks = WorkBudget {
            deadline: None,
            op_ticks: Some(10),
        };
        assert!(!store.push_work_budget(outer_ticks));
        store.ops_counter.fetch_add(10, Ordering::Relaxed);
        let generous_inner_ticks = WorkBudget {
            deadline: None,
            op_ticks: Some(1_000_000),
        };
        assert!(
            store.push_work_budget(generous_inner_ticks),
            "inner op-tick cap must be bounded by the outer's already-consumed ticks"
        );
        store.pop_work_budget();
        store.pop_work_budget();
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn local_precision_budget_clears_only_its_own_interrupt_marker() {
        fn long_query(store: &Store) -> SqlResult<i64> {
            store
                .conn
                .prepare(
                    "WITH RECURSIVE cnt(x) AS (
                         SELECT 1
                         UNION ALL
                         SELECT x + 1 FROM cnt WHERE x < 100000000
                     )
                     SELECT count(*) FROM cnt",
                )?
                .query_row([], |row| row.get(0))
        }

        let path = tmp_db("local_precision_budget_marker");
        let store = Store::open(&path).unwrap();

        assert!(!store.push_work_budget(WorkBudget {
            deadline: None,
            op_ticks: Some(1_000),
        }));
        let local = store.with_local_work_budget(
            WorkBudget {
                deadline: None,
                op_ticks: Some(1),
            },
            || long_query(&store),
        );
        assert_sqlite_interrupted(&local);
        assert_eq!(store.interrupt_source(), None);
        assert!(!store.work_interrupted());
        store.pop_work_budget();

        assert!(!store.push_work_budget(WorkBudget {
            deadline: None,
            op_ticks: Some(1),
        }));
        let inherited = store.with_local_work_budget(
            WorkBudget {
                deadline: None,
                op_ticks: Some(1_000),
            },
            || long_query(&store),
        );
        assert_sqlite_interrupted(&inherited);
        assert_eq!(store.interrupt_source(), Some(InterruptSource::Budget));
        store.pop_work_budget();
        assert_eq!(store.take_interrupt_source(), Some(InterruptSource::Budget));
        std::fs::remove_file(&path).ok();
    }

    struct XorShift64(u64);
    impl XorShift64 {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    #[derive(Clone)]
    struct GenSymbol {
        name: String,
        kind: String,
        file_path: String,
        line_start: u32,
        language: String,
    }

    #[derive(Clone)]
    struct GenEdge {
        from: usize,
        to_name: String,
        to_type: Option<String>,
    }

    const GEN_NAMES: &[&str] = &["Alpha", "Beta", "Gamma", "Delta"];
    const GEN_KINDS: &[&str] = &["function", "method", "class"];
    const GEN_FILES: &[&str] = &["src/a.rs", "src/sub/b.rs", "tests/t.rs"];
    const GEN_LANGUAGES: &[&str] = &["python", "rust"];

    /// Randomized calls-graph: few distinct names (forces same-name
    /// collisions across kinds), a mix of production and non-production
    /// files, a mix of languages, and edges that include `to_type`-only
    /// shapes (`Type::method()` — `to_name` resolves to no definition, only
    /// `to_type` matches a real name) — the case the degree pre-aggregation
    /// must union the `to_name` and `to_type` branches to get right.
    fn gen_calls_graph(
        seed: u64,
        symbol_count: usize,
        edge_count: usize,
    ) -> (Vec<GenSymbol>, Vec<GenEdge>) {
        let mut rng = XorShift64(seed.wrapping_mul(2).wrapping_add(1) | 1);
        let symbols: Vec<GenSymbol> = (0..symbol_count)
            .map(|index| GenSymbol {
                name: GEN_NAMES[rng.below(GEN_NAMES.len())].to_string(),
                kind: GEN_KINDS[rng.below(GEN_KINDS.len())].to_string(),
                file_path: GEN_FILES[rng.below(GEN_FILES.len())].to_string(),
                line_start: index as u32 + 1,
                language: GEN_LANGUAGES[rng.below(GEN_LANGUAGES.len())].to_string(),
            })
            .collect();
        let edges: Vec<GenEdge> = (0..edge_count)
            .map(|index| {
                let from = rng.below(symbol_count);
                match rng.below(3) {
                    0 => GenEdge {
                        from,
                        to_name: GEN_NAMES[rng.below(GEN_NAMES.len())].to_string(),
                        to_type: None,
                    },
                    1 => GenEdge {
                        from,
                        to_name: format!("__no_such_definition_{index}"),
                        to_type: Some(GEN_NAMES[rng.below(GEN_NAMES.len())].to_string()),
                    },
                    _ => GenEdge {
                        from,
                        to_name: GEN_NAMES[rng.below(GEN_NAMES.len())].to_string(),
                        to_type: Some(GEN_NAMES[rng.below(GEN_NAMES.len())].to_string()),
                    },
                }
            })
            .collect();
        (symbols, edges)
    }

    fn seed_calls_graph(store: &Store, symbols: &[GenSymbol], edges: &[GenEdge]) -> Vec<i64> {
        let ids: Vec<i64> = symbols
            .iter()
            .map(|s| {
                let id = store
                    .insert_symbol(
                        &s.name,
                        &s.kind,
                        &s.file_path,
                        s.line_start,
                        s.line_start + 1,
                        None,
                        None,
                    )
                    .unwrap();
                // `insert_symbol` doesn't take a `language` column — set it
                // directly so the equivalence tests can exercise the
                // `language` filter too.
                store
                    .conn
                    .execute(
                        "UPDATE symbols SET language = ?1 WHERE id = ?2",
                        params![s.language, id],
                    )
                    .unwrap();
                id
            })
            .collect();
        for edge in edges {
            store
                .insert_edge_with_type(
                    ids[edge.from],
                    &edge.to_name,
                    edge.to_type.as_deref(),
                    "calls",
                    1,
                )
                .unwrap();
        }
        ids
    }

    /// Mirrors `production_path_filter` in Rust.
    fn production_excluded(file_path: &str) -> bool {
        !is_production_path(file_path)
    }

    fn bruteforce_in_degree(
        symbols: &[GenSymbol],
        edges: &[GenEdge],
        name: &str,
        production_only: bool,
    ) -> u32 {
        edges
            .iter()
            .filter(|edge| edge.to_name == name || edge.to_type.as_deref() == Some(name))
            .filter(|edge| !production_only || !production_excluded(&symbols[edge.from].file_path))
            .map(|edge| edge.from)
            .collect::<BTreeSet<_>>()
            .len() as u32
    }

    fn bruteforce_name_collision(symbols: &[GenSymbol], name: &str, production_only: bool) -> u32 {
        symbols
            .iter()
            .filter(|s| s.name == name)
            .filter(|s| !production_only || !production_excluded(&s.file_path))
            .count() as u32
    }

    #[test]
    fn centrality_equivalence_property() {
        for seed in 0..12u64 {
            let path = tmp_db(&format!("centrality_equiv_{seed}"));
            let store = Store::open(&path).unwrap();
            let (symbols, edges) = gen_calls_graph(seed, 24, 40);
            seed_calls_graph(&store, &symbols, &edges);

            for production_only in [false, true] {
                // `centrality` (no production_only param — always the
                // whole index) is only compared at production_only=false.
                if !production_only {
                    let actual = store.centrality(None, None, None, 1000).unwrap();
                    let mut expected: Vec<(String, String, String, u32, u32, u32)> = symbols
                        .iter()
                        .filter(|s| s.kind != "module")
                        .map(|s| {
                            let in_degree = bruteforce_in_degree(&symbols, &edges, &s.name, false);
                            let collisions = bruteforce_name_collision(&symbols, &s.name, false);
                            (
                                s.name.clone(),
                                s.kind.clone(),
                                s.file_path.clone(),
                                s.line_start,
                                in_degree,
                                collisions,
                            )
                        })
                        .filter(|(_, _, _, _, in_degree, _)| *in_degree > 0)
                        .collect();
                    expected.sort_by(|a, b| {
                        b.4.cmp(&a.4)
                            .then_with(|| a.2.cmp(&b.2))
                            .then_with(|| a.3.cmp(&b.3))
                    });
                    let actual_tuples: Vec<_> = actual
                        .iter()
                        .map(|(s, in_degree, collisions)| {
                            (
                                s.name.clone(),
                                s.kind.clone(),
                                s.file_path.clone(),
                                s.line_start,
                                *in_degree,
                                *collisions,
                            )
                        })
                        .collect();
                    assert_eq!(actual_tuples, expected, "seed {seed} centrality");

                    // `language` filters the result symbols only — in-degree
                    // itself is computed across all edges regardless of the
                    // caller's language.
                    for language in GEN_LANGUAGES {
                        let actual = store.centrality(None, Some(language), None, 1000).unwrap();
                        let mut expected: Vec<(String, String, String, u32, u32, u32)> = symbols
                            .iter()
                            .filter(|s| s.kind != "module" && s.language == *language)
                            .map(|s| {
                                let in_degree =
                                    bruteforce_in_degree(&symbols, &edges, &s.name, false);
                                let collisions =
                                    bruteforce_name_collision(&symbols, &s.name, false);
                                (
                                    s.name.clone(),
                                    s.kind.clone(),
                                    s.file_path.clone(),
                                    s.line_start,
                                    in_degree,
                                    collisions,
                                )
                            })
                            .filter(|(_, _, _, _, in_degree, _)| *in_degree > 0)
                            .collect();
                        expected.sort_by(|a, b| {
                            b.4.cmp(&a.4)
                                .then_with(|| a.2.cmp(&b.2))
                                .then_with(|| a.3.cmp(&b.3))
                        });
                        let actual_tuples: Vec<_> = actual
                            .iter()
                            .map(|(s, in_degree, collisions)| {
                                (
                                    s.name.clone(),
                                    s.kind.clone(),
                                    s.file_path.clone(),
                                    s.line_start,
                                    *in_degree,
                                    *collisions,
                                )
                            })
                            .collect();
                        assert_eq!(
                            actual_tuples, expected,
                            "seed {seed} centrality language={language}"
                        );
                    }
                }

                // `map_centrality_filtered` at scope="root" (the whole
                // index) exercises the rewritten `name_degrees` CTE across
                // production_only on/off.
                let actual = store
                    .map_centrality_filtered(".", "root", 1000, production_only)
                    .unwrap();
                let mut expected: Vec<(String, String, String, u32, u32, u32)> = symbols
                    .iter()
                    .filter(|s| s.kind != "module")
                    .filter(|s| !production_only || !production_excluded(&s.file_path))
                    .map(|s| {
                        let in_degree =
                            bruteforce_in_degree(&symbols, &edges, &s.name, production_only);
                        let collisions =
                            bruteforce_name_collision(&symbols, &s.name, production_only);
                        (
                            s.name.clone(),
                            s.kind.clone(),
                            s.file_path.clone(),
                            s.line_start,
                            in_degree,
                            collisions,
                        )
                    })
                    .filter(|(_, _, _, _, in_degree, _)| *in_degree > 0)
                    .collect();
                expected.sort_by(|a, b| {
                    (a.5 > 1)
                        .cmp(&(b.5 > 1))
                        .then_with(|| b.4.cmp(&a.4))
                        .then_with(|| a.2.cmp(&b.2))
                        .then_with(|| a.3.cmp(&b.3))
                });
                let actual_tuples: Vec<_> = actual
                    .iter()
                    .map(|row| {
                        (
                            row.symbol.name.clone(),
                            row.symbol.kind.clone(),
                            row.symbol.file_path.clone(),
                            row.symbol.line_start,
                            row.in_degree,
                            row.name_collision,
                        )
                    })
                    .collect();
                assert_eq!(
                    actual_tuples, expected,
                    "seed {seed} map_centrality_filtered production_only={production_only}"
                );
            }
            std::fs::remove_file(&path).ok();
        }
    }

    #[test]
    fn unreferenced_equivalence_property() {
        for seed in 0..12u64 {
            let path = tmp_db(&format!("unreferenced_equiv_{seed}"));
            let store = Store::open(&path).unwrap();
            let (symbols, edges) = gen_calls_graph(seed, 24, 30);
            seed_calls_graph(&store, &symbols, &edges);

            let referenced: BTreeSet<&str> = edges
                .iter()
                .flat_map(|edge| {
                    std::iter::once(edge.to_name.as_str()).chain(edge.to_type.as_deref())
                })
                .collect();
            for kind in [None, Some("function"), Some("method")] {
                for language in [None, Some("python"), Some("rust")] {
                    let actual = store.unreferenced(kind, language).unwrap();
                    let mut expected: Vec<(String, String, String, u32)> = symbols
                        .iter()
                        .filter(|s| s.kind != "module")
                        .filter(|s| kind.is_none_or(|k| s.kind == k))
                        .filter(|s| language.is_none_or(|l| s.language == l))
                        .filter(|s| !referenced.contains(s.name.as_str()))
                        .map(|s| {
                            (
                                s.name.clone(),
                                s.kind.clone(),
                                s.file_path.clone(),
                                s.line_start,
                            )
                        })
                        .collect();
                    expected.sort_by(|a, b| (&a.2, a.3).cmp(&(&b.2, b.3)));
                    let actual_tuples: Vec<_> = actual
                        .iter()
                        .map(|s| {
                            (
                                s.name.clone(),
                                s.kind.clone(),
                                s.file_path.clone(),
                                s.line_start,
                            )
                        })
                        .collect();
                    assert_eq!(
                        actual_tuples, expected,
                        "seed {seed} kind {kind:?} language {language:?}"
                    );
                }
            }
            std::fs::remove_file(&path).ok();
        }
    }

    #[test]
    fn api_surface_equivalence_property() {
        for seed in 0..12u64 {
            let path = tmp_db(&format!("api_surface_equiv_{seed}"));
            let store = Store::open(&path).unwrap();
            let (symbols, edges) = gen_calls_graph(seed, 24, 40);
            seed_calls_graph(&store, &symbols, &edges);

            let prefix = "src/";
            let externally_referenced: BTreeSet<&str> = edges
                .iter()
                .filter(|edge| !symbols[edge.from].file_path.starts_with(prefix))
                .flat_map(|edge| {
                    std::iter::once(edge.to_name.as_str()).chain(edge.to_type.as_deref())
                })
                .collect();
            for language in [None, Some("python"), Some("rust")] {
                let actual = store.api_surface(prefix, language).unwrap();
                let expected: BTreeSet<(String, String, String, u32)> = symbols
                    .iter()
                    .filter(|s| s.kind != "module")
                    .filter(|s| s.file_path.starts_with(prefix))
                    .filter(|s| language.is_none_or(|l| s.language == l))
                    .filter(|s| externally_referenced.contains(s.name.as_str()))
                    .map(|s| {
                        (
                            s.name.clone(),
                            s.kind.clone(),
                            s.file_path.clone(),
                            s.line_start,
                        )
                    })
                    .collect();
                let actual_set: BTreeSet<(String, String, String, u32)> = actual
                    .iter()
                    .map(|s| {
                        (
                            s.name.clone(),
                            s.kind.clone(),
                            s.file_path.clone(),
                            s.line_start,
                        )
                    })
                    .collect();
                assert_eq!(actual_set, expected, "seed {seed} language {language:?}");
            }
            std::fs::remove_file(&path).ok();
        }
    }

    #[derive(Clone)]
    struct GenImportEdge {
        from_file: usize,
        to_name: String,
    }

    const GEN_IMPORT_FILES: &[&str] = &["src/a.py", "src/b.py", "src/c.py", "tests/t.py"];

    fn gen_import_graph(seed: u64, edge_count: usize) -> (Vec<GenSymbol>, Vec<GenImportEdge>) {
        let mut rng = XorShift64(seed.wrapping_mul(3).wrapping_add(1) | 1);
        // One `<module>` symbol per file plus a handful of real definitions,
        // several sharing a name across files (the collision case
        // `map_import_edges_filtered`'s pre-dedup must still resolve to the
        // same DISTINCT file pairs as the old per-edge join).
        let mut symbols = Vec::new();
        for (index, file) in GEN_IMPORT_FILES.iter().enumerate() {
            symbols.push(GenSymbol {
                name: "<module>".to_string(),
                kind: "module".to_string(),
                file_path: file.to_string(),
                line_start: (index as u32) * 100 + 1,
                language: "python".to_string(),
            });
        }
        for index in 0..8 {
            symbols.push(GenSymbol {
                name: GEN_NAMES[index % GEN_NAMES.len()].to_string(),
                kind: "function".to_string(),
                file_path: GEN_IMPORT_FILES[index % GEN_IMPORT_FILES.len()].to_string(),
                line_start: 1000 + index as u32,
                language: "python".to_string(),
            });
        }
        let edges: Vec<GenImportEdge> = (0..edge_count)
            .map(|_| GenImportEdge {
                from_file: rng.below(GEN_IMPORT_FILES.len()),
                to_name: GEN_NAMES[rng.below(GEN_NAMES.len())].to_string(),
            })
            .collect();
        (symbols, edges)
    }

    fn seed_import_graph(
        store: &Store,
        symbols: &[GenSymbol],
        edges: &[GenImportEdge],
    ) -> Vec<i64> {
        let ids: Vec<i64> = symbols
            .iter()
            .map(|s| {
                store
                    .insert_symbol(
                        &s.name,
                        &s.kind,
                        &s.file_path,
                        s.line_start,
                        s.line_start + 1,
                        None,
                        None,
                    )
                    .unwrap()
            })
            .collect();
        // Module symbols come first, one per `GEN_IMPORT_FILES` entry.
        for edge in edges {
            store
                .insert_edge(ids[edge.from_file], None, &edge.to_name, "imports", 1)
                .unwrap();
        }
        ids
    }

    #[test]
    fn import_edges_equivalence_property() {
        for seed in 0..12u64 {
            let path = tmp_db(&format!("import_edges_equiv_{seed}"));
            let store = Store::open(&path).unwrap();
            let (symbols, edges) = gen_import_graph(seed, 24);
            seed_import_graph(&store, &symbols, &edges);

            for production_only in [false, true] {
                let actual = store
                    .map_import_edges_filtered(".", "root", 10_000, production_only)
                    .unwrap();
                let mut expected: BTreeSet<(String, String)> = BTreeSet::new();
                for edge in &edges {
                    let from_file = GEN_IMPORT_FILES[edge.from_file];
                    for target in symbols.iter().filter(|s| s.name == edge.to_name) {
                        if from_file == target.file_path {
                            continue;
                        }
                        if production_only
                            && (production_excluded(from_file)
                                || production_excluded(&target.file_path))
                        {
                            continue;
                        }
                        expected.insert((from_file.to_string(), target.file_path.clone()));
                    }
                }
                let actual_set: BTreeSet<(String, String)> = actual.into_iter().collect();
                assert_eq!(
                    actual_set, expected,
                    "seed {seed} production_only={production_only}"
                );
            }
            std::fs::remove_file(&path).ok();
        }
    }

    fn bruteforce_dependency_pairs(
        symbols: &[GenSymbol],
        edges: &[GenImportEdge],
    ) -> BTreeSet<(String, String)> {
        let mut pairs = BTreeSet::new();
        for edge in edges {
            let from_file = GEN_IMPORT_FILES[edge.from_file];
            for target in symbols.iter().filter(|s| s.name == edge.to_name) {
                if from_file == target.file_path {
                    continue;
                }
                pairs.insert((from_file.to_string(), target.file_path.clone()));
            }
        }
        pairs
    }

    #[test]
    fn dependency_cycles_equivalence_and_cap() {
        for seed in 0..12u64 {
            let path = tmp_db(&format!("dep_cycles_equiv_{seed}"));
            let store = Store::open(&path).unwrap();
            let (symbols, edges) = gen_import_graph(seed, 24);
            seed_import_graph(&store, &symbols, &edges);

            let (actual, truncated) = store.dependency_cycles(None, 2).unwrap();
            assert!(!truncated);
            let pairs = bruteforce_dependency_pairs(&symbols, &edges);
            let mut adj: std::collections::BTreeMap<String, Vec<String>> = Default::default();
            for (from, to) in &pairs {
                adj.entry(from.clone()).or_default().push(to.clone());
            }
            let mut expected: Vec<Vec<String>> = tarjan_scc(&adj)
                .into_iter()
                .filter(|c| c.len() >= 2)
                .collect();
            expected.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
            assert_eq!(actual, expected, "seed {seed}");
            std::fs::remove_file(&path).ok();
        }

        // Above the pair cap, the truncation marker fires and Tarjan is
        // never run — capping a graph algorithm's input can split or hide
        // real cycles, so this must never look like "just more available".
        let path = tmp_db("dep_cycles_cap");
        let store = Store::open(&path).unwrap();
        store.conn.execute_batch("BEGIN").unwrap();
        store
            .insert_symbol("Shared", "function", "shared.py", 1, 2, None, None)
            .unwrap();
        let over_cap = DEPENDENCY_CYCLE_PAIR_LIMIT + 1;
        for index in 0..over_cap {
            let file = format!("src/mod_{index}.py");
            let module = store
                .insert_symbol("<module>", "module", &file, 1, 2, None, None)
                .unwrap();
            store
                .insert_edge(module, None, "Shared", "imports", 1)
                .unwrap();
        }
        store.conn.execute_batch("COMMIT").unwrap();
        let (cycles, truncated) = store.dependency_cycles(None, 2).unwrap();
        assert!(truncated);
        assert!(cycles.is_empty());
        std::fs::remove_file(&path).ok();
    }

    fn concept_signature_terms_for(path: &str, signature: &str) -> Vec<String> {
        concept_document("fixture", path, Some(signature), "")
            .signature_search
            .split_whitespace()
            .map(str::to_string)
            .collect()
    }

    fn concept_signature_terms(signature: &str) -> Vec<String> {
        concept_signature_terms_for("src/fixture.rs", signature)
    }

    #[test]
    fn concept_normalization_splits_digits_identifiers_and_unicode_lowercase() {
        let terms =
            concept_terms("sha256Digest HTTP2Server OAuth2Client path/to_file-name").unwrap();
        for expected in [
            "sha256digest",
            "sha",
            "256",
            "digest",
            "http2server",
            "http",
            "2",
            "server",
            "oauth2client",
            "auth",
            "client",
            "tofilename",
            "to",
            "file",
            "name",
        ] {
            assert!(
                terms.iter().any(|term| term == expected),
                "missing {expected:?}"
            );
        }

        let expanded = concept_terms("İValue").unwrap();
        assert!(expanded.iter().any(|term| term == "i\u{307}value"));
        assert!(expanded.iter().any(|term| term == "i\u{307}"));
        assert!(expanded.iter().any(|term| term == "value"));
    }

    #[test]
    fn concept_signature_redaction_omits_comments_literals_templates_and_regexes() {
        let multiline = concept_signature_terms(
            "fn borrow<'a>(\n\t/* TOPSECRET */ x: &'a str,\n y: ImportantType, c: char = 'X'\n) -> &'a str",
        );
        for expected in ["borrow", "a", "str", "importanttype"] {
            assert!(
                multiline.iter().any(|term| term == expected),
                "missing {expected:?}"
            );
        }
        assert!(!multiline.iter().any(|term| term.contains("topsecret")));

        let python_comment = concept_signature_terms_for(
            "src/fixture.py",
            "def borrow(x: str, # OTHERSECRET\n y: ImportantType): ...",
        );
        assert!(python_comment.iter().any(|term| term == "importanttype"));
        assert!(!python_comment
            .iter()
            .any(|term| term.contains("othersecret")));

        let rust_attributes = concept_signature_terms_for(
            "src/fixture.rs",
            "#[inline] pub fn r#match<'a>(x: &'a str) -> &'a str",
        );
        for expected in ["inline", "match", "a", "str"] {
            assert!(
                rust_attributes.iter().any(|term| term == expected),
                "missing {expected:?}: {rust_attributes:?}"
            );
        }

        for signature in [
            "function f(x = `prefix,TOPSECRET`, y: ImportantType) {}",
            "function f(x = /a,b/, y: ImportantType) {}",
            "function f(x = /[/,]TOPSECRET/u, y: ImportantType) {}",
            r"function f(x = /a\/,TOPSECRET/u, y: ImportantType) {}",
            "function f(x = a < b, y: ImportantType) {}",
            "function f(x = value < limit, y: ImportantType) {}",
            "function f(x = value <= limit, y: ImportantType) {}",
            "function f(x = low < value && value < high, y: ImportantType) {}",
            "void f(int x = 1 << 2, ImportantType y={})",
            "function f(x = new Map<Key,Value>(), y: ImportantType) {}",
            "function f(x = amount / /prefix,TOPSECRET/.test(v), y: ImportantType) {}",
            "function f(x = `value ${a / b}`, y: ImportantType) {}",
            "function f(x = `value ${/TOPSECRET/.test(v)}`, y: ImportantType) {}",
            "function f(x = `${typeof /[}]`PREFIX,TOPSECRET/}`, y: ImportantType) {}",
        ] {
            let terms = concept_signature_terms_for("src/fixture.ts", signature);
            assert!(
                terms.iter().any(|term| term == "importanttype"),
                "following declaration token lost for {signature:?}: {terms:?}"
            );
            assert!(
                !terms.iter().any(|term| term.contains("topsecret")),
                "literal leaked for {signature:?}: {terms:?}"
            );
        }

        for hash_count in [0usize, 1, 16, 17, 255] {
            let hashes = "#".repeat(hash_count);
            let content = if hash_count == 0 {
                "TOPSECRET"
            } else {
                "inside \",TOPSECRET"
            };
            let signature = format!(
                "fn borrow<'a>(x: &'a str = r{hashes}\"{content}\"{hashes}, y: ImportantType)"
            );
            let terms = concept_signature_terms(&signature);
            assert!(terms.iter().any(|term| term == "importanttype"));
            assert!(!terms.iter().any(|term| term.contains("topsecret")));
            assert!(terms.iter().any(|term| term == "a"));
        }
        let c_raw =
            concept_signature_terms("fn f(x: str = cr#\"inside \",TOPSECRET\"#, y: ImportantType)");
        assert!(c_raw.iter().any(|term| term == "importanttype"));
        assert!(!c_raw.iter().any(|term| term.contains("topsecret")));

        for prefix in ["R", "u8R", "uR", "UR", "LR"] {
            let signature = format!(
                "void f(const char* x = {prefix}\"tag(before\",TOPSECRET)tag\", ImportantType y={{}})"
            );
            let terms = concept_signature_terms_for("src/fixture.cpp", &signature);
            assert!(
                terms.iter().any(|term| term == "importanttype"),
                "following declaration token lost for {signature:?}: {terms:?}"
            );
            assert!(
                !terms.iter().any(|term| term.contains("topsecret")),
                "raw literal leaked for {signature:?}: {terms:?}"
            );
        }

        for (path, signature, retained) in [
            (
                "src/fixture.ts",
                "function f(x = `outer ${`inner,TOPSECRET`}`, y: ImportantType) {}",
                "importanttype",
            ),
            (
                "src/fixture.cs",
                r#"[Label(""""prefix """ TOPSECRET"""")] public void F()"#,
                "void",
            ),
            (
                "src/fixture.ts",
                "@Matches(/TOPSECRET/) class Candidate {}",
                "candidate",
            ),
            (
                "src/fixture.py",
                "def f(cb=lambda a, TOPSECRET: a, y: ImportantType): ...",
                "importanttype",
            ),
            (
                "src/fixture.cs",
                r#"[Label($"{ "TOPSECRET" }")] public void Interpolated()"#,
                "interpolated",
            ),
            (
                "src/fixture.cs",
                r#"[Label($@"{ "TOPSECRET" }")] public void VerbatimInterpolated()"#,
                "verbatiminterpolated",
            ),
            (
                "src/fixture.py",
                "@label(f\"{ \"TOPSECRET\" }\")\ndef decorated() -> ImportantType: ...",
                "importanttype",
            ),
            (
                "src/fixture.rs",
                "fn f(/* outer /* inner */ TOPSECRET */ x: ImportantType)",
                "importanttype",
            ),
            (
                "src/fixture.cpp",
                "void f(auto x = Foo<1, TOPSECRET>{}, ImportantType y={})",
                "importanttype",
            ),
            (
                "src/fixture.ts",
                "function f(x = Foo<\"x\", TOPSECRET>(), y: ImportantType) {}",
                "importanttype",
            ),
        ] {
            let terms = concept_signature_terms_for(path, signature);
            assert!(
                terms.iter().any(|term| term == retained),
                "following declaration token lost for {signature:?}: {terms:?}"
            );
            assert!(
                !terms.iter().any(|term| term.contains("topsecret")),
                "literal/default leaked for {signature:?}: {terms:?}"
            );
        }

        for signature in [
            "Widget& operator=(const Widget& other)",
            "Widget& operator+=(const Widget& other)",
        ] {
            let terms = concept_signature_terms_for("src/fixture.cpp", signature);
            assert!(terms.iter().any(|term| term == "operator"));
            assert!(terms.iter().any(|term| term == "widget"));
            assert!(terms.iter().any(|term| term == "other"));
        }

        for path in ["src/fixture.mjs", "src/fixture.cjs"] {
            let terms = concept_signature_terms_for(
                path,
                "function f(x = /prefix,TOPSECRET/, y: ImportantType) {}",
            );
            assert!(terms.iter().any(|term| term == "importanttype"));
            assert!(!terms.iter().any(|term| term.contains("topsecret")));
        }
        let phtml = concept_signature_terms_for(
            "src/fixture.phtml",
            "function f($x, # TOPSECRET\n ImportantType $y)",
        );
        assert!(phtml.iter().any(|term| term == "importanttype"));
        assert!(!phtml.iter().any(|term| term.contains("topsecret")));
        for path in ["src/fixture.ipp", "src/fixture.tpp"] {
            let terms =
                concept_signature_terms_for(path, "Widget& operator+=(const Widget& other)");
            assert!(terms.iter().any(|term| term == "operator"));
            assert!(terms.iter().any(|term| term == "other"));
        }

        for signature in [
            "function f($x = <<<LABEL\nprefix,TOPSECRET\n    LABEL,\n ImportantType $y = null) {}",
            "function f($x = <<<'LABEL'\nprefix,TOPSECRET\nLABEL;\n, ImportantType $y = null) {}",
        ] {
            let terms = concept_signature_terms_for("src/fixture.php", signature);
            assert!(
                terms.iter().any(|term| term == "importanttype"),
                "following declaration token lost for heredoc {terms:?}"
            );
            assert!(
                !terms.iter().any(|term| term.contains("topsecret")),
                "heredoc leaked into corpus: {terms:?}"
            );
        }
    }

    #[test]
    fn concept_persisted_corpus_never_contains_signature_canaries() {
        let path = tmp_db("concept_signature_canaries");
        let store = Store::open(&path).unwrap();
        for (index, (source_path, signature)) in [
            (
                "src/template.ts",
                "function template(x = `outer ${`inner,TOPSECRET`}`, y: ImportantType) {}",
            ),
            (
                "src/raw.cs",
                r#"[Label(""""prefix """ TOPSECRET"""")] public void RawCandidate()"#,
            ),
            (
                "src/decorator.ts",
                "@Matches(/TOPSECRET/) class RegexCandidate {}",
            ),
            (
                "src/lambda.py",
                "def lambda_candidate(cb=lambda a, TOPSECRET: a, y: ImportantType): ...",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            store
                .insert_symbol(
                    &format!("candidate_{index}"),
                    "function",
                    source_path,
                    1,
                    1,
                    Some(signature),
                    None,
                )
                .unwrap();
        }
        store.finalize_index_contracts_current().unwrap();

        assert!(store
            .search_concepts("\"topsecret\"", 50)
            .unwrap()
            .is_empty());
        let documents = store
            .conn
            .prepare("SELECT signature_search FROM symbol_concepts ORDER BY symbol_id")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<SqlResult<Vec<_>>>()
            .unwrap();
        assert_eq!(documents.len(), 4);
        assert!(documents
            .iter()
            .all(|document| !document.to_ascii_lowercase().contains("topsecret")));
        assert!(!store
            .search_concepts("\"importanttype\"", 50)
            .unwrap()
            .is_empty());
        assert!(!store
            .search_concepts("\"candidate\"", 50)
            .unwrap()
            .is_empty());

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn concept_insert_symbol_savepoint_nests_and_rolls_back_both_rows() {
        let path = tmp_db("concept_nested_savepoint");
        let store = Store::open(&path).unwrap();
        store
            .insert_symbol(
                "firstHandler",
                "function",
                "src/first.rs",
                1,
                2,
                Some("fn first_handler()"),
                None,
            )
            .unwrap();
        store.finalize_index_contracts_current().unwrap();
        assert!(store.concept_contract_current().unwrap());
        assert_eq!(store.symbol_count().unwrap(), 1);
        assert_eq!(store.concept_count().unwrap(), 1);

        store.conn.execute_batch("BEGIN").unwrap();
        store
            .insert_symbol(
                "secondHandler",
                "function",
                "src/second.rs",
                1,
                2,
                Some("fn second_handler()"),
                None,
            )
            .unwrap();
        assert_eq!(store.symbol_count().unwrap(), 2);
        assert_eq!(store.concept_count().unwrap(), 2);
        store.conn.execute_batch("ROLLBACK").unwrap();

        assert_eq!(store.symbol_count().unwrap(), 1);
        assert_eq!(store.concept_count().unwrap(), 1);
        assert!(store.concept_contract_current().unwrap());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn concept_old_writer_dirty_marker_blocks_incomplete_finalization() {
        let path = tmp_db("concept_old_writer_dirty");
        let store = Store::open(&path).unwrap();
        store
            .insert_symbol("ready", "function", "src/ready.rs", 1, 2, None, None)
            .unwrap();
        store.finalize_index_contracts_current().unwrap();
        assert!(store.concept_contract_current().unwrap());

        store
            .conn
            .execute(
                "INSERT INTO symbols(name, kind, file_path, line_start, line_end)
                 VALUES ('legacy_only', 'function', 'src/legacy.rs', 1, 2)",
                [],
            )
            .unwrap();
        assert!(!store.concept_contract_current().unwrap());
        assert!(store.finalize_index_contracts_current().is_err());
        assert!(!store.concept_contract_current().unwrap());

        store
            .conn
            .execute("DELETE FROM symbols WHERE name = 'legacy_only'", [])
            .unwrap();
        store.finalize_index_contracts_current().unwrap();
        assert!(store.concept_contract_current().unwrap());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn concept_documentation_legacy_file_writer_cannot_finalize_current() {
        let path = tmp_db("concept_documentation_legacy_file_writer");
        let mut store = Store::open(&path).unwrap();
        store
            .commit_file(PendingFile {
                path: "src/legacy.rs".to_string(),
                mtime: 1,
                content_sha256: "legacy-hash".to_string(),
                language: "rust".to_string(),
                symbols: vec![PendingSymbol {
                    name: "legacy".to_string(),
                    kind: "function".to_string(),
                    line_start: 1,
                    line_end: 2,
                    signature: Some("fn legacy()".to_string()),
                    parent_index: None,
                    decorators: None,
                }],
                edges: Vec::new(),
            })
            .unwrap();

        assert!(!store.concept_contract_current().unwrap());
        assert!(store.finalize_index_contracts_current().is_err());
        assert!(!store.concept_contract_current().unwrap());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn concept_schema_object_drift_repairs_additively_and_stays_dirty() {
        let path = tmp_db("concept_schema_object_drift");
        let store = Store::open(&path).unwrap();
        store.finalize_index_contracts_current().unwrap();
        assert!(store.concept_contract_current().unwrap());
        store
            .conn
            .execute_batch("DROP TRIGGER symbol_concepts_ai")
            .unwrap();
        assert!(!store.concept_contract_current().unwrap());

        store.ensure_concept_schema().unwrap();
        assert!(store.concept_schema_objects_current().unwrap());
        assert!(!store.concept_contract_current().unwrap());
        store.finalize_index_contracts_current().unwrap();
        assert!(store.concept_contract_current().unwrap());

        store
            .conn
            .execute_batch(
                "DROP TRIGGER symbol_concepts_ai;
                 CREATE TRIGGER symbol_concepts_ai
                 AFTER INSERT ON symbol_concepts BEGIN
                     INSERT INTO symbol_concepts_fts(
                         rowid, name_search, path_search, signature_search,
                         documentation_search
                     ) VALUES (
                         new.symbol_id, new.name_search, new.path_search,
                         new.signature_search, new.documentation_search
                     );
                     INSERT INTO meta(key, value) VALUES ('unexpected', 'extra');
                 END;",
            )
            .unwrap();
        assert!(!store.concept_schema_objects_current().unwrap());
        store.ensure_concept_schema().unwrap();
        assert!(store.concept_schema_objects_current().unwrap());
        assert!(!store.concept_contract_current().unwrap());

        store
            .conn
            .execute_batch(
                "DROP TRIGGER symbol_concepts_graph_ai;
                 CREATE TRIGGER symbol_concepts_graph_ai
                 AFTER INSERT ON symbols BEGIN
                     INSERT INTO meta(key, value)
                     VALUES ('CONCEPT_NORMALIZATION_VERSION', 'dirty')
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value
                     WHERE meta.value <> excluded.value;
                 END;",
            )
            .unwrap();
        assert!(!store.concept_schema_objects_current().unwrap());
        store.ensure_concept_schema().unwrap();
        assert!(store.concept_schema_objects_current().unwrap());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn concept_schema_contract_checks_and_repairs_every_fts_shadow_table() {
        for shadow in ["config", "data", "docsize", "idx"] {
            let path = tmp_db(&format!("concept_shadow_{shadow}"));
            let store = Store::open(&path).unwrap();
            store.finalize_index_contracts_current().unwrap();
            assert!(store.concept_contract_current().unwrap());

            store
                .conn
                .execute_batch(&format!("DROP TABLE symbol_concepts_fts_{shadow}"))
                .unwrap();
            assert!(!store.concept_schema_objects_current().unwrap());
            assert!(!store.concept_contract_current().unwrap());

            store.ensure_concept_schema().unwrap();
            assert!(store.concept_schema_objects_current().unwrap());
            assert!(!store.concept_contract_current().unwrap());
            store.finalize_index_contracts_current().unwrap();
            assert!(store.concept_contract_current().unwrap());
            std::fs::remove_file(path).ok();
        }
    }

    #[test]
    fn concept_shadow_repair_and_dirty_rebuild_are_one_transaction() {
        let path = tmp_db("concept_shadow_repair_atomic");
        let store = Store::open(&path).unwrap();
        store.finalize_index_contracts_current().unwrap();
        store
            .conn
            .execute_batch("DROP TABLE symbol_concepts_fts_data")
            .unwrap();
        assert!(!store.concept_schema_objects_current().unwrap());
        assert_eq!(
            store
                .meta_value(CONCEPT_NORMALIZATION_META_KEY)
                .unwrap()
                .as_deref(),
            Some(CONCEPT_NORMALIZATION_VERSION)
        );

        FAIL_CONCEPT_SCHEMA_AFTER_SHADOW_REPAIR.set(true);
        let result = store.ensure_concept_schema();
        FAIL_CONCEPT_SCHEMA_AFTER_SHADOW_REPAIR.set(false);
        assert!(result.is_err());
        assert!(!store.concept_schema_objects_current().unwrap());
        assert!(!store.concept_contract_current().unwrap());

        store.ensure_concept_schema().unwrap();
        assert!(store.concept_schema_objects_current().unwrap());
        assert!(!store.concept_contract_current().unwrap());
        store.finalize_index_contracts_current().unwrap();
        assert!(store.concept_contract_current().unwrap());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn concept_schema_repair_retires_orphaned_fts_shadow_tables() {
        let path = tmp_db("concept_orphan_shadows");
        let store = Store::open(&path).unwrap();
        store.finalize_index_contracts_current().unwrap();
        store
            .conn
            .execute_batch(
                "PRAGMA writable_schema = ON;
                 DELETE FROM sqlite_master
                 WHERE type = 'table' AND name = 'symbol_concepts_fts';
                 PRAGMA writable_schema = OFF;
                 PRAGMA schema_version = 99;",
            )
            .unwrap();
        assert!(!store.concept_schema_objects_current().unwrap());
        let orphan_count: u32 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name GLOB 'symbol_concepts_fts_*'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(orphan_count, 4);

        store.ensure_concept_schema().unwrap();
        assert!(store.concept_schema_objects_current().unwrap());
        assert!(!store.concept_contract_current().unwrap());
        store.finalize_index_contracts_current().unwrap();
        assert!(store.concept_contract_current().unwrap());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn concept_schema_repair_retires_wrong_type_owned_objects() {
        for (name, corruption) in [
            (
                "shadow_view",
                "DROP TABLE symbol_concepts_fts_data;
                 CREATE VIEW symbol_concepts_fts_data AS SELECT 1 AS id, x'' AS block;",
            ),
            (
                "shadow_table",
                "DROP TABLE symbol_concepts_fts_data;
                 CREATE TABLE symbol_concepts_fts_data(
                     id INTEGER PRIMARY KEY, wrong_column TEXT
                 );",
            ),
            (
                "virtual_view",
                "DROP TABLE symbol_concepts_fts;
                 CREATE VIEW symbol_concepts_fts AS SELECT 1 AS rowid;",
            ),
            (
                "content_view",
                "DROP TABLE symbol_concepts_fts;
                 DROP TABLE symbol_concepts;
                 CREATE VIEW symbol_concepts AS SELECT 1 AS symbol_id;",
            ),
            (
                "trigger_view",
                "DROP TRIGGER symbol_concepts_ai;
                 CREATE VIEW symbol_concepts_ai AS SELECT 1 AS value;",
            ),
        ] {
            let path = tmp_db(&format!("concept_wrong_type_{name}"));
            let store = Store::open(&path).unwrap();
            store.finalize_index_contracts_current().unwrap();
            store.conn.execute_batch(corruption).unwrap();
            assert!(!store.concept_schema_objects_current().unwrap());

            store.ensure_concept_schema().unwrap();
            assert!(store.concept_schema_objects_current().unwrap());
            assert!(!store.concept_contract_current().unwrap());
            store.finalize_index_contracts_current().unwrap();
            assert!(store.concept_contract_current().unwrap());
            std::fs::remove_file(path).ok();
        }
    }
}
