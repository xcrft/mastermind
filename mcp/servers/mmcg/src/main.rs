//! CLI entry point for mastermind (the `mmcg` codegraph binary).
//!
//! Common subcommands:
//!   mastermind init           — scaffold a project, build the index, draft CONTEXT.md
//!   mastermind setup claude   — register the codegraph with Claude Code (MCP)
//!   mastermind index [PATH]   — build or refresh the index
//!   mastermind enrich         — add validated semantic or declarative evidence
//!   mastermind temporal       — compare architecture at a Git baseline
//!   mastermind ui --since REF — open the local diff-first Lens UI
//!   mastermind review export  — write an autonomous PR evidence package
//!   mastermind serve          — run as MCP stdio server
//!   mastermind doctor         — health-check the setup
//!   mastermind query <kind>   — one-shot query from the CLI (handy for debugging)

mod commands;
mod templates;

use clap::{Parser, Subcommand, ValueEnum};
use mmcg::store::Store;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::ExitCode;

static BUILD_VERSION_MARKER: &str = concat!("MMCG_BUILD_VERSION=[", env!("CARGO_PKG_VERSION"), "]");

/// Which parts of a Mastermind setup `uninstall` should remove.
#[derive(Copy, Clone, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum MapFormat {
    Text,
    Json,
    Mermaid,
    Sarif,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum ImpactFormat {
    Text,
    Json,
    Sarif,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum TemporalFormat {
    Text,
    Json,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum PolicyFormat {
    Text,
    Json,
    Sarif,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum FactAdapterFormat {
    Sarif,
    Coverage,
    Junit,
    Otel,
}

impl From<FactAdapterFormat> for mmcg::fact_adapter::AdapterFormat {
    fn from(value: FactAdapterFormat) -> Self {
        match value {
            FactAdapterFormat::Sarif => Self::Sarif,
            FactAdapterFormat::Coverage => Self::Coverage,
            FactAdapterFormat::Junit => Self::Junit,
            FactAdapterFormat::Otel => Self::Otel,
        }
    }
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
mastermind setup claude --write       register the codegraph with Claude Code (run once)\n  \
mastermind temporal --since main        review architecture drift over time\n  \
  mastermind ui --since main            inspect this change in the local read-only Lens UI\n  \
mastermind doctor                     verify the setup\n\n\
Installed via npm? `mastermind install` does the global setup (workflow agents + skills + MCP) in one step — then `mastermind init` per repo.\n\n\
Then open the project in Claude Code — the codegraph tools (search, callers, impact, …) are available. \
Remove it all with `mastermind uninstall`. (`mmcg` is an alias for `mastermind` — same binary.)"
)]
struct Cli {
    /// Path to the SQLite index file. Root-scoped commands default to
    /// <root>/.mastermind/mmcg.db; commands without a root default relative to cwd.
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
    /// Add a validated evidence overlay without replacing the Tree-sitter graph.
    Enrich {
        /// SCIP protobuf index produced by a language-specific SCIP indexer.
        #[arg(
            long,
            value_name = "PATH",
            required_unless_present = "facts",
            conflicts_with = "facts"
        )]
        scip: Option<PathBuf>,
        /// Strict mastermind-facts/v1 manifest produced by a declarative extension.
        #[arg(
            long,
            value_name = "PATH",
            required_unless_present = "scip",
            conflicts_with = "scip"
        )]
        facts: Option<PathBuf>,
        /// Detached mastermind fact-manifest signature.
        #[arg(long, value_name = "PATH", requires = "facts", conflicts_with = "scip")]
        signature: Option<PathBuf>,
        /// Ed25519 public key used to authenticate the fact manifest.
        #[arg(long, value_name = "PATH", requires = "facts", conflicts_with = "scip")]
        public_key: Option<PathBuf>,
        /// Reject unsigned fact manifests.
        #[arg(long, requires = "facts", conflicts_with = "scip")]
        require_signature: bool,
        /// Trusted Ed25519 key ID. Repeatable for rotation windows.
        #[arg(long = "trusted-key-id", requires = "facts", conflicts_with = "scip")]
        trusted_key_ids: Vec<String>,
        /// Revoked Ed25519 key ID. Revocation wins over trust.
        #[arg(long = "revoked-key-id", requires = "facts", conflicts_with = "scip")]
        revoked_key_ids: Vec<String>,
    },
    /// Adapt, sign, and verify revision-bound declarative fact manifests.
    #[command(subcommand)]
    Facts(FactCmd),
    /// Lock and inspect a bounded local graph across multiple repositories.
    #[command(subcommand)]
    Team(TeamCmd),
    /// Audit Mastermind agent, skill, model, tool, and writer wiring.
    #[command(subcommand)]
    Workflow(WorkflowCmd),
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
        /// Exclude non-production path segments and conventional test filenames.
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
    /// Compare bounded architecture snapshots between a Git baseline and the indexed worktree.
    Temporal {
        #[arg(long)]
        since: String,
        #[arg(long, value_enum, default_value_t = TemporalFormat::Text)]
        format: TemporalFormat,
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, default_value = ".")]
        path: String,
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=5))]
        depth: u8,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
        top: u32,
        #[arg(long)]
        production_only: bool,
        /// Override the repository CODEOWNERS file used for ownership drift.
        #[arg(long, value_name = "PATH")]
        codeowners: Option<PathBuf>,
    },
    /// Evaluate repository-owned architecture rules over bounded change evidence.
    #[command(subcommand)]
    Policy(PolicyCmd),
    /// Build portable review evidence from the same bounded Lens snapshot.
    #[command(subcommand)]
    Review(ReviewCmd),
    /// Serve Mastermind Lens: a local, read-only, diff-first change review UI.
    Ui {
        /// Git ref used as the change-impact baseline.
        #[arg(long)]
        since: String,
        /// Project root. Defaults to cwd.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Repository-relative map scope. Defaults to the repository root.
        #[arg(long, default_value = ".")]
        path: String,
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u8).range(1..=5))]
        depth: u8,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=100))]
        top: u32,
        /// Exclude conventional tests, fixtures, examples, generated, and vendor paths from the architecture map.
        #[arg(long)]
        production_only: bool,
        /// Import a SARIF 2.1 report as read-only Lens evidence. Repeatable.
        #[arg(long = "sarif", value_name = "PATH")]
        sarif: Vec<PathBuf>,
        /// Import an LCOV tracefile or Cobertura XML report. Repeatable.
        #[arg(long = "coverage", value_name = "PATH")]
        coverage: Vec<PathBuf>,
        /// Import a JUnit XML test report. Only explicit testcase file paths are correlated. Repeatable.
        #[arg(long = "junit", value_name = "PATH")]
        junit: Vec<PathBuf>,
        /// Import an OpenTelemetry OTLP JSON trace export. Repeatable.
        #[arg(long = "otel", value_name = "PATH")]
        otel: Vec<PathBuf>,
        /// Override the repository CODEOWNERS file used by Lens.
        #[arg(long, value_name = "PATH")]
        codeowners: Option<PathBuf>,
        /// Do not correlate exact changed-file mentions from indexed specs, ADRs, audits, and lessons.
        #[arg(long)]
        no_project_knowledge: bool,
        /// Bound read-only Git churn and contributor evidence. Zero disables it.
        #[arg(long, default_value_t = 200, value_parser = clap::value_parser!(u16).range(0..=1000))]
        git_commits: u16,
        /// Loopback port. Zero asks the OS for an available ephemeral port.
        #[arg(long, default_value_t = 0)]
        port: u16,
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
    /// Search durable project decisions/ADRs, reports, audits, lessons, and context.
    /// Results are observed retrieval evidence; Markdown remains authoritative.
    History {
        /// FTS5 MATCH query.
        query: String,
        /// Limit results to one artifact kind.
        #[arg(long, value_parser = ["context", "lesson", "task_spec", "executor_report", "audit", "release_notes", "architecture_decision"])]
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
        /// changed_files, discrepancies, snapshot_drift, executor_report_path.
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
enum WorkflowCmd {
    /// Build a deterministic read-only graph of the owned workflow layout.
    Audit {
        /// Source repository or installed client workflow root.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Output the versioned schema-v1 report as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum FactCmd {
    /// Convert one bounded third-party report into mastermind-facts/v1.
    Adapt {
        #[arg(long, value_enum)]
        format: FactAdapterFormat,
        #[arg(long, value_name = "PATH")]
        input: PathBuf,
        #[arg(long, value_name = "PATH")]
        output: PathBuf,
        #[arg(long)]
        producer: String,
        #[arg(long)]
        producer_version: String,
        #[arg(long)]
        dataset: String,
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Generate a local Ed25519 producer keypair without exposing the seed.
    Keygen {
        #[arg(long)]
        private_key: PathBuf,
        #[arg(long)]
        public_key: PathBuf,
    },
    /// Write a domain-separated Ed25519 detached fact-manifest signature.
    Sign {
        manifest: PathBuf,
        #[arg(long)]
        private_key: PathBuf,
        #[arg(long)]
        signature: PathBuf,
    },
    /// Verify a fact manifest against an explicit local trust policy.
    Verify {
        manifest: PathBuf,
        #[arg(long)]
        signature: PathBuf,
        #[arg(long)]
        public_key: PathBuf,
        #[arg(long = "trusted-key-id", required = true)]
        trusted_key_ids: Vec<String>,
        #[arg(long = "revoked-key-id")]
        revoked_key_ids: Vec<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TeamCmd {
    /// Resolve repository roots/indexes and pin identity, revision, and DB+WAL digest.
    Lock {
        manifest: PathBuf,
        #[arg(long, value_name = "PATH")]
        output: PathBuf,
    },
    /// Read a locked team manifest and emit its local, read-only graph.
    Map { manifest: PathBuf },
}

#[derive(Subcommand)]
enum PolicyCmd {
    /// Check architecture policy against a baseline and exit non-zero on violations or incomplete evidence.
    Check {
        /// Git ref used as the diff and cycle-comparison baseline.
        #[arg(long)]
        since: String,
        /// Project root. Defaults to cwd.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Repository-owned v1 policy file.
        #[arg(long, default_value = "mastermind-policy.yml")]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = PolicyFormat::Text)]
        format: PolicyFormat,
        /// Override the repository CODEOWNERS file.
        #[arg(long, value_name = "PATH")]
        codeowners: Option<PathBuf>,
        /// Directory containing canonical strict task evidence.
        #[arg(long, default_value = ".mastermind/tasks", value_name = "PATH")]
        workflow_evidence: PathBuf,
        /// Static caller depth used for blast-radius and boundary evidence.
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=5))]
        depth: u32,
        /// Maximum returned impacted symbols. Incomplete evidence fails closed.
        #[arg(long, default_value_t = 500, value_parser = clap::value_parser!(u32).range(1..=500))]
        top: u32,
    },
}

#[derive(Subcommand)]
enum ReviewCmd {
    /// Export one autonomous HTML/SARIF/Markdown package plus its revision manifest and CI workflow.
    Export {
        /// Git ref used as the change-impact baseline.
        #[arg(long)]
        since: String,
        /// New output directory. Export refuses to overwrite an existing path.
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        /// Project root. Defaults to cwd.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Repository-relative map scope. Defaults to the repository root.
        #[arg(long, default_value = ".")]
        path: String,
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u8).range(1..=5))]
        depth: u8,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=100))]
        top: u32,
        /// Exclude conventional tests, fixtures, examples, generated, and vendor paths from the architecture map.
        #[arg(long)]
        production_only: bool,
        /// Import a SARIF 2.1 report as read-only evidence. Repeatable.
        #[arg(long = "sarif", value_name = "PATH")]
        sarif: Vec<PathBuf>,
        /// Import an LCOV tracefile or Cobertura XML report. Repeatable.
        #[arg(long = "coverage", value_name = "PATH")]
        coverage: Vec<PathBuf>,
        /// Import a JUnit XML test report. Repeatable.
        #[arg(long = "junit", value_name = "PATH")]
        junit: Vec<PathBuf>,
        /// Import an OpenTelemetry OTLP JSON trace export. Repeatable.
        #[arg(long = "otel", value_name = "PATH")]
        otel: Vec<PathBuf>,
        /// Override the repository CODEOWNERS file used by Lens.
        #[arg(long, value_name = "PATH")]
        codeowners: Option<PathBuf>,
        /// Do not correlate exact changed-file mentions from indexed specs, ADRs, audits, and lessons.
        #[arg(long)]
        no_project_knowledge: bool,
        /// Bound read-only Git churn and contributor evidence. Zero disables it.
        #[arg(long, default_value_t = 200, value_parser = clap::value_parser!(u16).range(0..=1000))]
        git_commits: u16,
        /// Optional strict v1 producer manifest binding repository-relative evidence digests to the same head OID.
        #[arg(long, value_name = "PATH")]
        evidence_attestation: Option<PathBuf>,
    },
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
    /// in the file-level import graph. C/C++ includes resolve to indexed header
    /// paths; other languages use conservative leaf-name matching.
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
        #[arg(long, value_parser = ["context", "lesson", "task_spec", "executor_report", "audit", "release_notes", "architecture_decision"])]
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
    /// Inspect compiler-resolved SCIP definitions and references. An empty
    /// result with fallback_active=true means the Tree-sitter-only mode is active.
    Semantic {
        symbol: String,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=500))]
        top: u32,
    },
    /// Read normalized declarative facts. The response also exposes the exact
    /// repository identity, revision, API version, and supported capabilities
    /// that a producer must place in its manifest.
    Facts {
        #[arg(long, default_value = ".")]
        path: String,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=400))]
        top: u32,
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

fn index_path_for_root(explicit: Option<&std::path::Path>, root: &std::path::Path) -> PathBuf {
    explicit
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| root.join(".mastermind/mmcg.db"))
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
    let index_override = cli.index;
    let index_path = index_override.clone().unwrap_or_else(default_index_path);

    match cli.cmd {
        Cmd::Index { root, force } => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            let index_path = index_path_for_root(index_override.as_deref(), &root);
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
            for path in &stats.skipped_binary_paths {
                eprintln!("skipped binary source: {path:?}");
            }
            for path in &stats.skipped_too_large_paths {
                eprintln!("skipped oversized source: {path:?}");
            }
            for path in &stats.skipped_paths {
                eprintln!("skipped unsupported path: {path:?}");
            }
            if stats.files_failed > 0 {
                eprintln!("warning: {} files failed to parse", stats.files_failed);
            }
            if stats.extractor_contract_rebuilt {
                eprintln!("extractor contract changed: rebuilt all structural data");
            }
        }
        Cmd::Workflow(WorkflowCmd::Audit { root, json }) => {
            let report = mmcg::workflow_status::audit_workflow(&root);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", report.render_text());
            }
            if report.has_errors() {
                std::process::exit(1);
            }
        }
        Cmd::Enrich {
            scip,
            facts,
            signature,
            public_key,
            require_signature,
            trusted_key_ids,
            revoked_key_ids,
        } => {
            if !index_path.is_file() {
                return Err(format!(
                    "codegraph index {} does not exist; run `mastermind index .` first",
                    index_path.display()
                )
                .into());
            }
            let store = Store::open(&index_path)?;
            let summary = match (scip, facts) {
                (Some(scip), None) => {
                    serde_json::to_value(mmcg::scip_overlay::import(&store, &scip)?)?
                }
                (None, Some(facts)) => {
                    let policy = mmcg::fact_signature::FactTrustPolicy {
                        signature,
                        public_key,
                        require_signature,
                        trusted_key_ids: trusted_key_ids.into_iter().collect(),
                        revoked_key_ids: revoked_key_ids.into_iter().collect(),
                    };
                    serde_json::to_value(mmcg::facts::import_with_trust(&store, &facts, &policy)?)?
                }
                _ => unreachable!("clap requires exactly one enrichment input"),
            };
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
        Cmd::Facts(command) => match command {
            FactCmd::Adapt {
                format,
                input,
                output,
                producer,
                producer_version,
                dataset,
                root,
            } => {
                let selected_index = index_path_for_root(index_override.as_deref(), &root);
                if !selected_index.is_file() {
                    return Err(format!(
                        "codegraph index {} does not exist; run `mastermind index .` first",
                        selected_index.display()
                    )
                    .into());
                }
                let store = Store::open_read_only(&selected_index)?;
                let summary = mmcg::fact_adapter::adapt(
                    &store,
                    &mmcg::fact_adapter::AdaptOptions {
                        format: format.into(),
                        input: &input,
                        output: &output,
                        producer: &producer,
                        producer_version: &producer_version,
                        dataset: &dataset,
                        root: &root,
                    },
                )?;
                println!("{}", serde_json::to_string_pretty(&summary)?);
            }
            FactCmd::Keygen {
                private_key,
                public_key,
            } => {
                let summary = mmcg::fact_signature::generate_keypair(&private_key, &public_key)?;
                println!("{}", serde_json::to_string_pretty(&summary)?);
            }
            FactCmd::Sign {
                manifest,
                private_key,
                signature,
            } => {
                let signed = mmcg::fact_signature::sign(&manifest, &private_key, &signature)?;
                println!("{}", serde_json::to_string_pretty(&signed)?);
            }
            FactCmd::Verify {
                manifest,
                signature,
                public_key,
                trusted_key_ids,
                revoked_key_ids,
                json,
            } => {
                let policy = mmcg::fact_signature::FactTrustPolicy {
                    signature: Some(signature),
                    public_key: Some(public_key),
                    require_signature: true,
                    trusted_key_ids: trusted_key_ids.into_iter().collect::<BTreeSet<_>>(),
                    revoked_key_ids: revoked_key_ids.into_iter().collect::<BTreeSet<_>>(),
                };
                let report = mmcg::fact_signature::verify(&manifest, &policy)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    println!(
                        "fact manifest verified: {} ({})",
                        report.manifest_digest,
                        report.key_id.as_deref().unwrap_or("unsigned")
                    );
                }
            }
        },
        Cmd::Team(command) => match command {
            TeamCmd::Lock { manifest, output } => {
                let summary = mmcg::team::lock(&manifest, &output)?;
                println!("{}", serde_json::to_string_pretty(&summary)?);
            }
            TeamCmd::Map { manifest } => {
                let graph = mmcg::team::map(&manifest, None)?;
                println!("{}", serde_json::to_string_pretty(&graph)?);
            }
        },
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
            let index_path = index_path_for_root(index_override.as_deref(), &root);
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
        Cmd::Temporal {
            since,
            format,
            root,
            path,
            depth,
            top,
            production_only,
            codeowners,
        } => {
            let root = root
                .canonicalize()
                .map_err(|_| mmcg::queries::ChangeImpactError::RootMismatch)?;
            let index_path = index_path_for_root(index_override.as_deref(), &root);
            commands::query::dispatch_temporal(
                &mmcg::temporal::TemporalOptions {
                    since,
                    path,
                    depth,
                    top,
                    production_only,
                    codeowners,
                },
                format,
                &root,
                &index_path,
            )?;
        }
        Cmd::Policy(PolicyCmd::Check {
            since,
            root,
            config,
            format,
            codeowners,
            workflow_evidence,
            depth,
            top,
        }) => {
            let root = root
                .canonicalize()
                .map_err(|_| mmcg::policy::PolicyError::new_for_cli("policy_root_unavailable"))?;
            let index_path = index_path_for_root(index_override.as_deref(), &root);
            let store = Store::open_read_only(&index_path)
                .map_err(|_| mmcg::policy::PolicyError::new_for_cli("policy_index_unavailable"))?;
            let report = mmcg::policy::check(
                &store,
                &root,
                &mmcg::policy::CheckOptions {
                    since,
                    config_path: config,
                    codeowners,
                    workflow_evidence_path: workflow_evidence,
                    depth,
                    top: usize::try_from(top).map_err(|_| {
                        mmcg::policy::PolicyError::new_for_cli("invalid_policy_limit")
                    })?,
                },
            )?;
            match format {
                PolicyFormat::Text => print!("{}", mmcg::policy::render_text(&report)),
                PolicyFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                }
                PolicyFormat::Sarif => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&mmcg::sarif_export::architecture_policy(
                            &report
                        ))?
                    );
                }
            }
            if !report.passed {
                std::process::exit(1);
            }
        }
        Cmd::Review(ReviewCmd::Export {
            since,
            out,
            root,
            path,
            depth,
            top,
            production_only,
            sarif,
            coverage,
            junit,
            otel,
            codeowners,
            no_project_knowledge,
            git_commits,
            evidence_attestation,
        }) => {
            let root = root
                .canonicalize()
                .map_err(|_| mmcg::lens::LensError::RootUnavailable)?;
            let index_path = index_path_for_root(index_override.as_deref(), &root);
            let result =
                mmcg::review_package::export(&mmcg::review_package::ReviewExportOptions {
                    root,
                    index_path,
                    out,
                    lens: mmcg::lens::LensOptions {
                        since,
                        path,
                        depth,
                        top,
                        production_only,
                    },
                    evidence: mmcg::evidence::EvidenceOptions {
                        sarif,
                        coverage,
                        discover_codeowners: codeowners.is_none(),
                        codeowners,
                        git_commits,
                    },
                    extensions: mmcg::evidence::EvidenceExtensionOptions {
                        junit,
                        otel,
                        project_knowledge: !no_project_knowledge,
                    },
                    evidence_attestation,
                })?;
            println!(
                "review package: {} | head {} | {} | {} files | evidence {}",
                result.output_dir.display(),
                result.head_oid,
                if result.partial {
                    "partial"
                } else {
                    "complete"
                },
                result.artifacts,
                result.evidence_binding,
            );
        }
        Cmd::Ui {
            since,
            root,
            path,
            depth,
            top,
            production_only,
            sarif,
            coverage,
            junit,
            otel,
            codeowners,
            no_project_knowledge,
            git_commits,
            port,
        } => {
            let root = root
                .canonicalize()
                .map_err(|_| mmcg::lens::LensError::RootUnavailable)?;
            let index_path = index_path_for_root(index_override.as_deref(), &root);
            mmcg::lens::run_with_evidence_extensions(
                root,
                index_path,
                mmcg::lens::LensOptions {
                    since,
                    path,
                    depth,
                    top,
                    production_only,
                },
                mmcg::evidence::EvidenceOptions {
                    sarif,
                    coverage,
                    discover_codeowners: codeowners.is_none(),
                    codeowners,
                    git_commits,
                },
                mmcg::evidence::EvidenceExtensionOptions {
                    junit,
                    otel,
                    project_knowledge: !no_project_knowledge,
                },
                port,
            )?;
        }
        Cmd::Serve => {
            let store = Store::open(&index_path)?;
            store.set_default_work_budget_ms(mmcg::store::query_budget_ms_from_env(
                mmcg::store::DEFAULT_SERVE_BUDGET_MS,
            ));
            mmcg::mcp::serve(store)?;
        }
        Cmd::Watch { root } => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            let index_path = index_path_for_root(index_override.as_deref(), &root);
            let store = Store::open(&index_path)?;
            mmcg::watcher::run(root, store)?;
        }
        Cmd::Status { root } => {
            let root = root
                .canonicalize()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or(root));
            let index_path = index_path_for_root(index_override.as_deref(), &root);
            let ws = mmcg::workflow_status::WorkflowStatus::scan_with_index(&root, &index_path);
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
            let index_path = index_path_for_root(index_override.as_deref(), &root);
            let ws = mmcg::workflow_status::WorkflowStatus::scan_with_index(&root, &index_path);
            print!("{}", ws.render_next_text());
        }
        Cmd::Resume { root, task } => {
            let root = root
                .canonicalize()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or(root));
            let index_path = index_path_for_root(index_override.as_deref(), &root);
            let ws = mmcg::workflow_status::WorkflowStatus::scan_with_index(&root, &index_path);
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
            let init_index_path = index_override.as_deref().map(std::path::Path::to_path_buf);
            commands::do_init(
                &root,
                commands::init::InitOpts {
                    index_path: init_index_path,
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
            let index_root = root.canonicalize().unwrap_or_else(|_| root.clone());
            let index_path = index_path_for_root(index_override.as_deref(), &index_root);
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
            let index_root = root.canonicalize().unwrap_or_else(|_| root.clone());
            let index_path = index_path_for_root(index_override.as_deref(), &index_root);
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
            let index_path = index_path_for_root(index_override.as_deref(), &root);
            let me = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
            let report = mmcg::doctor::run_with_index(&root, &me, &index_path);
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
            let index_root = root.canonicalize().unwrap_or_else(|_| root.clone());
            let index_path = index_path_for_root(index_override.as_deref(), &index_root);
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
            let index_root = root.canonicalize().unwrap_or_else(|_| root.clone());
            let index_path = index_path_for_root(index_override.as_deref(), &index_root);
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
    } else if let Some(error) = error.downcast_ref::<mmcg::policy::PolicyError>() {
        error.to_string()
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
    std::hint::black_box(BUILD_VERSION_MARKER);
    render_exit(run(), &mut std::io::stderr())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_scoped_commands_default_the_index_to_the_selected_repository() {
        let root = std::path::Path::new("/workspace/repository");
        assert_eq!(
            index_path_for_root(None, root),
            root.join(".mastermind/mmcg.db")
        );
        assert_eq!(
            index_path_for_root(Some(std::path::Path::new("custom/index.db")), root),
            PathBuf::from("custom/index.db")
        );
    }

    #[test]
    fn workflow_audit_cli_contract_is_explicit() {
        let cli = Cli::try_parse_from([
            "mastermind",
            "workflow",
            "audit",
            "--root",
            "/workflow",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Workflow(WorkflowCmd::Audit { root, json })
                if root == std::path::Path::new("/workflow") && json
        ));
        let Err(usage) = Cli::try_parse_from(["mastermind", "workflow", "audit", "--unknown"])
        else {
            panic!("unknown workflow audit flags must fail parsing");
        };
        assert_eq!(usage.exit_code(), 2);
    }

    #[test]
    fn enrich_and_semantic_query_cli_contracts_are_explicit() {
        let enrich = Cli::try_parse_from([
            "mastermind",
            "--index",
            "graph.db",
            "enrich",
            "--scip",
            "index.scip",
        ])
        .unwrap();
        assert!(
            matches!(enrich.cmd, Cmd::Enrich { scip: Some(scip), facts: None, .. } if scip == std::path::Path::new("index.scip"))
        );

        let query = Cli::try_parse_from([
            "mastermind",
            "query",
            "semantic",
            "scip-clang . demo . target().",
            "--top",
            "25",
        ])
        .unwrap();
        assert!(matches!(
            query.cmd,
            Cmd::Query(QueryCmd::Semantic { symbol, top })
                if symbol == "scip-clang . demo . target()." && top == 25
        ));
    }

    #[test]
    fn enrich_accepts_one_declarative_fact_manifest() {
        let enrich = Cli::try_parse_from([
            "mastermind",
            "--index",
            "graph.db",
            "enrich",
            "--facts",
            "mastermind-facts.json",
        ])
        .unwrap();
        assert!(matches!(
            enrich.cmd,
            Cmd::Enrich { scip: None, facts: Some(path), .. }
                if path == std::path::Path::new("mastermind-facts.json")
        ));

        assert!(Cli::try_parse_from([
            "mastermind",
            "enrich",
            "--scip",
            "index.scip",
            "--facts",
            "mastermind-facts.json",
        ])
        .is_err());

        let query = Cli::try_parse_from([
            "mastermind",
            "query",
            "facts",
            "--path",
            "src",
            "--top",
            "250",
        ])
        .unwrap();
        assert!(matches!(
            query.cmd,
            Cmd::Query(QueryCmd::Facts { path, top }) if path == "src" && top == 250
        ));
        assert!(Cli::try_parse_from(["mastermind", "query", "facts", "--top", "401",]).is_err());
    }

    #[test]
    fn fact_lifecycle_and_team_graph_cli_contracts_are_explicit() {
        let adapt = Cli::try_parse_from([
            "mastermind",
            "facts",
            "adapt",
            "--format",
            "otel",
            "--input",
            "traces.json",
            "--output",
            "facts.json",
            "--producer",
            "collector",
            "--producer-version",
            "1.2.3",
            "--dataset",
            "runtime",
        ])
        .unwrap();
        assert!(matches!(
            adapt.cmd,
            Cmd::Facts(FactCmd::Adapt {
                format: FactAdapterFormat::Otel,
                ..
            })
        ));

        let keygen = Cli::try_parse_from([
            "mastermind",
            "facts",
            "keygen",
            "--private-key",
            "producer.seed",
            "--public-key",
            "producer.pub",
        ])
        .unwrap();
        assert!(matches!(keygen.cmd, Cmd::Facts(FactCmd::Keygen { .. })));

        let signed_import = Cli::try_parse_from([
            "mastermind",
            "enrich",
            "--facts",
            "facts.json",
            "--signature",
            "facts.sig.json",
            "--public-key",
            "facts.pub",
            "--trusted-key-id",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--require-signature",
        ])
        .unwrap();
        assert!(matches!(
            signed_import.cmd,
            Cmd::Enrich {
                facts: Some(_),
                require_signature: true,
                ..
            }
        ));
        assert!(Cli::try_parse_from([
            "mastermind",
            "enrich",
            "--scip",
            "index.scip",
            "--signature",
            "facts.sig.json",
        ])
        .is_err());

        let team = Cli::try_parse_from([
            "mastermind",
            "team",
            "lock",
            "team.json",
            "--output",
            "team.lock.json",
        ])
        .unwrap();
        assert!(matches!(
            team.cmd,
            Cmd::Team(TeamCmd::Lock { manifest, output })
                if manifest == std::path::Path::new("team.json")
                    && output == std::path::Path::new("team.lock.json")
        ));
    }

    #[test]
    fn architecture_policy_cli_contract_is_explicit() {
        let cli = Cli::try_parse_from([
            "mastermind",
            "policy",
            "check",
            "--since",
            "main",
            "--format",
            "sarif",
            "--config",
            "architecture.yml",
            "--workflow-evidence",
            "policy-evidence",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Policy(PolicyCmd::Check {
                since,
                config,
                format: PolicyFormat::Sarif,
                workflow_evidence,
                depth: 3,
                top: 500,
                ..
            }) if since == "main"
                && config == std::path::Path::new("architecture.yml")
                && workflow_evidence == std::path::Path::new("policy-evidence")
        ));
    }

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
    fn claude_setup_keeps_write_mcp_as_an_alias_for_canonical_write() {
        assert!(claude_setup_args(&["mastermind", "setup", "claude", "--write"]).write);
        assert!(claude_setup_args(&["mastermind", "setup", "claude", "--write-mcp"]).write);
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

        let decision = Cli::try_parse_from([
            "mastermind",
            "history",
            "storage boundary",
            "--kind",
            "architecture_decision",
        ])
        .unwrap();
        assert!(matches!(
            decision.cmd,
            Cmd::History { kind: Some(kind), .. } if kind == "architecture_decision"
        ));
    }

    #[test]
    fn architecture_risks_have_sarif_cli_projections() {
        let map = Cli::try_parse_from(["mastermind", "map", ".", "--format", "sarif"]).unwrap();
        assert!(matches!(
            map.cmd,
            Cmd::Map {
                format: MapFormat::Sarif,
                ..
            }
        ));

        let impact = Cli::try_parse_from([
            "mastermind",
            "impact",
            "--since",
            "main",
            "--format",
            "sarif",
        ])
        .unwrap();
        assert!(matches!(
            impact.cmd,
            Cmd::Impact {
                format: ImpactFormat::Sarif,
                ..
            }
        ));
    }

    #[test]
    fn temporal_graph_is_a_bounded_root_scoped_command() {
        assert!(Cli::try_parse_from(["mastermind", "temporal"]).is_err());
        let cli = Cli::try_parse_from([
            "mastermind",
            "temporal",
            "--since",
            "origin/main",
            "--format",
            "json",
            "--path",
            "services/payment",
            "--depth",
            "3",
            "--top",
            "50",
            "--production-only",
            "--codeowners",
            ".github/CODEOWNERS",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Temporal {
                since,
                format: TemporalFormat::Json,
                root,
                path,
                depth: 3,
                top: 50,
                production_only: true,
                codeowners: Some(codeowners),
            } if since == "origin/main"
                && root.as_path() == std::path::Path::new(".")
                && path == "services/payment"
                && codeowners.as_path() == std::path::Path::new(".github/CODEOWNERS")
        ));
        assert!(Cli::try_parse_from(
            ["mastermind", "temporal", "--since", "main", "--depth", "6",]
        )
        .is_err());
    }

    #[test]
    fn lens_ui_is_a_bounded_root_scoped_cli_command() {
        assert!(Cli::try_parse_from(["mastermind", "ui"]).is_err());

        let cli = Cli::try_parse_from([
            "mastermind",
            "ui",
            "--since",
            "origin/main",
            "--production-only",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Ui {
                since,
                root,
                path,
                depth: 3,
                top: 100,
                production_only: true,
                sarif,
                coverage,
                junit,
                otel,
                codeowners: None,
                no_project_knowledge: false,
                git_commits: 200,
                port: 0,
            } if since == "origin/main"
                && root.as_path() == std::path::Path::new(".")
                && path == "."
                && sarif.is_empty()
                && coverage.is_empty()
                && junit.is_empty()
                && otel.is_empty()
        ));

        assert!(
            Cli::try_parse_from(["mastermind", "ui", "--since", "main", "--depth", "6",]).is_err()
        );

        let overlays = Cli::try_parse_from([
            "mastermind",
            "ui",
            "--since",
            "main",
            "--sarif",
            "semgrep.sarif",
            "--sarif",
            "codeql.sarif",
            "--coverage",
            "lcov.info",
            "--coverage",
            "cobertura.xml",
            "--junit",
            "junit.xml",
            "--otel",
            "traces.json",
            "--no-project-knowledge",
            "--codeowners",
            ".github/CODEOWNERS",
            "--git-commits",
            "25",
        ])
        .unwrap();
        assert!(matches!(
            overlays.cmd,
            Cmd::Ui {
                sarif,
                coverage,
                junit,
                otel,
                codeowners: Some(codeowners),
                no_project_knowledge: true,
                git_commits: 25,
                ..
            } if sarif == [PathBuf::from("semgrep.sarif"), PathBuf::from("codeql.sarif")]
                && coverage == [PathBuf::from("lcov.info"), PathBuf::from("cobertura.xml")]
                && junit == [PathBuf::from("junit.xml")]
                && otel == [PathBuf::from("traces.json")]
                && codeowners.as_path() == std::path::Path::new(".github/CODEOWNERS")
        ));
        assert!(Cli::try_parse_from([
            "mastermind",
            "ui",
            "--since",
            "main",
            "--git-commits",
            "1001",
        ])
        .is_err());
    }

    #[test]
    fn review_export_matches_lens_bounds_and_revision_inputs() {
        assert!(
            Cli::try_parse_from(["mastermind", "review", "export", "--since", "main"]).is_err()
        );
        let cli = Cli::try_parse_from([
            "mastermind",
            "review",
            "export",
            "--since",
            "origin/main",
            "--out",
            "mastermind-review",
            "--path",
            "services/payment",
            "--production-only",
            "--sarif",
            "semgrep.sarif",
            "--coverage",
            "lcov.info",
            "--junit",
            "junit.xml",
            "--otel",
            "traces.json",
            "--evidence-attestation",
            "evidence-attestation.json",
            "--git-commits",
            "25",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Review(ReviewCmd::Export {
                since,
                out,
                path,
                depth: 3,
                top: 100,
                production_only: true,
                sarif,
                coverage,
                junit,
                otel,
                git_commits: 25,
                evidence_attestation: Some(attestation),
                ..
            }) if since == "origin/main"
                && out.as_path() == std::path::Path::new("mastermind-review")
                && path == "services/payment"
                && sarif == [PathBuf::from("semgrep.sarif")]
                && coverage == [PathBuf::from("lcov.info")]
                && junit == [PathBuf::from("junit.xml")]
                && otel == [PathBuf::from("traces.json")]
                && attestation.as_path() == std::path::Path::new("evidence-attestation.json")
        ));
        assert!(Cli::try_parse_from([
            "mastermind",
            "review",
            "export",
            "--since",
            "main",
            "--out",
            "review",
            "--depth",
            "6",
        ])
        .is_err());
    }
}
