//! SQLite storage for the code graph.
//!
//! Schema:
//!   symbols(id, name, kind, file_path, line_start, line_end, signature, parent_id)
//!   edges(id, from_id, to_id?, to_name, kind, line)
//!   files(path, indexed_at, symbol_count)
//!   meta(key, value)

use rusqlite::{params, Connection, OptionalExtension, Result as SqlResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: &str = "5";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub id: i64,
    pub name: String,
    pub kind: String, // "function" | "class" | "method"
    pub file_path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub signature: Option<String>,
    pub parent_id: Option<i64>,
    /// Comma-bookended list of decorators / attributes / modifiers
    /// (e.g. `",Fact,"`, `",partial,sealed,"`). `None` if no modifiers.
    /// Used by `mmcg_unreferenced` filtering and by `mmcg_search` partial-class collapse.
    pub decorators: Option<String>,
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

/// Per-file batch ready to be committed in a single transaction.
/// Symbols hold local indices; the store resolves them to rowids at commit time.
#[derive(Debug, Default)]
pub struct PendingFile {
    pub path: String,
    pub mtime: i64,
    /// Programming language identifier — `python`, `typescript`, `tsx`,
    /// `javascript`, `rust`. Stored on every symbol of this file. Enables the
    /// `language` filter in queries (defends against cross-language name collisions
    /// in monorepos).
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
    /// Decorator / attribute names attached to this symbol, comma-delimited with
    /// leading and trailing commas for safe `LIKE ',name,'` matching. e.g.
    /// `,pytest.fixture,property,` or `,tokio::main,` or None for none.
    pub decorators: Option<String>,
}

#[derive(Debug)]
pub struct PendingEdge {
    /// Index into `symbols` vec of the symbol making the call/import.
    pub from_index: usize,
    /// Leaf name — `foo` in `obj.foo()`, `baz` in `from a.b import baz`.
    pub to_name: String,
    /// Fully-qualified path as it appears in source — `obj.foo`, `a.b.baz`,
    /// `Foo::bar`. None if the call/import has no resolvable path beyond the leaf.
    pub to_path: Option<String>,
    /// Type/namespace prefix for the call — `SessionStore` for `SessionStore::new()`,
    /// `Foo` for `Foo::bar()`. None if there's no type prefix (free function, plain
    /// method on a variable). Used to make `mmcg_callers <Type>` find Rust constructor
    /// and associated-function calls that would otherwise hide under their leaf name.
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
        // Detect existing schema version. If it exists and doesn't match,
        // drop everything and rebuild — we don't ship migrations.
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
                    "[mmcg] schema version mismatch (have {:?}, need {}). Rebuilding — re-run `mmcg index <root>` to repopulate.",
                    stored, SCHEMA_VERSION
                );
                self.conn.execute_batch(
                    r#"
                    DROP TABLE IF EXISTS edges;
                    DROP TABLE IF EXISTS symbols;
                    DROP TABLE IF EXISTS files;
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
                path          TEXT PRIMARY KEY,
                indexed_at    INTEGER NOT NULL,
                symbol_count  INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;

        // Stamp schema version on first init
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
    /// This is the hot path during indexing — keep it batched.
    pub fn commit_file(&mut self, pending: PendingFile) -> SqlResult<()> {
        let tx = self.conn.transaction()?;

        // Purge any existing data for this file
        tx.execute(
            "DELETE FROM symbols WHERE file_path = ?1",
            params![&pending.path],
        )?;
        tx.execute("DELETE FROM files WHERE path = ?1", params![&pending.path])?;

        // Insert symbols, remember the rowid each one got
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

        // Insert edges (to_id left NULL — we resolve by name/type during queries)
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

        // Stamp the file
        tx.execute(
            "INSERT INTO files(path, indexed_at, symbol_count) VALUES (?1, ?2, ?3)",
            params![&pending.path, pending.mtime, pending.symbols.len() as u32],
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

    /// All paths currently in the index. Used by the indexer to detect deletions —
    /// any path that's no longer on disk gets purged at the end of an index run.
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

    /// Callers of a symbol — symbols connected to it via an edge whose `to_name`
    /// OR `to_type` matches. The `to_type` match catches Rust constructor /
    /// associated-function calls like `SessionStore::new()` that would otherwise
    /// hide under the leaf name (`new`). Optional `language` filter scopes the
    /// search (defends against cross-language name collisions in monorepos).
    ///
    /// `edge_kind`:
    ///   - `None` → defaults to `'calls'` (preserves the historical "who calls X" meaning)
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

    /// Callees of a symbol-id — names that the symbol references via the given
    /// edge kind. `edge_kind = None` defaults to `'calls'`.
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

    /// Transitive callers up to `max_depth`. Returns (symbol, depth) pairs.
    /// Matches on `to_name OR to_type` to catch type-method calls like
    /// `SessionStore::new()`. Optional `language` filter.
    pub fn impact_of(
        &self,
        name: &str,
        max_depth: u32,
        language: Option<&str>,
    ) -> SqlResult<Vec<(Symbol, u32)>> {
        // `d` must be appended AFTER the SYMBOL_COLS_S list so its index lines up
        // with `row_to_symbol`'s column count. Adjust the `r.get(N)` for depth
        // if you change SYMBOL_COLS.
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
            // Depth is the column AFTER the 9 SYMBOL_COLS_S columns.
            let depth: u32 = r.get(9)?;
            Ok((sym, depth))
        })?;
        rows.collect()
    }

    /// Symbols that no edge references — neither by `to_name` nor `to_type`.
    /// Excludes synthetic `<module>` rows (never "called" by definition) and
    /// symbols decorated with framework-registered patterns (pytest, FastAPI/
    /// Flask routes, Triton/Numba JIT, Click commands, Celery tasks, Rust
    /// `#[test]` / `#[tokio::main]`). Also excludes pytest-convention test
    /// functions (`test_*` names in test files).
    ///
    /// **Remaining false-positives** (caller responsibility):
    /// - Entry points (`main`, framework-registered handlers without decorators)
    /// - Dynamic dispatch / reflection / trait objects whose calls don't surface
    /// - Cross-language calls
    /// - Functions registered via dict / list at runtime
    ///
    /// Optional `kind` (e.g. "function") and `language` filters scope the result.
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
               AND NOT EXISTS (
                   SELECT 1 FROM edges e
                   WHERE e.to_name = s.name OR e.to_type = s.name
               )
               -- Filter out pytest test functions by convention (test_* in *test* files)
               AND NOT (
                   s.name LIKE 'test_%'
                   AND (s.file_path LIKE '%test%' OR s.file_path LIKE '%spec%')
               )
               -- Filter out symbols decorated by framework registries
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
                   -- callsites resolve here; not visible to mmcg's call graph.
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

    /// Symbols defined in files under `path_prefix` that are referenced from
    /// at least one file OUTSIDE the prefix. "Empirical API surface" — independent
    /// of declared visibility (which mmcg doesn't extract).
    ///
    /// `path_prefix` is matched via SQL `LIKE` — pass the prefix without `%`; we append it.
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
    /// (matched by `to_name` or `to_type`, like `callers_of`). The top of the
    /// list is the codebase's structural attractor surface: utilities everyone
    /// depends on, core domain types, framework registration points.
    ///
    /// Use as a planner pre-flight on an unfamiliar codebase or path prefix:
    /// "what are the 20 most-referenced symbols in `src/auth/`?" answers
    /// "what should I read first" cheaply.
    ///
    /// - `path_prefix`: limit to symbols whose `file_path` starts with this
    ///   prefix. `None` = whole index. Trailing `%` accepted, otherwise we append.
    /// - `language`, `kind`: standard filters.
    /// - `top`: how many results to return (caller decides — no hard cap).
    ///
    /// Excludes synthetic `<module>` symbols (always-zero in-degree under
    /// name-matched edges) and symbols not referenced anywhere (in-degree 0).
    pub fn centrality(
        &self,
        path_prefix: Option<&str>,
        language: Option<&str>,
        kind: Option<&str>,
        top: u32,
    ) -> SqlResult<Vec<(Symbol, u32)>> {
        let pattern = path_prefix.map(|p| {
            if p.ends_with('%') {
                p.to_string()
            } else {
                format!("{p}%")
            }
        });
        // In-degree = distinct CALLER symbols (not distinct call sites). Mirrors
        // `mmcg_callers` semantics — a function with 5 calls to `foo` from the
        // same caller counts once, not five times.
        let sql = format!(
            "SELECT {SYMBOL_COLS_S}, COUNT(DISTINCT e.from_id) AS in_degree
             FROM symbols s
             JOIN edges e ON e.kind = 'calls'
                         AND (e.to_name = s.name OR e.to_type = s.name)
             WHERE s.kind != 'module'
               AND (?1 IS NULL OR s.file_path LIKE ?1)
               AND (?2 IS NULL OR s.language = ?2)
               AND (?3 IS NULL OR s.kind = ?3)
             GROUP BY s.id
             ORDER BY in_degree DESC, s.file_path, s.line_start
             LIMIT ?4"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![pattern, language, kind, top], |r| {
            let sym = Self::row_to_symbol(r)?;
            // in_degree is the column after the 9 SYMBOL_COLS_S columns.
            let in_degree: u32 = r.get(9)?;
            Ok((sym, in_degree))
        })?;
        rows.collect()
    }

    /// Files indexed within the last `window_secs` seconds (based on `indexed_at`).
    /// Used by `mmcg_recent_changes` to answer "what has the watcher touched lately".
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

    /// Files indexed under a path prefix (path = None means everything).
    /// Optional `language` filter — uses EXISTS subquery on symbols table since
    /// language lives there, not on files. Files with zero symbols are excluded
    /// when `language` is set (in practice every file has at least the synthetic
    /// `<module>` symbol, so this is a no-op for indexed files).
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

    /// Files whose module imports the given name. Matches `to_name` (leaf binding).
    /// Optional `language` filter scopes the search to a single language.
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

    /// Files whose module imports something at exactly this fully-qualified path.
    /// Matches `to_path` precisely — use this when the same leaf name is imported
    /// from multiple modules and you only want the ones from a specific module.
    /// Optional `language` filter.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    /// Unique path per test — tests run in parallel so we can't share the file.
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
        // Module imports `target`; caller_fn calls `target`. Same to_name, different kinds.
        store
            .insert_edge(module, None, "target", "imports", 2)
            .unwrap();
        store
            .insert_edge(caller, None, "target", "calls", 12)
            .unwrap();

        // Default (None) → 'calls' only — finds caller_fn, not module
        let default_callers = store.callers_of("target", None, None).unwrap();
        assert_eq!(default_callers.len(), 1);
        assert_eq!(default_callers[0].name, "caller_fn");

        // edge_kind = 'imports' — finds module, not caller_fn
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
        // Direct insert via the connection — bypass the high-level insert_symbol API to
        // include decorators column. Set up: 3 functions, none called by anything.
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
        // plain_dead survives — actually unreferenced, no decorator, no test pattern
        assert!(
            names.contains(&"plain_dead"),
            "plain_dead is genuinely unreferenced"
        );
        // db is filtered out — has pytest.fixture decorator
        assert!(
            !names.contains(&"db"),
            "db is filtered by @pytest.fixture decorator"
        );
        // test_foo is filtered out — test_* in test file
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
        // foo calls bar — bar referenced; foo and orphan have no incoming edges
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
        // pub_fn lives in src/api/, called from src/main.rs (OUTSIDE prefix)
        let main_mod = store
            .insert_symbol("<module>", "module", "src/main.rs", 1, 1, None, None)
            .unwrap();
        let _pub_fn = store
            .insert_symbol("pub_fn", "function", "src/api/lib.rs", 3, 5, None, None)
            .unwrap();
        store
            .insert_edge(main_mod, None, "pub_fn", "calls", 10)
            .unwrap();

        // internal_fn lives in src/api/, called only from src/api/util.rs (INSIDE prefix)
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
        // Same caller calling popular twice → still in_degree=1 (DISTINCT callers).
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
            .map(|(s, deg)| (s.name.as_str(), *deg))
            .collect();
        assert_eq!(
            by_name["popular"], 3,
            "3 distinct callers, dup call ignored"
        );
        assert_eq!(by_name["medium"], 1);
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

        // Prefix filter: src/api/ excludes the class in src/core/.
        let api_only = store.centrality(Some("src/api/"), None, None, 10).unwrap();
        let names: Vec<&str> = api_only.iter().map(|(s, _)| s.name.as_str()).collect();
        assert!(names.contains(&"api_target"));
        assert!(!names.contains(&"CoreClass"));

        // Kind filter: class only.
        let classes = store.centrality(None, None, Some("class"), 10).unwrap();
        let class_names: Vec<&str> = classes.iter().map(|(s, _)| s.name.as_str()).collect();
        assert!(class_names.contains(&"CoreClass"));
        assert!(!class_names.contains(&"api_target"));
        std::fs::remove_file(&path).ok();
    }
}
