//! SQLite storage for the code graph.
//!
//! Schema:
//!   symbols(id, name, kind, file_path, line_start, line_end, signature, parent_id)
//!   edges(id, from_id, to_id?, to_name, kind, line)
//!   files(path, indexed_at, symbol_count)
//!   meta(key, value)

use rusqlite::{
    params, types::Value as SqlValue, Connection, OptionalExtension, Result as SqlResult,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: &str = "6";

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
    db_path: PathBuf,
}

impl Store {
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
            "#,
        )?;
        let store = Self { conn, db_path };
        store.init_schema()?;
        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn init_schema(&self) -> SqlResult<()> {
        // If a stored schema version exists and doesn't match, drop everything
        // and rebuild — we don't ship migrations.
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
                self.conn.execute_batch(
                    r#"
                    DROP TABLE IF EXISTS edges;
                    DROP TABLE IF EXISTS symbols;
                    DROP TABLE IF EXISTS files;
                    DROP TABLE IF EXISTS task_specs;
                    DROP TABLE IF EXISTS task_specs_fts;
                    DROP TABLE IF EXISTS meta;
                    "#,
                )?;
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
                decorators   TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
            CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_path);
            CREATE INDEX IF NOT EXISTS idx_symbols_parent ON symbols(parent_id);
            CREATE INDEX IF NOT EXISTS idx_symbols_language ON symbols(language);

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

            CREATE TABLE IF NOT EXISTS files (
                path                    TEXT PRIMARY KEY,
                indexed_at              INTEGER NOT NULL,
                symbol_count            INTEGER NOT NULL,
                structural_fingerprint  TEXT NOT NULL DEFAULT '',
                content_sha256          TEXT NOT NULL DEFAULT ''
            );

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

        // Stamp schema version on first init.
        self.conn.execute(
            "INSERT OR IGNORE INTO meta(key, value) VALUES ('schema_version', ?1)",
            params![SCHEMA_VERSION],
        )?;
        Ok(())
    }

    /// Wipe everything related to a single file before re-indexing it.
    /// Foreign keys CASCADE delete edges + child symbols.
    pub fn purge_file(&self, file_path: &str) -> SqlResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM symbols WHERE file_path = ?1",
            params![file_path],
        )?;
        tx.execute("DELETE FROM files WHERE path = ?1", params![file_path])?;
        tx.commit()?;
        Ok(())
    }

    /// Commit a parsed file's symbols and edges in a single transaction.
    /// Hot path during indexing — keep it batched.
    pub fn commit_file(&mut self, pending: PendingFile) -> SqlResult<()> {
        let tx = self.conn.transaction()?;

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
        let mut symbol_ids: Vec<i64> = Vec::with_capacity(pending.symbols.len());
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO symbols(name, kind, file_path, line_start, line_end, signature, parent_id, language, decorators)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for s in &pending.symbols {
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
                    s.decorators
                ])?;
                symbol_ids.push(tx.last_insert_rowid());
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
            "INSERT INTO files(path, indexed_at, symbol_count, structural_fingerprint, content_sha256) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &pending.path,
                pending.mtime,
                pending.symbols.len() as u32,
                &fingerprint,
                &pending.content_sha256
            ],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Wipe the entire index.
    pub fn purge_all(&self) -> SqlResult<()> {
        self.conn.execute_batch(
            r#"
            DELETE FROM edges;
            DELETE FROM symbols;
            DELETE FROM files;
            "#,
        )?;
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
        self.conn.execute(
            "INSERT INTO symbols(name, kind, file_path, line_start, line_end, signature, parent_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![name, kind, file_path, line_start, line_end, signature, parent_id],
        )?;
        Ok(self.conn.last_insert_rowid())
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

    pub fn upsert_file(&self, path: &str, mtime: i64, symbol_count: u32) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO files(path, indexed_at, symbol_count) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET indexed_at=?2, symbol_count=?3",
            params![path, mtime, symbol_count],
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

    /// Transitive callers up to `max_depth`, as (symbol, depth) pairs. Matches
    /// `to_name OR to_type` to catch type-method calls like `SessionStore::new()`.
    /// Optional `language` filter.
    pub fn impact_of(
        &self,
        name: &str,
        max_depth: u32,
        language: Option<&str>,
    ) -> SqlResult<Vec<(Symbol, u32)>> {
        // `d` must come AFTER SYMBOL_COLS_S so its index lines up with
        // `row_to_symbol`'s column count. Adjust the depth `r.get(N)` if you
        // change SYMBOL_COLS.
        let sql = format!(
            "WITH RECURSIVE impact(sym_id, name, depth) AS (
                 SELECT s.id, s.name, 1
                 FROM symbols s
                 JOIN edges e ON e.from_id = s.id
                 WHERE e.kind = 'calls'
                   AND (e.to_name = ?1 OR e.to_type = ?1)
                   AND (?3 IS NULL OR s.language = ?3)
               UNION
                 SELECT s.id, s.name, i.depth + 1
                 FROM symbols s
                 JOIN edges e ON e.from_id = s.id
                 JOIN impact i ON (e.to_name = i.name OR e.to_type = i.name)
                 WHERE i.depth < ?2
                   AND e.kind = 'calls'
                   AND (?3 IS NULL OR s.language = ?3)
             )
             SELECT {SYMBOL_COLS_S},
                    MIN(i.depth) AS d
             FROM impact i
             JOIN symbols s ON s.id = i.sym_id
             GROUP BY s.id
             ORDER BY d, s.file_path"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![name, max_depth, language], |r| {
            let sym = Self::row_to_symbol(r)?;
            // Depth is the column after the 9 SYMBOL_COLS_S columns.
            let depth: u32 = r.get(9)?;
            Ok((sym, depth))
        })?;
        rows.collect()
    }

    pub fn impact_of_many(
        &self,
        names: &[String],
        max_depth: u32,
        row_limit: usize,
    ) -> SqlResult<Vec<SeedImpact>> {
        if names.is_empty() || names.len() > 200 {
            return Err(rusqlite::Error::InvalidParameterName(
                "seed_count".to_string(),
            ));
        }
        if !(1..=5).contains(&max_depth) {
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
        let sql = format!(
            "WITH RECURSIVE seed(seed) AS (VALUES {placeholders}),
             walk(seed, sym_id, name, depth, visited) AS (
                 SELECT seed.seed, s.id, s.name, 1, ',' || s.id || ','
                 FROM seed
                 JOIN edges e ON e.kind = 'calls'
                              AND (e.to_name = seed.seed OR e.to_type = seed.seed)
                 JOIN symbols s ON s.id = e.from_id
               UNION ALL
                 SELECT walk.seed, s.id, s.name, walk.depth + 1,
                        walk.visited || s.id || ','
                 FROM walk
                 JOIN edges e ON e.kind = 'calls'
                              AND (e.to_name = walk.name OR e.to_type = walk.name)
                 JOIN symbols s ON s.id = e.from_id
                 WHERE walk.depth < ?{depth_param}
                   AND instr(walk.visited, ',' || s.id || ',') = 0
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

        let started = std::time::Instant::now();
        let operations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handler_operations = operations.clone();
        self.conn.progress_handler(
            1_000,
            Some(move || {
                handler_operations.fetch_add(1, std::sync::atomic::Ordering::Relaxed) > 250_000
                    || started.elapsed() > std::time::Duration::from_secs(2)
            }),
        )?;
        let result = (|| {
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
        })();
        self.conn.progress_handler(0, None::<fn() -> bool>)?;
        result
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
        let sql = format!(
            "SELECT {SYMBOL_COLS_S}
             FROM symbols s
             WHERE (?1 IS NULL OR s.kind = ?1)
               AND (?2 IS NULL OR s.language = ?2)
               AND s.kind != 'module'
               -- Module-level constants are referenced by VALUE-READ, not
               -- call/import edges, so they'd dominate the default output as
               -- false positives. Exclude unless caller asked for `kind=constant`
               -- (then the kind filter controls the slice).
               AND (?1 IS NOT NULL OR s.kind != 'constant')
               AND NOT EXISTS (
                   SELECT 1 FROM edges e
                   WHERE e.to_name = s.name OR e.to_type = s.name
               )
               -- pytest test functions by convention (test_* in *test*/*spec* files)
               AND NOT (
                   s.name LIKE 'test_%'
                   AND (s.file_path LIKE '%test%' OR s.file_path LIKE '%spec%')
               )
               -- Symbols decorated by framework registries
               AND (s.decorators IS NULL OR (
                   -- pytest
                   s.decorators NOT LIKE '%,fixture,%'
                   AND s.decorators NOT LIKE '%,pytest.fixture,%'
                   AND s.decorators NOT LIKE '%,parametrize,%'
                   AND s.decorators NOT LIKE '%,pytest.mark.parametrize,%'
                   AND s.decorators NOT LIKE '%,pytest.mark.%'
                   -- web frameworks (FastAPI, Flask, Quart, etc.)
                   AND s.decorators NOT LIKE '%.route,%'
                   AND s.decorators NOT LIKE '%.get,%'
                   AND s.decorators NOT LIKE '%.post,%'
                   AND s.decorators NOT LIKE '%.put,%'
                   AND s.decorators NOT LIKE '%.delete,%'
                   AND s.decorators NOT LIKE '%.patch,%'
                   AND s.decorators NOT LIKE '%.websocket,%'
                   -- JIT compilers
                   AND s.decorators NOT LIKE '%triton.jit,%'
                   AND s.decorators NOT LIKE '%numba.jit,%'
                   AND s.decorators NOT LIKE '%numba.njit,%'
                   AND s.decorators NOT LIKE '%nb.njit,%'
                   AND s.decorators NOT LIKE '%,jit,%'
                   AND s.decorators NOT LIKE '%,njit,%'
                   -- Celery / task queues
                   AND s.decorators NOT LIKE '%celery.task,%'
                   AND s.decorators NOT LIKE '%shared_task,%'
                   AND s.decorators NOT LIKE '%,task,%'
                   -- CLI (Click, Typer)
                   AND s.decorators NOT LIKE '%click.command,%'
                   AND s.decorators NOT LIKE '%click.group,%'
                   AND s.decorators NOT LIKE '%,command,%'
                   AND s.decorators NOT LIKE '%,callback,%'
                   -- Rust attributes
                   AND s.decorators NOT LIKE '%,test,%'
                   AND s.decorators NOT LIKE '%,tokio::test,%'
                   AND s.decorators NOT LIKE '%,tokio::main,%'
                   AND s.decorators NOT LIKE '%,async_std::main,%'
                   AND s.decorators NOT LIKE '%,async_std::test,%'
                   -- C# test frameworks (xUnit / NUnit / MSTest) — leaf only, Attribute suffix stripped
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
                   -- ASP.NET routing
                   AND s.decorators NOT LIKE '%,HttpGet,%'
                   AND s.decorators NOT LIKE '%,HttpPost,%'
                   AND s.decorators NOT LIKE '%,HttpPut,%'
                   AND s.decorators NOT LIKE '%,HttpDelete,%'
                   AND s.decorators NOT LIKE '%,HttpPatch,%'
                   AND s.decorators NOT LIKE '%,Route,%'
                   -- BenchmarkDotNet
                   AND s.decorators NOT LIKE '%,Benchmark,%'
                   AND s.decorators NOT LIKE '%,GlobalSetup,%'
                   AND s.decorators NOT LIKE '%,GlobalCleanup,%'
                   -- Java polymorphic dispatch — `@Override` means a parent's
                   -- callsites resolve here, invisible to mmcg's call graph.
                   AND s.decorators NOT LIKE '%,Override,%'
                   -- Java test frameworks (JUnit 4/5, TestNG)
                   AND s.decorators NOT LIKE '%,Test,%'
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
                   -- Spring routing / DI
                   AND s.decorators NOT LIKE '%,RequestMapping,%'
                   AND s.decorators NOT LIKE '%,GetMapping,%'
                   AND s.decorators NOT LIKE '%,PostMapping,%'
                   AND s.decorators NOT LIKE '%,PutMapping,%'
                   AND s.decorators NOT LIKE '%,DeleteMapping,%'
                   AND s.decorators NOT LIKE '%,PatchMapping,%'
                   AND s.decorators NOT LIKE '%,Bean,%'
                   AND s.decorators NOT LIKE '%,Scheduled,%'
                   AND s.decorators NOT LIKE '%,EventListener,%'
                   -- PHP test / web (PHP 8 attributes — same leaf-name convention)
                   AND s.decorators NOT LIKE '%,DataProvider,%'
                   AND s.decorators NOT LIKE '%,TestDox,%'
                   AND s.decorators NOT LIKE '%,Group,%'
                   AND s.decorators NOT LIKE '%,Route,%'
                   AND s.decorators NOT LIKE '%,AsCommand,%'
                   AND s.decorators NOT LIKE '%,AsController,%'
                   AND s.decorators NOT LIKE '%,AsEventListener,%'
                   AND s.decorators NOT LIKE '%,On,%'
               ))
             ORDER BY s.file_path, s.line_start"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![kind, language], Self::row_to_symbol)?;
        rows.collect()
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
            "SELECT DISTINCT {SYMBOL_COLS_S}
             FROM symbols s
             WHERE s.file_path LIKE ?1
               AND (?2 IS NULL OR s.language = ?2)
               AND s.kind != 'module'
               AND EXISTS (
                   SELECT 1 FROM edges e
                   JOIN symbols caller ON caller.id = e.from_id
                   WHERE (e.to_name = s.name OR e.to_type = s.name)
                     AND caller.file_path NOT LIKE ?1
               )
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
        let mut stmt = self.conn.prepare(
            "SELECT path
             FROM files
             WHERE ?2 = 'root'
                OR (?2 = 'file' AND path = ?1)
                OR (
                    ?2 = 'directory'
                    AND substr(path, 1, length(?1) + 1) = ?1 || '/'
                )
             ORDER BY path
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![scope, kind, limit as i64], |row| row.get(0))?;
        rows.collect()
    }

    pub fn map_boundaries(
        &self,
        components: &[MapBoundaryScope],
        limit_per_component: usize,
        global_limit: usize,
    ) -> SqlResult<Vec<MapBoundaryRow>> {
        if components.is_empty() || limit_per_component == 0 || global_limit == 0 {
            return Ok(Vec::new());
        }
        let placeholders = (0..components.len())
            .map(|index| {
                let first = index * 3 + 1;
                format!("(?{first}, ?{}, ?{})", first + 1, first + 2)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let per_component_param = components.len() * 3 + 1;
        let global_param = components.len() * 3 + 2;
        let sql = format!(
            "WITH component(component, path, direct_only) AS (VALUES {placeholders}),
             ranked AS (
                 SELECT c.component,
                        {SYMBOL_COLS_S},
                        COALESCE(parent.file_path, '') AS parent_file_path,
                        COALESCE(parent.line_start, -1) AS parent_line_start,
                        COALESCE(parent.name, '') AS parent_name,
                        COALESCE(parent.kind, '') AS parent_kind,
                        COALESCE(parent.line_end, -1) AS parent_line_end,
                        COALESCE(parent.signature, '') AS parent_signature,
                        COALESCE(parent.decorators, '') AS parent_decorators,
                        ROW_NUMBER() OVER (
                            PARTITION BY c.component
                            ORDER BY s.file_path, s.line_start, s.name, s.kind, s.line_end,
                                     COALESCE(s.signature, ''), COALESCE(s.decorators, ''),
                                     COALESCE(parent.file_path, ''),
                                     COALESCE(parent.line_start, -1),
                                     COALESCE(parent.name, ''), COALESCE(parent.kind, ''),
                                     COALESCE(parent.line_end, -1),
                                     COALESCE(parent.signature, ''),
                                     COALESCE(parent.decorators, '')
                        ) AS position
                 FROM component c
                 JOIN symbols s
                   ON (
                       c.direct_only = 1
                       AND (
                           (c.path = '' AND instr(s.file_path, '/') = 0)
                           OR (
                               c.path != ''
                               AND substr(s.file_path, 1, length(c.path) + 1) = c.path || '/'
                               AND instr(substr(s.file_path, length(c.path) + 2), '/') = 0
                           )
                       )
                   ) OR (
                       c.direct_only = 0
                       AND substr(s.file_path, 1, length(c.path) + 1) = c.path || '/'
                   )
                 LEFT JOIN symbols parent ON parent.id = s.parent_id
                 WHERE s.kind != 'module'
                   AND EXISTS (
                       SELECT 1
                       FROM edges e
                       JOIN symbols caller ON caller.id = e.from_id
                       WHERE e.kind = 'calls'
                         AND (e.to_name = s.name OR e.to_type = s.name)
                         AND NOT (
                             (
                                 c.direct_only = 1
                                 AND (
                                     (c.path = '' AND instr(caller.file_path, '/') = 0)
                                     OR (
                                         c.path != ''
                                         AND substr(caller.file_path, 1, length(c.path) + 1)
                                             = c.path || '/'
                                         AND instr(
                                             substr(caller.file_path, length(c.path) + 2),
                                             '/'
                                         ) = 0
                                     )
                                 )
                             ) OR (
                                 c.direct_only = 0
                                 AND substr(caller.file_path, 1, length(c.path) + 1)
                                     = c.path || '/'
                             )
                         )
                   )
             )
             SELECT component,
                    id, name, kind, file_path, line_start, line_end, signature, parent_id, decorators
             FROM ranked
             WHERE position <= ?{per_component_param}
             ORDER BY component, file_path, line_start, name, kind, line_end,
                      COALESCE(signature, ''), COALESCE(decorators, ''),
                      parent_file_path, parent_line_start, parent_name, parent_kind,
                      parent_line_end, parent_signature, parent_decorators
             LIMIT ?{global_param}"
        );
        let mut values = Vec::with_capacity(components.len() * 3 + 2);
        for component in components {
            values.push(SqlValue::Text(component.label.clone()));
            values.push(SqlValue::Text(component.path.clone()));
            values.push(SqlValue::Integer(match component.match_mode {
                MapBoundaryMatch::Direct => 1,
                MapBoundaryMatch::Recursive => 0,
            }));
        }
        values.push(SqlValue::Integer(limit_per_component as i64));
        values.push(SqlValue::Integer(global_limit as i64));
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values), |row| {
            Ok(MapBoundaryRow {
                component: row.get(0)?,
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
            })
        })?;
        rows.collect()
    }

    pub fn map_centrality(
        &self,
        scope: &str,
        kind: &str,
        top_probe: usize,
    ) -> SqlResult<Vec<MapCentralityRow>> {
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
                   AND (
                       ?2 = 'root'
                       OR (?2 = 'file' AND s.file_path = ?1)
                       OR (
                           ?2 = 'directory'
                           AND substr(s.file_path, 1, length(?1) + 1) = ?1 || '/'
                       )
                   )
             ),
             degrees AS (
                 SELECT d.id, COUNT(DISTINCT e.from_id) AS in_degree
                 FROM scoped_defs d
                 JOIN edges e
                   ON e.kind = 'calls'
                  AND (e.to_name = d.name OR e.to_type = d.name)
                 GROUP BY d.id
             ),
             scoped_names AS (
                 SELECT DISTINCT name
                 FROM scoped_defs
             ),
             collisions AS (
                 SELECT n.name,
                        (
                            SELECT COUNT(*)
                            FROM symbols s INDEXED BY idx_symbols_name
                            WHERE s.kind != 'module' AND s.name = n.name
                        ) AS name_collision
                 FROM scoped_names n
             )
             SELECT d.id, d.name, d.kind, d.file_path, d.line_start, d.line_end,
                    d.signature, d.parent_id, d.decorators,
                    degrees.in_degree, collisions.name_collision
             FROM scoped_defs d
             JOIN degrees ON degrees.id = d.id
             JOIN collisions ON collisions.name = d.name
             ORDER BY degrees.in_degree DESC, d.file_path, d.line_start, d.name,
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
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT source.file_path, target.file_path
             FROM edges e
             JOIN symbols source ON source.id = e.from_id
             JOIN symbols target ON target.name = e.to_name
             WHERE e.kind = 'imports'
               AND source.file_path != target.file_path
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
             ORDER BY source.file_path, target.file_path
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![scope, kind, limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        rows.collect()
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

    /// Strongly-connected components of size ≥ `min_size` in the file-level
    /// import graph. A returned SCC = a circular-import group.
    ///
    /// Edges resolved by **leaf name match** — each `imports` edge targets every
    /// file with a same-named symbol. Over-approximates: two unrelated `Logger`
    /// symbols produce a cross-edge even if the importer meant neither. Upside:
    /// no extractor-specific import resolution. Downside: false-positive cycles
    /// to verify manually.
    ///
    /// Self-edges excluded (`from_file = to_file`).
    ///
    /// `min_size` defaults to 2 (smallest cycle). Higher surfaces only larger
    /// problems (min_size=3 hides trivial A→B→A).
    pub fn dependency_cycles(
        &self,
        language: Option<&str>,
        min_size: usize,
    ) -> SqlResult<Vec<Vec<String>>> {
        // File-level adjacency: from_file → set of to_files.
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT
                s_from.file_path AS from_file,
                s_to.file_path   AS to_file
             FROM edges e
             JOIN symbols s_from ON s_from.id = e.from_id
             JOIN symbols s_to   ON s_to.name = e.to_name
             WHERE e.kind = 'imports'
               AND s_from.file_path != s_to.file_path
               AND (?1 IS NULL OR s_from.language = ?1)
               AND (?1 IS NULL OR s_to.language = ?1)
             ORDER BY from_file, to_file",
        )?;
        let rows = stmt.query_map(params![language], |r| {
            let from: String = r.get(0)?;
            let to: String = r.get(1)?;
            Ok((from, to))
        })?;

        let mut adj: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for row in rows {
            let (from, to) = row?;
            adj.entry(from).or_default().push(to);
        }

        let cycles = tarjan_scc(&adj);
        let mut out: Vec<Vec<String>> =
            cycles.into_iter().filter(|c| c.len() >= min_size).collect();
        // Stable order: largest cycles first, lex within.
        out.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        Ok(out)
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
    use std::env;

    /// Unique path per test — parallel tests can't share the file.
    fn tmp_db(test_name: &str) -> PathBuf {
        let mut p = env::temp_dir();
        p.push(format!("mmcg-test-{}-{}.db", std::process::id(), test_name));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn schema_initializes() {
        let path = tmp_db("schema_initializes");
        let store = Store::open(&path).unwrap();
        assert_eq!(store.symbol_count().unwrap(), 0);
        std::fs::remove_file(&path).ok();
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

        // impact of c should include b (depth 1) and a (depth 2)
        let imp = store.impact_of("c", 5, None).unwrap();
        let names: Vec<&str> = imp.iter().map(|(s, _)| s.name.as_str()).collect();
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

        let cycles = store.dependency_cycles(None, 2).unwrap();
        assert_eq!(cycles.len(), 1, "exactly one cycle expected");
        assert_eq!(cycles[0], vec!["a.py".to_string(), "b.py".to_string()]);

        // min_size=3 hides the 2-node cycle entirely.
        let bigger = store.dependency_cycles(None, 3).unwrap();
        assert!(bigger.is_empty());

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
    fn map_boundaries_use_one_batched_statement_and_exact_external_scope() {
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
            .impact_of_many(&["beta".to_string(), "alpha".to_string()], 3, 5001)
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
        assert!(store.impact_of_many(&[], 1, 1).is_err());
        assert!(store
            .impact_of_many(&vec!["seed".to_string(); 201], 1, 1)
            .is_err());
        assert!(store.impact_of_many(&["seed".to_string()], 0, 1).is_err());
        assert!(store
            .impact_of_many(&["seed".to_string()], 1, 5002)
            .is_err());
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
        let result = store.impact_of_many(&["node0".to_string()], 5, 5001);
        assert!(result.is_err() || result.as_ref().is_ok_and(|rows| rows.len() <= 5001));
        std::fs::remove_file(&path).ok();
    }
}
