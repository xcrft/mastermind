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

/// CONTEXT.md profile variants. Adding a new profile:
///   1. Add a file under `templates/profiles/`.
///   2. Add a const in `templates.rs`.
///   3. Add an arm in `templates::for_profile` and `templates::profile_label`.
///   4. Add a variant here.
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
    /// Rust command-line tool.
    RustCli,
}

/// Which parts of a Mastermind setup `uninstall` should remove.
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
    /// Auto-detects the stack profile and always drops the workflow CLAUDE.md
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
        /// Skip installing the workflow subagents, skills, and slash commands into ~/.claude/. npm
        /// installs do this by default so the full workflow (not just the codegraph)
        /// is available; it overwrites Mastermind's own files there.
        #[arg(long)]
        no_global: bool,
        /// Skip seeding `~/.mastermind/style.md` (the author's code-shape "write like me"
        /// profile, mined from git history). On by default; seeds only if the file is absent.
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
    /// Interactive configuration for an external tool — currently only Claude
    /// Code. Safe by default: prints a diff and exits without writing unless
    /// `--write-mcp` is passed.
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
    /// the actual repo state. Diffs against `<git-ref>` (typically `main` or
    /// merge-base): claimed files vs `git diff --name-only`, pre-edit
    /// snapshot vs live `mmcg_callers` counts, snapshot symbols still exist.
    /// Exit code 0 unless verdict is `broken`.
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
        /// `<!-- mastermind:executor-begin -->` sentinel). When provided,
        /// integration-claim verification and vacuous-test detection run on
        /// top of the standard Phase A checks.
        #[arg(long)]
        executor_report: Option<PathBuf>,
        /// Write a portable audit bundle JSON to this path. Contains verdict,
        /// files_diff, discrepancies, snapshot_drift, executor_report_path.
        #[arg(long)]
        bundle: Option<PathBuf>,
    },
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
    /// gates. Auto-resumes via `.mastermind/run-state/<basename>.json`.
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
        /// Path to the bundle JSON file (written by `audit-spec --bundle`).
        bundle: PathBuf,
    },
    /// CI gate: index, verify all specs, run audit for every spec that has an
    /// executor-report.md, and optionally write bundles. Exit 0 if all pass.
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
    },
    /// Scaffold a new task spec under `.mastermind/tasks/`. Picks the next
    /// available NNN sequence number automatically.
    NewSpec {
        /// Short description of the task. Used as the spec title and folder slug.
        description: String,
        /// Template complexity: `lite` (Goal / Scope / Pre-edit snapshot / Verify),
        /// `standard` (adds Alternatives / Codeflow / Tests / Docs / Observability / Performance),
        /// or `strict` (adds Risk Register / Evidence Ledger / Rollback / 3-lens critic panel).
        #[arg(long, default_value = "lite")]
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
    /// Audit CONTEXT.md quality: placeholder residue, minimum content,
    /// stack section presence, decision log, freshness vs task specs,
    /// and _lessons.md existence. Exit code 1 if any check fails.
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
enum MinerCmd {
    /// Mine an author's style ("write like me") from their git history into
    /// `~/.mastermind/style.md`: code-shape idioms (indentation, quotes, line
    /// length, comment density, brace style, declarations) plus commit conventions
    /// (prefix, subject length, body usage). Deterministic by default. Each run
    /// enriches a user-global cross-repo store; hand edits in the manual block are
    /// preserved across re-mines.
    Profile {
        /// Repository to mine. Defaults to cwd.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Author to profile — substring of name or email. Defaults to
        /// `git config user.name` (matches all the person's emails).
        #[arg(long)]
        author: Option<String>,
        /// Rebuild the cross-repo store from scratch (drop every other repo's
        /// contribution), then re-mine this one. Without it, mining enriches.
        #[arg(long)]
        force: bool,
        /// Also run a deep LLM analysis (design patterns, tendencies) via `claude -p`.
        /// Sends sampled added lines + commit messages to the CLI. Slower and
        /// non-deterministic; not part of the deterministic core.
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
    /// Register mmcg with Claude Code's MCP layer. Merges into existing
    /// `mcpServers` rather than clobbering. Default = print diff + exit
    /// without writing. Pass `--write-mcp` to apply.
    Claude {
        /// Project-local target: writes `<path>/.mcp.json` instead of
        /// registering at user scope via `claude mcp add`.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Actually write the config file. Without this, prints diff only.
        #[arg(long)]
        write_mcp: bool,
        /// Alias for the default (no-write) mode — useful for scripting clarity.
        #[arg(long)]
        dry_run: bool,
        /// Overwrite a customized `mmcg` entry.
        #[arg(long)]
        force: bool,
    },
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

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = parse_cli();
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
                "indexed {} (unchanged {}, purged {}, failed {}) / scanned {} | {} symbols | {} edges | {} task specs | {} ms",
                stats.files_indexed,
                stats.files_unchanged,
                stats.files_purged,
                stats.files_failed,
                stats.files_scanned,
                stats.symbols_total,
                stats.edges_total,
                stats.task_specs_indexed,
                stats.duration_ms
            );
            if stats.files_failed > 0 {
                eprintln!("warning: {} files failed to parse", stats.files_failed);
            }
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
        Cmd::Setup(SetupCmd::Claude {
            project,
            write_mcp,
            dry_run: _,
            force,
        }) => {
            let me = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
            let opts = mmcg::setup::Opts {
                write: write_mcp,
                force,
            };
            let outcome = if let Some(p) = project {
                let root = p
                    .canonicalize()
                    .map_err(|e| format!("canonicalize {}: {e}", p.display()))?;
                mmcg::setup::run_claude(&mmcg::setup::Target::project(&root), &me, opts)
            } else {
                mmcg::setup::add_claude_user(&me, opts)
            };
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
        Cmd::PrComment { bundle } => {
            commands::pr_comment(&bundle)?;
        }
        Cmd::Ci {
            since,
            root,
            bundle_dir,
        } => {
            let ok = commands::ci(
                commands::ci::CiOpts {
                    since,
                    root,
                    bundle_dir,
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

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mmcg error: {e}");
            ExitCode::FAILURE
        }
    }
}
