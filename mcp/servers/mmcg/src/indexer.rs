//! Multi-language code indexer.
//!
//! Each supported language is implemented as a [`LanguageExtractor`] in its
//! own submodule. The dispatch logic here walks the file tree, picks the right
//! extractor per file extension, parses with tree-sitter in parallel via
//! rayon, and serializes writes through a single SQLite connection.

use crate::store::{PendingFile, PendingSymbol, Store};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tree_sitter::{Parser, Tree};
use walkdir::WalkDir;

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

/// Per-language symbol/edge extractor.
///
/// Implementors receive a parsed tree-sitter tree plus the source bytes and
/// append symbols and edges to a [`PendingFile`]. The synthetic module symbol
/// is added at index `module_index` before `extract` is called — use it as
/// the parent for top-level calls and as the source of import edges.
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
        // C / C++ share one tree-sitter-cpp grammar. Acceptable since C is mostly
        // a C++ subset; rare C-only keyword identifiers (e.g. `new` as a var)
        // may mis-parse — see cpp.rs precision disclaimer.
        "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hh" | "hxx" | "ipp" | "tpp" => {
            Some(Box::new(CppExtractor))
        }
        _ => None,
    }
}

/// Directory names skipped during the walk. Cheap to extend.
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
    /// Files seen during the walk (including ones we don't have an extractor for).
    pub files_scanned: u32,
    /// Files we actually parsed and committed (re-indexed this run).
    pub files_indexed: u32,
    /// Files that failed to parse or commit.
    pub files_failed: u32,
    /// Files with extensions we don't support — skipped before parsing.
    pub files_skipped: u32,
    /// Indexable files whose stored mtime is current — skipped without parsing.
    pub files_unchanged: u32,
    /// Files that were in the index but no longer exist on disk — purged.
    pub files_purged: u32,
    pub symbols_total: u32,
    pub edges_total: u32,
    pub by_language: std::collections::BTreeMap<String, u32>,
    pub duration_ms: u128,
}

pub struct Indexer {
    root: PathBuf,
}

impl Indexer {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Index everything reachable from `root`. Incremental by default — files whose
    /// filesystem mtime is `<=` their stored mtime are skipped. Pass `force_full=true`
    /// to re-index everything regardless of mtime (e.g., after a schema change or to
    /// recover from a corrupted index).
    ///
    /// Files that exist in the index but no longer on disk are purged at the end.
    /// Writes to `store`.
    pub fn index_all(&self, store: &mut Store, force_full: bool) -> Result<IndexStats, IndexError> {
        let start = SystemTime::now();

        // Phase 1: walk filesystem
        let candidates: Vec<PathBuf> = WalkDir::new(&self.root)
            .into_iter()
            .filter_entry(|e| !is_skipped_dir(e.file_name().to_str().unwrap_or("")))
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .collect();

        let mut stats = IndexStats {
            files_scanned: candidates.len() as u32,
            ..Default::default()
        };

        // Phase 2: classify each candidate serially (cheap — one indexed lookup per file).
        // Build (to_parse, current_paths) for phases 3-5.
        let mut to_parse: Vec<PathBuf> = Vec::new();
        let mut current_paths: std::collections::HashSet<String> =
            std::collections::HashSet::with_capacity(candidates.len());

        for path in &candidates {
            if extractor_for_path(path).is_none() {
                stats.files_skipped += 1;
                continue;
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

        // Phase 3: parse stale files in parallel (extractor lookup is cheap, redo it)
        let parsed: Vec<Result<PendingFile, IndexError>> = to_parse
            .par_iter()
            .filter_map(|p| {
                let extractor = extractor_for_path(p)?;
                Some(parse_one(p, &self.root, extractor.as_ref()))
            })
            .collect();

        // Phase 4: commit serially (SQLite single-writer)
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
                Err(_) => stats.files_failed += 1,
            }
        }

        // Phase 5: purge orphans (files in index but not on disk anymore).
        // Only safe to do when we've scanned the FULL root — partial scans would
        // wrongly purge anything outside the scanned subtree.
        if let Ok(indexed) = store.indexed_paths() {
            for path in indexed {
                if !current_paths.contains(&path) && store.purge_file(&path).is_ok() {
                    stats.files_purged += 1;
                }
            }
        }

        stats.duration_ms = start.elapsed().map(|d| d.as_millis()).unwrap_or(0);
        Ok(stats)
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

fn is_skipped_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

fn guess_language_for(rel_path: &str) -> Option<&'static str> {
    if rel_path.ends_with(".py") {
        Some("python")
    } else if rel_path.ends_with(".tsx") {
        Some("tsx")
    } else if rel_path.ends_with(".ts") {
        Some("typescript")
    } else if rel_path.ends_with(".jsx") {
        Some("jsx")
    } else if rel_path.ends_with(".js") || rel_path.ends_with(".mjs") || rel_path.ends_with(".cjs")
    {
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
    let source = std::fs::read(path).map_err(|e| IndexError::Io(e.to_string()))?;
    // Milliseconds — second-precision was missing edits made in the same second
    // as the previous index run. i64 millis covers ~292M years.
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

    let mut parser = Parser::new();
    let language = extractor.language();
    parser
        .set_language(&language)
        .map_err(|e| IndexError::Parse(e.to_string()))?;
    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| IndexError::Parse("tree-sitter parse returned None".to_string()))?;

    let line_count = source.iter().filter(|&&b| b == b'\n').count() as u32 + 1;
    let language = guess_language_for(&rel).unwrap_or("").to_string();
    let mut pending = PendingFile {
        path: rel.clone(),
        mtime,
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
        signature: Some(format!("module {rel}")),
        parent_index: None,
        decorators: None,
    });
    let module_index = pending.symbols.len() - 1;

    extractor.extract(&tree, &source, &mut pending, module_index);
    Ok(pending)
}

#[derive(Debug)]
pub enum IndexError {
    Io(String),
    Parse(String),
    Other(String),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexError::Io(m) => write!(f, "io: {m}"),
            IndexError::Parse(m) => write!(f, "parse: {m}"),
            IndexError::Other(m) => write!(f, "other: {m}"),
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

        // Bump mtime on a.py to 10 seconds in the future — bypasses second-resolution issues.
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

    /// Same as `push_def` but records decorator/attribute names. `decorators`
    /// should be pre-formatted as `,name1,name2,` (leading/trailing commas)
    /// so the `unreferenced` query can match individual names via `LIKE ',name,'`.
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

    /// For calls where the receiver/namespace is a type (e.g. Rust `SessionStore::new()`),
    /// pass the type as `to_type` so `mmcg_callers <Type>` finds these sites.
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
