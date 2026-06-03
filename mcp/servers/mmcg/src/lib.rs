//! mmcg — Mastermind Codegraph.
//!
//! A multi-language code indexer plus MCP server. Indexes functions, classes,
//! methods, structs, traits, constants, calls, and import edges into a local
//! SQLite database (`.mastermind/mmcg.db` by default). Exposes structural
//! queries over MCP for AI agents.
//!
//! Supported languages: Python, TypeScript/TSX, JavaScript/JSX, Rust, C#, Go,
//! Java, PHP, C/C++. C/C++ is best-effort syntactic — see README Limitations.
//!
//! Modules:
//! - `store`   — SQLite schema (incl. FTS5 task-spec corpus) + per-file batched writes
//! - `indexer` — tree-sitter parsers (one extractor per language under `indexer/`)
//! - `queries` — high-level query API with serializable response types
//! - `diff`    — git-ref-based symbol diff (powers `mmcg_symbols_changed_since`)
//! - `mcp`     — stdio JSON-RPC MCP server. The authoritative tool list lives
//!   in `mcp::tools_list()`; READMEs are kept in sync against that.
//! - `watcher` — notify-based filesystem watcher for incremental re-indexing

pub mod audit_spec;
pub mod diff;
pub mod doctor;
pub mod indexer;
pub mod lessons;
pub mod mcp;
pub mod queries;
pub mod run_task;
pub mod setup;
pub mod spec;
pub mod store;
pub mod verify_spec;
pub mod watcher;
