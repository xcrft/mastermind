//! mmcg — Mastermind Codegraph.
//!
//! Multi-language code indexer and MCP server. Indexes functions, classes,
//! methods, structs, traits, constants, calls, and import edges into a local
//! SQLite database (`.mastermind/mmcg.db` by default). Exposes structural
//! queries over MCP for AI agents.
//!
//! Supported languages: Python, TypeScript/TSX, JavaScript/JSX, Rust, C#, Go,
//! Java, PHP, C/C++. C/C++ is best-effort syntactic — see README Limitations.
//! Repository-owned architecture policies consume the same bounded graph.

pub mod audit_bundle;
pub mod audit_spec;
pub mod context_doctor;
pub mod diff;
pub mod doctor;
pub mod evidence;
pub mod executor_report;
pub mod fingerprint;
pub mod hex;
pub mod indexer;
pub mod lens;
pub mod lessons;
pub mod mcp;
pub mod miner;
pub mod policy;
pub mod queries;
pub mod run_task;
pub mod sarif_export;
pub mod scip_overlay;
pub mod setup;
pub mod spec;
pub mod store;
pub mod verify_spec;
pub mod watcher;
pub mod workflow_status;
