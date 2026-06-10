//! mmcg — Mastermind Codegraph.
//!
//! Multi-language code indexer and MCP server. Indexes functions, classes,
//! methods, structs, traits, constants, calls, and import edges into a local
//! SQLite database (`.mastermind/mmcg.db` by default). Exposes structural
//! queries over MCP for AI agents.
//!
//! Supported languages: Python, TypeScript/TSX, JavaScript/JSX, Rust, C#, Go,
//! Java, PHP, C/C++. C/C++ is best-effort syntactic — see README Limitations.

pub mod audit_spec;
pub mod context_doctor;
pub mod diff;
pub mod doctor;
pub mod executor_report;
pub mod fingerprint;
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
pub mod workflow_status;
