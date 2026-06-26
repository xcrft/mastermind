//! Miners — read-only analyzers that derive **user-global** signal from a
//! repository's history. Distinct from the rest of the crate, which is
//! project-scoped: `indexer` builds the per-project structural graph;
//! `lessons` / `workflow_status` track per-project workflow state. A miner's
//! output is about the *person*, not the project, and lives under
//! `~/.mastermind/`.
//!
//! - [`profile`] — mines an author's code-shape idioms ("write like me") from
//!   their git-authored diffs into `~/.mastermind/style.md`, which the planner
//!   reads when drafting `CHANGE TO` blocks.
//! - [`store`] — the user-global SQLite store (`~/.mastermind/style.db`) that
//!   accumulates each repo's counts so the profile enriches across repos.

pub mod profile;
pub mod store;
