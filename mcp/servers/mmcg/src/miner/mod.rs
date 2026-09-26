//! Miners — read-only analyzers that derive **user-global** signal from a
//! repository's history. Distinct from the rest of the crate, which is
//! project-scoped: `indexer` builds the per-project structural graph;
//! `lessons` / `workflow_status` track per-project workflow state. A miner's
//! output is about the *person*, not the project, and lives under
//! `~/.mastermind/`.
//!
//! - [`feedback`] — preferences the author stated to coding agents, quoted
//!   verbatim from Claude Code or supported Codex transcripts, or imported
//!   from memory.
//! - [`profile`] — mines an author's code-shape idioms ("write like me") from
//!   their git-authored diffs into `~/.mastermind/style.md`, which the planner
//!   reads when drafting `CHANGE TO` blocks.
//! - [`store`] — the user-global SQLite store (`~/.mastermind/style.db`) that
//!   accumulates each repo's counts so the profile enriches across repos.
//! - [`tooling`] — formatter and linter scopes, so conventions a repository's
//!   tooling decides are not credited to the author.
//! - [`range`] — which languages, areas and libraries the author's commits
//!   touch, and how recently.
//! - [`workflow`] — how the author delivers changes (pull requests, tests and
//!   docs alongside code, tracker keys, change size): the process side of the
//!   profile.

pub mod access;
mod codex_transcript;
pub mod collection;
pub mod curation;
pub mod feedback;
pub mod habit;
pub mod hooks;
pub mod profile;
mod range;
mod stats;
pub mod store;
mod tooling;
mod workflow;
