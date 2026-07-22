//! CLI entry point for mastermind (the `mmcg` codegraph binary).
//!
//! Common subcommands:
//!   mastermind init           — scaffold a project, build the index, draft CONTEXT.md
//!   mastermind setup claude   — register the codegraph with Claude Code (MCP)
//!   mastermind index [PATH]   — build or refresh the index
//!   mastermind serve          — run as MCP stdio server
//!   mastermind doctor         — health-check the setup
//!   mastermind query <kind>   — one-shot query from the CLI (handy for debugging)

mod commands;
mod templates;

use clap::{Parser, Subcommand, ValueEnum};
use mmcg::store::Store;
use std::path::PathBuf;
use std::process::ExitCode;

/// Stack classifications used to inform drafting and diagnostics. CONTEXT.md
/// itself stays stack-agnostic so derivable commands and layouts do not become
/// duplicated project memory.
#[derive(Copy, Clone, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum Profile {
    /// Generic CONTEXT.md — no stack-specific seeding.
    Generic,
    /// TypeScript HTTP/REST/GraphQL API service.
    TypescriptApi,
    /// React Native mobile app (Expo or bare).
    ReactNative,
    /// Python FastAPI async API service.
    PythonFastapi,
    /// Rust project (CLI, service, or library).
    Rust,
    /// Polyglot monorepo — no single root stack; manifests live per-package.
    Monorepo,
}

/// Which parts of a Mastermind setup `uninstall` should remove.
#[derive(Copy, Clone, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum MapFormat {
    Text,
    Json,
    Mermaid,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum ImpactFormat {
    Text,
    Json,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum UninstallScope {
    /// This project only: remove `.mastermind/` and the project `.mcp.json` mmcg entry.
    Project,
    /// Global only: de-register mmcg from Claude Code user scope via `claude mcp remove` (leaves `.mastermind/` alone).
    Global,
    /// Both project and global.
    All,
}

#[derive(Parser)]
#[command(
    name = "mastermind",
    version,
    about = "Mastermind — codegraph-backed workflow CLI for AI coding agents (Python, TS/JS, Rust, C#, Go, Java, PHP, C/C++).",
    long_about = "Mastermind indexes your codebase into a queryable graph and serves it to Claude Code \
over MCP, plus runs spec-driven workflow gates (verify-spec / audit-spec).\n\n\
Onboard a project (run inside your repo):\n  \
mastermind init                       scaffold .mastermind/, build the index, draft CONTEXT.md\n  \
mastermind setup claude --write-mcp   register the codegraph with Claude Code (run once)\n  \
mastermind doctor                     verify the setup\n\n\
Installed via npm? `mastermind install` does the global setup (workflow agents + skills + MCP) in one step — then `mastermind init` per repo.\n\n\
Then open the project in Claude Code — the codegraph tools (search, callers, impact, …) are available. \
Remove it all with `mastermind uninstall`. (`mmcg` is an alias for `mastermind` — same binary.)"
)]
struct Cli {
    /// Path to the SQLite index file. Defaults to .mastermind/mmcg.db (relative to cwd).
    #[arg(long, env = "MMCG_INDEX_PATH", global = true)]
    index: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Build or refresh the index. Incremental by default — skips files whose
    /// mtime matches the stored index. Use --force to re-parse everything.
    Index {
        /// Project root to index. Defaults to cwd.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Re-parse every file regardless of mtime. Use after schema changes or to recover from a stale index.
        #[arg(long)]
        force: bool,
    },
    /// Build a compact deterministic architecture briefing from the codegraph.
    Map {
        /// Repository-relative file or directory scope. Defaults to the index root.
        #[arg(default_value = ".")]
        path: String,
        #[arg(long, value_enum, default_value = "text")]
        format: MapFormat,
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=6))]
        depth: u8,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
        top: u32,
        /// Exclude tests, fixtures, examples, generated code, and vendored dependencies.
        #[arg(long)]
        production_only: bool,
    },
    /// Analyze changed symbols, affected callers, component crossings, and candidate tests.
    Impact {
        #[arg(long)]
        since: String,
        #[arg(long, value_enum, default_value_t = ImpactFormat::Text)]
        format: ImpactFormat,
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=5))]
        depth: u32,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=500))]
        top: u32,
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Run as an MCP stdio server. Reads JSON-RPC from stdin, writes to stdout.
    Serve,
    /// Watch a directory and re-index files as they change. Long-running.
    Watch {
        /// Project root to watch. Defaults to cwd.
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Show workflow status: index freshness, installed subagents/skills,
    /// active tasks and their phase, and the recommended next step.
    Status {
        /// Project root. Defaults to cwd.
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Search durable project decisions, reports, audits, lessons, and context.
    /// Results are observed retrieval evidence; Markdown remains authoritative.
    History {
        /// FTS5 MATCH query.
        query: String,
        /// Limit results to one artifact kind.
        #[arg(long, value_parser = ["context", "lesson", "task_spec", "executor_report", "audit", "release_notes"])]
        kind: Option<String>,
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=50))]
        top: u32,
    },
    /// Build a grounded evidence packet for "why" questions without inventing
    /// rationale that is absent from durable project history.
    Why {
        query: String,
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=50))]
        top: u32,
    },
    /// Print only the next recommended action and the ready-to-paste Claude
    /// prompt for the highest-priority pending task.
    Next {
        /// Project root. Defaults to cwd.
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Print a full resume packet for the highest-priority pending task (or a
    /// named task): current phase, state, goal excerpt, file list, and a
    /// ready-to-paste Claude prompt. Use this when re-opening a session.
    Resume {
        /// Project root. Defaults to cwd.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Task folder name to resume (e.g. `042-payment-routing`). If omitted,
        /// picks the highest-priority pending task automatically.
        #[arg(long)]
        task: Option<String>,
    },
    /// Scaffold a project for the Mastermind workflow: create .mastermind/tasks/,
    /// CONTEXT.md (only if missing) and the index, then build the index and
    /// (unless --no-claude) populate CONTEXT.md from the codebase via `claude -p`.
    /// Auto-detects the stack as a drafting hint and always drops the workflow CLAUDE.md
    /// (an existing CLAUDE.md is left alone unless --force).
    Init {
        /// Project root. Defaults to cwd.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Overwrite existing files (CONTEXT.md, CLAUDE.md). Off by default.
        #[arg(long)]
        force: bool,
        /// Skip the automatic index build (otherwise `init` runs `index .` for you).
        #[arg(long)]
        no_index: bool,
        /// Skip auto-populating CONTEXT.md via `claude -p` (e.g. offline, in CI, or
        /// when the Claude Code CLI isn't installed). The bare template is left in place.
        #[arg(long)]
        no_claude: bool,
        /// Skip reconciling the npm workflow bundle into ~/.claude/. npm installs do this by
        /// default so the full workflow (not just the codegraph) is available. Only artifacts
        /// recorded in Mastermind's ownership manifest are retired on later updates.
        #[arg(long)]
        no_global: bool,
        /// Skip enriching `~/.mastermind/style.md` with this repository's authored history.
        /// Enabled by default and idempotent per repository; manual and interpreted sections
        /// are preserved.
        #[arg(long)]
        no_seed_style: bool,
    },
    /// Remove a Mastermind setup. By default (`--scope project`) deletes
    /// `.mastermind/` (index, tasks, run-state) and de-registers the `mmcg`
    /// entry from the project `.mcp.json`. `--scope global` de-registers mmcg
    /// from Claude Code user scope via `claude mcp remove`; `--scope all` does
    /// both. Never touches CONTEXT.md / CLAUDE.md. Safe by default: prints the
    /// plan and exits unless `--force` is passed.
    Uninstall {
        /// Project root. Defaults to cwd. (Ignored for `--scope global`.)
        #[arg(default_value = ".")]
        root: PathBuf,
        /// What to remove: `project` (.mastermind/ + project .mcp.json),
        /// `global` (de-register from Claude Code user scope via `claude mcp remove`), or `all`.
        #[arg(long, value_enum, default_value = "project")]
        scope: UninstallScope,
        /// Actually delete. Without this, prints what would be removed and exits.
        #[arg(long)]
        force: bool,
    },
    /// Dry-run-first MCP configuration for supported clients.
    #[command(subcommand)]
    Setup(SetupCmd),
    /// Pre-execution gate: mechanical checks on a spec file before handing
    /// off to the executor. Verifies mandatory sections non-empty, claimed
    /// symbols exist in the index, claimed files exist on disk, pre-edit
    /// snapshot caller counts match live index, blast radius isn't surprising.
    /// Exit code 0 if no errors (warnings OK).
    VerifySpec {
        /// Path to a `.mastermind/tasks/<NNN>-<name>/spec.md` file.
        spec: PathBuf,
        /// Project root the spec's file paths are relative to. Defaults to cwd.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        json: bool,
        /// Fail if no index is present, instead of skipping the live symbol checks.
        #[arg(long)]
        require_index: bool,
        /// Contract-driven mode: also require YAML frontmatter scoping the change
        /// (`touches` with files) and at least one `verify[].cmd`. Implies --require-index.
        #[arg(long)]
        strict: bool,
    },
    /// Post-execution gate: mechanical audit comparing spec contract against
    /// the actual repo state. Diffs `<git-ref>` (typically `main` or
    /// merge-base) against the working tree — uncommitted and untracked work
    /// counts, since the audit runs before the commit step: claimed files vs
    /// what actually differs, pre-edit snapshot vs live `mmcg_callers` counts,
    /// snapshot symbols still exist. Exit code 0 unless verdict is `broken`.
    AuditSpec {
        /// Path to the spec.
        spec: PathBuf,
        /// Git ref to compare against (e.g. `main`, `HEAD~3`).
        #[arg(long)]
        since: String,
        /// Project root. Defaults to cwd.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        json: bool,
        /// Path to a structured executor report (bare YAML or markdown with
        /// `<!-- mastermind:report-begin -->` sentinel; legacy executor
        /// sentinels remain accepted). When provided,
        /// integration-claim verification and vacuous-test detection run on
        /// top of the standard Phase A checks.
        #[arg(long)]
        executor_report: Option<PathBuf>,
        /// Write a portable audit bundle JSON to this path. Contains verdict,
        /// files_diff, discrepancies, snapshot_drift, executor_report_path.
        #[arg(long)]
        bundle: Option<PathBuf>,
    },
    /// Verify or sign sealed schema-v3 audit envelopes.
    #[command(subcommand)]
    Audit(AuditCmd),
    /// Health-check the environment for adoption — index existence,
    /// freshness, gitignore, CLAUDE.md workflow markers, MCP config,
    /// `mastermind serve` handshake. Exit code 0 if no checks fail.
    Doctor {
        /// Project root. Defaults to cwd.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Output JSON instead of human-readable text.
        #[arg(long)]
        json: bool,
        /// Show full context: binary path, index path, MCP config candidates,
        /// Claude config path, and hints for every check (not just failing ones).
        #[arg(long)]
        explain: bool,
    },
    /// Two-phase orchestrator that wraps the mastermind workflow in mechanical
    /// gates. Canonical tasks resume from their task-local `state.json`.
    ///
    ///   Pre-flight  — `verify-spec` + risk report + state write + hand-off.
    ///   Post-flight — `audit-spec` vs the recorded baseline ref; on Held,
    ///                 emits release notes to stdout + `.mastermind/releases/`.
    ///
    /// Defaults to hand-off semantics — print "now invoke the executor and
    /// re-run". Pass `--exec` to shell out to `claude -p` between phases.
    RunTask {
        /// Path to the spec file (typically under `.mastermind/tasks/`).
        spec: PathBuf,
        /// Project root the spec's file paths resolve against. Defaults to cwd.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Drop existing state and force pre-flight even if state exists.
        #[arg(long)]
        reset: bool,
        /// Run only pre-flight (verify + risk report + state write); never auto-resume.
        #[arg(long)]
        pre_only: bool,
        /// Run only post-flight (errors if no state file).
        #[arg(long)]
        post_only: bool,
        /// Shell out to `claude -p` synchronously between phases. Default: hand-off only.
        #[arg(long)]
        exec: bool,
        /// Skip the "index must exist and be non-empty" pre-check. Use for
        /// docs-only / spec-only specs that don't touch indexed source.
        /// Default = hard-fail when no index, because mmcg gates are only as
        /// strong as the codegraph they reason from.
        #[arg(long)]
        allow_no_index: bool,
        /// Contract-driven mode: fold strict spec checks into pre-flight
        /// (frontmatter scoping, file-scoped touches, a runnable verify command).
        #[arg(long)]
        strict: bool,
        /// Maximum number of pre-flight iterations on the same spec before
        /// the dispatcher refuses to continue. Default 3. Set to 0 to disable
        /// (not recommended). The counter survives `--reset` so the budget
        /// can't be trivially bypassed.
        #[arg(long, default_value_t = 3)]
        max_iterations: u32,
        /// Bypass the iteration-budget check for this one invocation. Still
        /// appends a `kind: iteration_budget_exhausted` lesson so the override
        /// is visible to future planners.
        #[arg(long)]
        force_iteration: bool,
    },
    /// One-shot query — handy for CLI debugging without going through MCP.
    #[command(subcommand)]
    Query(QueryCmd),
    /// Self-contained demo: builds a temp repo, indexes it, runs mmcg queries,
    /// and prints the mechanical auditor verdict. Zero setup — no Claude API key needed.
    Demo {
        /// Which demo scenario to run.
        /// Available: hallucinated-symbol, scope-creep, stale-find-block, vacuous-test, signature-drift
        #[arg(default_value = "hallucinated-symbol")]
        scenario: String,
    },
    /// Print a guided walkthrough of the Mastermind workflow: index → demo → setup → spec → run → track.
    Tour,
    /// Generate a PR comment in GitHub Flavored Markdown from a bundle JSON
    /// produced by `audit-spec --bundle`. Writes to stdout — pipe or redirect
    /// to a file and post via `gh pr comment --body-file <file>`.
    PrComment {
        /// Path to the sealed schema-v3 bundle JSON.
        bundle: PathBuf,
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        expected_repository: String,
        #[arg(long)]
        expected_baseline: String,
        #[arg(long)]
        expected_head: String,
    },
    /// Render an integrity-valid envelope without repository or signer trust.
    /// The output is explicitly marked untrusted and is forbidden in publication.
    PrCommentUntrusted { bundle: PathBuf },
    /// CI gate: index, verify selected specs, audit executor evidence, and
    /// optionally write bundles. Exit 0 if all pass.
    Ci {
        /// Git ref to diff against (required for audit phase).
        #[arg(long, default_value = "origin/main")]
        since: String,
        /// Project root. Defaults to cwd.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Write audit bundle JSONs to this directory (one per spec).
        #[arg(long)]
        bundle_dir: Option<PathBuf>,
        /// Audit only task folders whose spec or executor report changed
        /// between `since` and HEAD. Intended for pull-request CI.
        #[arg(long)]
        changed_only: bool,
        /// Fail when a selected task has no canonical executor-report.md.
        /// Bundle publication implies this requirement even when omitted.
        #[arg(long)]
        require_executor_report: bool,
    },
    /// Scaffold a new task spec under `.mastermind/tasks/`. Picks the next
    /// available NNN sequence number automatically.
    NewSpec {
        /// Short description of the task. Used as the spec title and folder slug.
        description: String,
        /// Workflow contract: `verified` (compact goal/scope/acceptance/tests)
        /// or `strict` (adds risk, evidence, rollback, and critic review).
        /// Legacy `lite` and `standard` templates remain accepted.
        #[arg(long, default_value = "verified")]
        mode: String,
        /// Project root. Defaults to cwd.
        #[arg(long, default_value = ".")]
        root: std::path::PathBuf,
    },
    /// Subcommands for inspecting project context quality.
    #[command(subcommand)]
    Context(ContextCmd),
    /// Mine user-global signal from a repository's history (e.g. an author's
    /// code-shape style). Output lives under `~/.mastermind/`, not the project.
    #[command(subcommand)]
    Miner(MinerCmd),
}

#[derive(Subcommand)]
enum ContextCmd {
    /// Audit project memory: CONTEXT placeholders and decision schema,
    /// completed-task history reviews, and lesson lifecycle quality.
    Doctor {
        /// Project root. Defaults to cwd.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Output JSON instead of human-readable text.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum AuditCmd {
    /// Verify content integrity and one or more independent trust anchors.
    Verify {
        bundle: PathBuf,
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        expected_repository: Option<String>,
        #[arg(long)]
        expected_baseline: Option<String>,
        #[arg(long)]
        expected_head: Option<String>,
        #[arg(long, requires = "public_key")]
        signature: Option<PathBuf>,
        #[arg(long, requires = "signature")]
        public_key: Option<PathBuf>,
        #[arg(long)]
        require_signature: bool,
        #[arg(long = "trusted-key-id")]
        trusted_key_ids: Vec<String>,
        #[arg(long = "revoked-key-id")]
        revoked_key_ids: Vec<String>,
        #[arg(long, conflicts_with_all = ["expected_repository", "expected_baseline", "expected_head", "root", "signature", "public_key", "require_signature", "trusted_key_ids", "revoked_key_ids"])]
        integrity_only: bool,
        #[arg(long)]
        json: bool,
    },
    /// Write a domain-separated Ed25519 detached signature.
    Sign {
        bundle: PathBuf,
        #[arg(long)]
        private_key: PathBuf,
        #[arg(long)]
        signature: PathBuf,
    },
    #[command(hide = true)]
    PrepareOutput {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum MinerCmd {
    /// Mine an author's style ("write like me") from their git history into
    /// `~/.mastermind/style.md`: advisory corpus observations plus commit
    /// conventions. Deterministic by default. Each run enriches a user-global
    /// cross-repo store; manual and interpreted sections survive normal re-mines.
    Profile {
        /// Repository to mine. Defaults to cwd.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Author to profile — substring of name or email. Defaults to
        /// `git config user.name` (matches all the person's emails).
        #[arg(long)]
        author: Option<String>,
        /// Replace the profile from scratch: drop every repository contribution
        /// and preserved manual/interpreted sections, then mine this repository.
        /// Without it, mining enriches safely.
        #[arg(long)]
        force: bool,
        /// Compatibility path for a deep LLM analysis via `claude -p`. Sends bounded sampled
        /// added lines + commit messages and replaces the preserved interpreted section.
        /// Prefer the `mastermind-style-deep` skill when qualitative accuracy matters.
        #[arg(long)]
        deep: bool,
    },
}

#[derive(Subcommand)]
enum QueryCmd {
    /// Find symbols by exact name.
    Search {
        name: String,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        language: Option<String>,
        /// Disable collapsing of partial-class declarations into one hit.
        /// By default mmcg joins C# `partial class Foo` rows across files
        /// into a single result with a `locations` list.
        #[arg(long)]
        no_collapse_partials: bool,
    },
    /// List callers of a symbol (matches name OR type prefix).
    Callers {
        name: String,
        #[arg(long)]
        language: Option<String>,
        /// Edge kind filter — 'calls' (default), 'imports', or 'inherits'.
        #[arg(long)]
        edge_kind: Option<String>,
    },
    /// List callees of a symbol.
    Callees {
        name: String,
        #[arg(long)]
        language: Option<String>,
        /// Edge kind filter — 'calls' (default), 'imports', or 'inherits'.
        #[arg(long)]
        edge_kind: Option<String>,
    },
    /// Transitive callers (blast radius).
    Impact {
        name: String,
        #[arg(long, default_value_t = 2)]
        depth: u32,
        #[arg(long)]
        language: Option<String>,
    },
    /// List indexed files.
    Files {
        #[arg(long)]
        prefix: Option<String>,
        #[arg(long)]
        language: Option<String>,
    },
    /// List all symbols defined in a file, in source order.
    SymbolsInFile { file: String },
    /// Show the symbol tree of a file (classes own their methods, etc.).
    Outline { file: String },
    /// Files re-indexed within a recent window. `--since 2h` / `30m` / `1d`.
    Recent {
        #[arg(long)]
        since: String,
    },
    /// Symbols nothing references (dead-code candidates).
    Unreferenced {
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        language: Option<String>,
    },
    /// Symbols under `prefix` referenced from OUTSIDE `prefix`.
    ApiSurface {
        prefix: String,
        #[arg(long)]
        language: Option<String>,
    },
    /// Detect circular imports — strongly-connected components of size ≥ min_size
    /// in the file-level import graph. Resolves edges by leaf-name match
    /// (over-approximating: may surface cycles between two unrelated symbols
    /// sharing a name — manually verify before refactoring).
    DependencyCycles {
        #[arg(long)]
        language: Option<String>,
        /// Smallest SCC to report. Default 2 = any cycle. 3 hides trivial A↔B.
        #[arg(long, default_value_t = 2)]
        min_size: u32,
    },
    /// Rank symbols by in-degree (distinct callers). Pre-flight "where is the
    /// gravity" — most-referenced functions, classes, methods in a path prefix
    /// or the whole index.
    Centrality {
        #[arg(long)]
        prefix: Option<String>,
        #[arg(long)]
        language: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        /// How many results to return.
        #[arg(long, default_value_t = 20)]
        top: u32,
    },
    /// Symbol-level diff between a git ref and the current index.
    /// Returns added / removed / signature-changed symbols across the files
    /// in `git diff --name-only <ref>..HEAD`. Uses `git show <ref>:<path>`
    /// to fetch old blobs and re-parses them with the same extractor.
    SymbolsChangedSince {
        /// Git ref to diff against (tag, branch, commit, HEAD~3, etc.)
        git_ref: String,
        /// Project root — defaults to cwd. Symbol paths are relative to this.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Full-text search past task specs in `.mastermind/tasks/`.
    /// FTS5 MATCH syntax — bare words AND-joined, phrases in double quotes,
    /// OR / NOT supported. Returns paths, titles, and snippet excerpts.
    Tasks {
        /// FTS5 MATCH query.
        query: String,
        #[arg(long, default_value_t = 10)]
        top: u32,
    },
    /// Search all durable project-history artifacts, not only task specs.
    History {
        query: String,
        #[arg(long, value_parser = ["context", "lesson", "task_spec", "executor_report", "audit", "release_notes"])]
        kind: Option<String>,
        #[arg(long, default_value_t = 10)]
        top: u32,
    },
    /// List files whose top-level imports reference the given name or path.
    ImportedBy {
        /// Name or fully-qualified path to look up
        query: String,
        /// How to match — by leaf binding name (default) or by fully-qualified path
        #[arg(long, default_value = "name")]
        match_kind: String,
        #[arg(long)]
        language: Option<String>,
    },
    /// Debug a symbol query: show matched symbol IDs, files, edge counts,
    /// source-language edge precision, and known limitations. Useful when
    /// mmcg returns unexpected results or you want to understand the trust
    /// level of a callers/callees result.
    Explain {
        name: String,
        #[arg(long)]
        language: Option<String>,
    },
}

#[derive(Subcommand)]
enum SetupCmd {
    /// Configure Claude Code.
    Claude {
        #[command(flatten)]
        args: SetupArgs,
    },
    /// Configure Cursor.
    Cursor {
        #[command(flatten)]
        args: SetupArgs,
    },
    /// Configure Codex.
    Codex {
        #[command(flatten)]
        args: SetupArgs,
    },
    /// Configure Continue.
    Continue {
        #[command(flatten)]
        args: SetupArgs,
    },
    /// Configure an explicit generic MCP JSON file.
    Generic {
        #[command(flatten)]
        args: SetupArgs,
    },
}

#[derive(clap::Args, Debug)]
struct SetupArgs {
    #[arg(long, value_enum, default_value_t = SetupScope::User)]
    scope: SetupScope,
    #[arg(long, default_value = ".")]
    root: PathBuf,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long, alias = "write-mcp")]
    write: bool,
    #[arg(long, hide = true)]
    dry_run: bool,
    #[arg(long)]
    remove: bool,
    #[arg(long)]
    force: bool,
    #[arg(long, hide = true)]
    project: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum SetupScope {
    Project,
    User,
}

impl std::fmt::Display for SetupScope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Project => "project",
            Self::User => "user",
        })
    }
}

impl From<SetupScope> for mmcg::setup::Scope {
    fn from(scope: SetupScope) -> Self {
        match scope {
            SetupScope::Project => Self::Project,
            SetupScope::User => Self::User,
        }
    }
}

fn normalize_setup_args(
    client: mmcg::setup::Client,
    mut args: SetupArgs,
) -> Result<SetupArgs, String> {
    if args.dry_run && args.write {
        return Err("legacy --dry-run conflicts with --write".into());
    }
    if let Some(project) = args.project.take() {
        if client != mmcg::setup::Client::Claude
            || args.scope != SetupScope::User
            || args.root.as_path() != std::path::Path::new(".")
            || args.config.is_some()
        {
            return Err("legacy --project conflicts with client, scope, root, or config".into());
        }
        args.scope = SetupScope::Project;
        args.root = project;
    }
    Ok(args)
}

fn default_index_path() -> PathBuf {
    PathBuf::from(".mastermind/mmcg.db")
}

/// Parse argv with the program name pinned to `mastermind` so `--help` and
/// usage strings read consistently — the npm wrapper spawns the native binary
/// directly, so argv[0] would otherwise surface the internal `mmcg` name.
fn parse_cli() -> Cli {
    use clap::{CommandFactory, FromArgMatches};
    let matches = Cli::command().bin_name("mastermind").get_matches();
    Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit())
}

fn run_cli_inner(
    cli: Cli,
    impact_engine: &mmcg::queries::ImpactEngine<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let index_path = cli.index.unwrap_or_else(default_index_path);

    match cli.cmd {
        Cmd::Index { root, force } => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            let mut store = Store::open(&index_path)?;
            let indexer = mmcg::indexer::Indexer::new(&root);
            let stats = indexer.index_all(&mut store, force)?;
            println!(
                "indexed {} (unchanged {}, purged {}, skipped binary {}, skipped large {}, failed {}) / scanned {} | {} symbols | {} edges | {} task specs | {} history entries (skipped {}, truncated {}) | {} ms",
                stats.files_indexed,
                stats.files_unchanged,
                stats.files_purged,
                stats.files_skipped_binary,
                stats.files_skipped_too_large,
                stats.files_failed,
                stats.files_scanned,
                stats.symbols_total,
                stats.edges_total,
                stats.task_specs_indexed,
                stats.history_entries_indexed,
                stats.history_entries_skipped,
                stats.history_entries_truncated,
                stats.duration_ms
            );
            if stats.files_failed > 0 {
                eprintln!("warning: {} files failed to parse", stats.files_failed);
            }
            if stats.extractor_contract_rebuilt {
                eprintln!("extractor contract changed: rebuilt all structural data");
            }
        }
        Cmd::Map {
            path,
            format,
            depth,
            top,
            production_only,
        } => {
            commands::query::dispatch_map(&path, format, depth, top, production_only, &index_path)?
        }
        Cmd::Impact {
            since,
            format,
            depth,
            top,
            root,
        } => {
            let root = root
                .canonicalize()
                .map_err(|_| mmcg::queries::ChangeImpactError::RootMismatch)?;
            let store = Store::open(&index_path)
                .map_err(|_| mmcg::queries::ChangeImpactError::IndexStale)?;
            let top =
                usize::try_from(top).map_err(|_| mmcg::queries::ChangeImpactError::InvalidRef)?;
            let response = impact_engine(&store, &root, &since, depth, top)?;
            print!(
                "{}",
                commands::query::render_change_impact(&response, format)?
            );
        }
        Cmd::Serve => {
            let store = Store::open(&index_path)?;
            mmcg::mcp::serve(store)?;
        }
        Cmd::Watch { root } => {
            let store = Store::open(&index_path)?;
            mmcg::watcher::run(root, store)?;
        }
        Cmd::Status { root } => {
            let root = root
                .canonicalize()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or(root));
            let ws = mmcg::workflow_status::WorkflowStatus::scan(&root);
            print!("{}", ws.render_text());
        }
        Cmd::History { query, kind, top } => {
            commands::query::dispatch_history(&query, kind.as_deref(), top, &index_path)?;
        }
        Cmd::Why { query, top } => {
            commands::query::dispatch_why(&query, top, &index_path)?;
        }
        Cmd::Next { root } => {
            let root = root
                .canonicalize()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or(root));
            let ws = mmcg::workflow_status::WorkflowStatus::scan(&root);
            print!("{}", ws.render_next_text());
        }
        Cmd::Resume { root, task } => {
            let root = root
                .canonicalize()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or(root));
            let ws = mmcg::workflow_status::WorkflowStatus::scan(&root);
            print!("{}", ws.render_resume_text(task.as_deref()));
        }
        Cmd::Init {
            root,
            force,
            no_index,
            no_claude,
            no_global,
            no_seed_style,
        } => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            commands::do_init(
                &root,
                commands::init::InitOpts {
                    force,
                    index: !no_index,
                    claude: !no_claude,
                    global: !no_global,
                    seed_style: !no_seed_style,
                },
            )?;
        }
        Cmd::Uninstall { root, scope, force } => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            commands::do_uninstall(&root, scope, force)?;
        }
        Cmd::Setup(setup_cmd) => {
            let me = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
            let (client, args) = match setup_cmd {
                SetupCmd::Claude { args } => (mmcg::setup::Client::Claude, args),
                SetupCmd::Cursor { args } => (mmcg::setup::Client::Cursor, args),
                SetupCmd::Codex { args } => (mmcg::setup::Client::Codex, args),
                SetupCmd::Continue { args } => (mmcg::setup::Client::Continue, args),
                SetupCmd::Generic { args } => (mmcg::setup::Client::Generic, args),
            };
            let args = normalize_setup_args(client, args)?;
            let scope = mmcg::setup::Scope::from(args.scope);
            let invalid = (client == mmcg::setup::Client::Codex
                && scope == mmcg::setup::Scope::Project)
                || (client == mmcg::setup::Client::Generic && args.config.is_none())
                || (client != mmcg::setup::Client::Generic && args.config.is_some());
            let root = if invalid {
                args.root
            } else {
                args.root
                    .canonicalize()
                    .map_err(|e| format!("canonicalize {}: {e}", args.root.display()))?
            };
            let request = mmcg::setup::Request {
                client,
                scope,
                root,
                config: args.config,
                write: args.write,
                remove: args.remove,
                force: args.force,
            };
            let outcome = mmcg::setup::run(&request, &me);
            if matches!(
                outcome,
                mmcg::setup::Outcome::Error | mmcg::setup::Outcome::RefusedOverwrite
            ) {
                std::process::exit(1);
            }
        }
        Cmd::VerifySpec {
            spec,
            root,
            json,
            require_index,
            strict,
        } => {
            commands::verify_spec(&spec, root, json, require_index, strict, &index_path)?;
        }
        Cmd::AuditSpec {
            spec,
            since,
            root,
            json,
            executor_report,
            bundle,
        } => {
            commands::audit_spec(
                &spec,
                &since,
                root,
                json,
                &index_path,
                executor_report.as_deref(),
                bundle.as_deref(),
            )?;
        }
        Cmd::Audit(AuditCmd::Verify {
            bundle,
            root,
            expected_repository,
            expected_baseline,
            expected_head,
            signature,
            public_key,
            require_signature,
            trusted_key_ids,
            revoked_key_ids,
            integrity_only,
            json,
        }) => {
            commands::audit::validate_key_ids(&trusted_key_ids)?;
            commands::audit::validate_key_ids(&revoked_key_ids)?;
            commands::audit::verify(commands::audit::VerifyOptions {
                bundle,
                root,
                expected_repository,
                expected_baseline,
                expected_head,
                signature,
                public_key,
                require_signature,
                trusted_key_ids,
                revoked_key_ids,
                integrity_only,
                json,
            })?;
        }
        Cmd::Audit(AuditCmd::Sign {
            bundle,
            private_key,
            signature,
        }) => commands::audit::sign(&bundle, &private_key, &signature)?,
        Cmd::Audit(AuditCmd::PrepareOutput { root, path }) => {
            commands::audit::prepare_output(&root, &path)?;
        }
        Cmd::Doctor {
            root,
            json,
            explain,
        } => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            let me = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
            let report = mmcg::doctor::run(&root, &me);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else if explain {
                print!("{}", report.render_explain(&me, &index_path));
            } else {
                print!("{}", report.render_text());
            }
            if report.has_failures() {
                std::process::exit(1);
            }
        }
        Cmd::Demo { scenario } => {
            commands::demo(&scenario)?;
        }
        Cmd::Tour => {
            commands::tour();
        }
        Cmd::PrComment {
            bundle,
            root,
            expected_repository,
            expected_baseline,
            expected_head,
        } => {
            commands::pr_comment(
                &bundle,
                &root,
                &expected_repository,
                &expected_baseline,
                &expected_head,
            )?;
        }
        Cmd::PrCommentUntrusted { bundle } => {
            commands::pr_comment::run_untrusted(&bundle)?;
        }
        Cmd::Ci {
            since,
            root,
            bundle_dir,
            changed_only,
            require_executor_report,
        } => {
            let ok = commands::ci(
                commands::ci::CiOpts {
                    since,
                    root,
                    bundle_dir,
                    changed_only,
                    require_executor_report,
                },
                &index_path,
            )?;
            if !ok {
                std::process::exit(1);
            }
        }
        Cmd::NewSpec {
            description,
            mode,
            root,
        } => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            let mode = commands::new_spec::Mode::from_str(&mode)
                .map_err(Box::<dyn std::error::Error>::from)?;
            commands::new_spec(&description, mode, &root)?;
        }
        Cmd::RunTask {
            spec,
            root,
            reset,
            pre_only,
            post_only,
            exec,
            allow_no_index,
            strict,
            max_iterations,
            force_iteration,
        } => {
            let outcome = commands::run_task(
                &spec,
                root,
                &index_path,
                mmcg::run_task::RunOpts {
                    reset,
                    pre_only,
                    post_only,
                    exec,
                    allow_no_index,
                    strict,
                    max_iterations,
                    force_iteration,
                },
            )?;
            if matches!(
                outcome,
                mmcg::run_task::Outcome::PreFailed
                    | mmcg::run_task::Outcome::PostBroken
                    | mmcg::run_task::Outcome::ExecFailed
            ) {
                std::process::exit(1);
            }
        }
        Cmd::Query(q) => commands::dispatch_query(q, &index_path)?,
        Cmd::Context(ContextCmd::Doctor { root, json }) => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            let report = mmcg::context_doctor::run(&root);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", report.render_text());
            }
            if report.has_failures() {
                std::process::exit(1);
            }
        }
        Cmd::Miner(MinerCmd::Profile {
            root,
            author,
            force,
            deep,
        }) => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            mmcg::miner::profile::run(&root, author, force, deep)?;
        }
    }
    Ok(())
}

fn run_cli(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    run_cli_inner(cli, &mmcg::queries::change_impact)
}

#[cfg(test)]
fn run_cli_with_impact_engine(
    cli: Cli,
    impact_engine: &mmcg::queries::ImpactEngine<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    run_cli_inner(cli, impact_engine)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    run_cli(parse_cli())
}

fn cli_error_line(error: &(dyn std::error::Error + 'static)) -> String {
    if let Some(error) = error.downcast_ref::<mmcg::queries::ChangeImpactError>() {
        error.code().to_string()
    } else {
        format!("mmcg error: {error}")
    }
}

fn render_exit(
    result: Result<(), Box<dyn std::error::Error>>,
    stderr: &mut dyn std::io::Write,
) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(stderr, "{}", cli_error_line(e.as_ref()));
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    render_exit(run(), &mut std::io::stderr())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_setup_args(argv: &[&str]) -> SetupArgs {
        let cli = Cli::try_parse_from(argv).unwrap();
        let Cmd::Setup(SetupCmd::Claude { args }) = cli.cmd else {
            panic!("expected claude setup command");
        };
        args
    }

    #[test]
    fn legacy_claude_dry_run_flag_is_accepted_and_conflicts_with_write() {
        let args = claude_setup_args(&["mastermind", "setup", "claude", "--dry-run"]);
        assert!(normalize_setup_args(mmcg::setup::Client::Claude, args).is_ok());

        let args = claude_setup_args(&["mastermind", "setup", "claude", "--dry-run", "--write"]);
        assert_eq!(
            normalize_setup_args(mmcg::setup::Client::Claude, args).unwrap_err(),
            "legacy --dry-run conflicts with --write"
        );
    }

    #[test]
    fn impact_cli_dispatch_renders_all_engine_failures_as_code_only() {
        let root =
            std::env::temp_dir().join(format!("mmcg-cli-impact-errors-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let index = root.join("impact-errors.db");
        for (error, code) in [
            (mmcg::queries::ChangeImpactError::InvalidRef, "invalid_ref"),
            (
                mmcg::queries::ChangeImpactError::RootMismatch,
                "root_mismatch",
            ),
            (mmcg::queries::ChangeImpactError::IndexStale, "index_stale"),
            (
                mmcg::queries::ChangeImpactError::SnapshotChanged,
                "snapshot_changed",
            ),
            (mmcg::queries::ChangeImpactError::GitTimeout, "git_timeout"),
            (
                mmcg::queries::ChangeImpactError::GitOutputLimit,
                "git_output_limit",
            ),
        ] {
            let injected_detail = "injected-engine-detail-must-not-leak";
            let cli = Cli::try_parse_from([
                "mastermind",
                "--index",
                index.to_str().unwrap(),
                "impact",
                "--since",
                injected_detail,
                "--format",
                "json",
                "--root",
                root.to_str().unwrap(),
            ])
            .unwrap();
            let engine = |_: &Store, _: &std::path::Path, git_ref: &str, _: u32, _: usize| {
                assert_eq!(git_ref, injected_detail);
                Err(error)
            };
            let result = run_cli_with_impact_engine(cli, &engine);
            let mut stderr = Vec::new();
            assert_eq!(render_exit(result, &mut stderr), ExitCode::FAILURE);
            let transcript = String::from_utf8(stderr).unwrap();
            assert_eq!(transcript, format!("{code}\n"));
            assert!(!transcript.contains(injected_detail));
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn audit_verify_requires_both_signature_and_public_key() {
        assert!(Cli::try_parse_from([
            "mastermind",
            "audit",
            "verify",
            "bundle.json",
            "--signature",
            "bundle.sig.json",
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "mastermind",
            "audit",
            "verify",
            "bundle.json",
            "--public-key",
            "audit.pub",
        ])
        .is_err());
    }

    #[test]
    fn history_and_why_are_first_class_cli_commands() {
        let history = Cli::try_parse_from([
            "mastermind",
            "history",
            "webhook dedupe",
            "--kind",
            "audit",
            "--top",
            "5",
        ])
        .unwrap();
        assert!(matches!(
            history.cmd,
            Cmd::History {
                query,
                kind: Some(kind),
                top: 5
            } if query == "webhook dedupe" && kind == "audit"
        ));

        let why = Cli::try_parse_from(["mastermind", "why", "idempotency"]).unwrap();
        assert!(matches!(why.cmd, Cmd::Why { query, top: 10 } if query == "idempotency"));
    }
}
