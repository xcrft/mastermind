//! mmcg — Mastermind Codegraph.
//!
//! A Python code indexer plus MCP server. Indexes functions, classes, methods,
//! and call edges into a local SQLite database (.mastermind/mmcg.db by default).
//! Exposes structural queries over MCP for AI agents.
//!
//! Modules:
//! - `store` — SQLite schema + per-file batched writes
//! - `indexer` — tree-sitter Python parser, walks AST, populates the store (parallel)
//! - `queries` — high-level query API with serializable response types
//! - `mcp`   — stdio JSON-RPC MCP server exposing 6 tools

pub mod diff;
pub mod indexer;
pub mod mcp;
pub mod queries;
pub mod store;
pub mod watcher;
