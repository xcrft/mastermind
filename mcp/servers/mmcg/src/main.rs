//! CLI entry point for mastermind (the `mmcg` codegraph binary).
//!
//! Common subcommands:
//!   mastermind init           — scaffold a project, build the index, draft CONTEXT.md
//!   mastermind setup claude   — register the codegraph with Claude Code (MCP)
//!   mastermind index [PATH]   — build or refresh the index
//!   mastermind serve          — run as MCP stdio server
//!   mastermind doctor         — health-check the setup
//!   mastermind query <kind>   — one-shot query from the CLI (handy for debugging)

use clap::{Parser, Subcommand, ValueEnum};
use mmcg::indexer::Indexer;
use mmcg::queries;
use mmcg::store::Store;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Templates embedded at build time. These live inside the crate as a mirror of
/// canonical sources (`agents/claude-md/mastermind-{context,workflow}.md` and
/// `skills/workflow/mastermind-task-planning/references/spec-template.md`) so the
/// published crate is self-contained — `cargo publish` cannot reach files outside
/// the crate root, and reaching `../../../..` only works in a checked-out repo.
///
/// **Keep in sync** with the canonical files. `scripts/validate.py` enforces this
/// (run after any edit to either side). To sync manually:
///   cp agents/claude-md/mastermind-context.md mcp/servers/mmcg/templates/context.md
///   cp agents/claude-md/mastermind-workflow.md mcp/servers/mmcg/templates/workflow.md
///   cp skills/workflow/mastermind-task-planning/references/spec-template.md \
///      mcp/servers/mmcg/templates/spec-template.md
const CONTEXT_TEMPLATE: &str = include_str!("../templates/context.md");
const WORKFLOW_TEMPLATE: &str = include_str!("../templates/workflow.md");
const SPEC_TEMPLATE: &str = include_str!("../templates/spec-template.md");

// Per-stack CONTEXT.md profiles (`mastermind init --profile <name>`). Each one is a
// pre-seeded version of context.md with stack-specific layout conventions, test
// commands, and gotchas baked in. Adding a new profile = add a file under
// `templates/profiles/` + a const here + a `Profile` enum variant.
const PROFILE_TYPESCRIPT_API: &str = include_str!("../templates/profiles/typescript-api.md");
const PROFILE_REACT_NATIVE: &str = include_str!("../templates/profiles/react-native.md");
const PROFILE_PYTHON_FASTAPI: &str = include_str!("../templates/profiles/python-fastapi.md");
const PROFILE_RUST_CLI: &str = include_str!("../templates/profiles/rust-cli.md");

#[derive(Copy, Clone, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum Profile {
    /// Generic CONTEXT.md (current default — no stack-specific seeding).
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
enum UninstallScope {
    /// This project only: remove `.mastermind/` and the project `.mcp.json` mmcg entry.
    Project,
    /// Global only: de-register mmcg from `~/.claude/.mcp.json` (leaves `.mastermind/` alone).
    Global,
    /// Both project and global.
    All,
}

fn profile_template(p: Profile) -> &'static str {
    match p {
        Profile::Generic => CONTEXT_TEMPLATE,
        Profile::TypescriptApi => PROFILE_TYPESCRIPT_API,
        Profile::ReactNative => PROFILE_REACT_NATIVE,
        Profile::PythonFastapi => PROFILE_PYTHON_FASTAPI,
        Profile::RustCli => PROFILE_RUST_CLI,
    }
}

fn profile_label(p: Profile) -> &'static str {
    match p {
        Profile::Generic => "generic",
        Profile::TypescriptApi => "typescript-api",
        Profile::ReactNative => "react-native",
        Profile::PythonFastapi => "python-fastapi",
        Profile::RustCli => "rust-cli",
    }
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
    /// Print one-shot status (file count, symbol count, db path).
    Status,
    /// Scaffold a project for the Mastermind workflow: create .mastermind/tasks/,
    /// CONTEXT.md (only if missing) and the index, then build the index and
    /// (unless --no-claude) populate CONTEXT.md from the codebase via `claude -p`.
    /// Does NOT touch an existing CLAUDE.md (pass --with-claude-md to drop + fill the template).
    Init {
        /// Project root. Defaults to cwd.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Also drop the Mastermind workflow CLAUDE.md template and fill its <PLACEHOLDER> sections
        /// from the codebase via `claude -p` (unless --no-claude). Refuses to overwrite an existing
        /// CLAUDE.md unless --force is passed.
        #[arg(long)]
        with_claude_md: bool,
        /// Overwrite existing files (CONTEXT.md, CLAUDE.md). Off by default.
        #[arg(long)]
        force: bool,
        /// CONTEXT.md template variant. `generic` (default) is stack-agnostic.
        /// Stack-specific profiles pre-seed the file with conventions, test
        /// commands, and canonical gotchas — prune what doesn't apply.
        #[arg(long, value_enum, default_value = "generic")]
        profile: Profile,
        /// Skip the automatic index build (otherwise `init` runs `index .` for you).
        #[arg(long)]
        no_index: bool,
        /// Skip auto-populating CONTEXT.md via `claude -p` (e.g. offline, in CI, or
        /// when the Claude Code CLI isn't installed). The bare template is left in place.
        #[arg(long)]
        no_claude: bool,
        /// Skip installing the workflow subagents + skills into ~/.claude/. npm
        /// installs do this by default so the full workflow (not just the codegraph)
        /// is available; it overwrites Mastermind's own files there.
        #[arg(long)]
        no_global: bool,
    },
    /// Remove a Mastermind setup. By default (`--scope project`) deletes
    /// `.mastermind/` (index, tasks, run-state) and de-registers the `mmcg`
    /// entry from the project `.mcp.json`. `--scope global` removes only the
    /// `~/.claude/.mcp.json` entry; `--scope all` does both. Never touches
    /// CONTEXT.md / CLAUDE.md. Safe by default: prints the plan and exits unless
    /// `--force` is passed.
    Uninstall {
        /// Project root. Defaults to cwd. (Ignored for `--scope global`.)
        #[arg(default_value = ".")]
        root: PathBuf,
        /// What to remove: `project` (.mastermind/ + project .mcp.json),
        /// `global` (the ~/.claude/.mcp.json mmcg entry), or `all`.
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
        /// Path to a `.mastermind/tasks/XXX-*.md` spec.
        spec: PathBuf,
        /// Project root the spec's file paths are relative to. Defaults to cwd.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        json: bool,
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
    },
    /// One-shot query — handy for CLI debugging without going through MCP.
    #[command(subcommand)]
    Query(QueryCmd),
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
}

#[derive(Subcommand)]
enum SetupCmd {
    /// Register mmcg with Claude Code's MCP layer. Merges into existing
    /// `mcpServers` rather than clobbering. Default = print diff + exit
    /// without writing. Pass `--write-mcp` to apply.
    Claude {
        /// Project-local target: writes `<path>/.mcp.json` instead of the
        /// global `~/.claude/.mcp.json`.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Actually write the config file. Without this, prints diff only.
        #[arg(long)]
        write_mcp: bool,
        /// Alias for the default (no-write) mode — useful for scripting clarity.
        #[arg(long)]
        dry_run: bool,
        /// Also drop the workflow CLAUDE.md template into the project root.
        /// Refuses to overwrite existing CLAUDE.md unless `--force`.
        #[arg(long)]
        with_workflow: bool,
        /// Overwrite a customized `mmcg` entry or existing CLAUDE.md.
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
            let indexer = Indexer::new(&root);
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
        Cmd::Status => {
            let store = Store::open(&index_path)?;
            let s = queries::status(&store)?;
            println!("{}", serde_json::to_string_pretty(&s)?);
        }
        Cmd::Init {
            root,
            with_claude_md,
            force,
            profile,
            no_index,
            no_claude,
            no_global,
        } => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            do_init(
                &root,
                with_claude_md,
                force,
                profile,
                !no_index,
                !no_claude,
                !no_global,
            )?;
        }
        Cmd::Uninstall { root, scope, force } => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            do_uninstall(&root, scope, force)?;
        }
        Cmd::Setup(SetupCmd::Claude {
            project,
            write_mcp,
            dry_run: _,
            with_workflow,
            force,
        }) => {
            // `--dry-run` is documented as an alias for the default (no-write)
            // mode — it's already the default, so passing it doesn't change
            // behavior. We accept it for scripting clarity (lets a wrapper say
            // "always set --dry-run explicitly").
            let target = if let Some(p) = project {
                let root = p
                    .canonicalize()
                    .map_err(|e| format!("canonicalize {}: {e}", p.display()))?;
                mmcg::setup::Target::project(&root)
            } else {
                mmcg::setup::Target::global()?
            };
            let project_root = std::env::current_dir().map_err(|e| format!("cwd: {e}"))?;
            let me = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
            let outcome = mmcg::setup::run_claude(
                &target,
                &me,
                &project_root,
                &strip_template_comment(WORKFLOW_TEMPLATE),
                mmcg::setup::Opts {
                    write: write_mcp,
                    force,
                    with_workflow,
                },
            );
            if matches!(
                outcome,
                mmcg::setup::Outcome::Error | mmcg::setup::Outcome::RefusedOverwrite
            ) {
                std::process::exit(1);
            }
        }
        Cmd::VerifySpec { spec, root, json } => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            let parsed = mmcg::spec::parse_file(&spec)
                .map_err(|e| format!("parse {}: {e}", spec.display()))?;
            // Store is optional — skip live mmcg checks if no index. That
            // still gives mandatory-sections + missing-files coverage.
            let store = Store::open(&index_path).ok();
            let report = mmcg::verify_spec::run(&parsed, store.as_ref(), &root);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", report.render_text());
            }
            if report.has_failures() {
                std::process::exit(1);
            }
        }
        Cmd::AuditSpec {
            spec,
            since,
            root,
            json,
        } => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            let parsed = mmcg::spec::parse_file(&spec)
                .map_err(|e| format!("parse {}: {e}", spec.display()))?;
            let store = Store::open(&index_path)?;
            let report = mmcg::audit_spec::run(&parsed, &store, &root, &since)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", report.render_text());
            }
            if report.has_failures() {
                std::process::exit(1);
            }
        }
        Cmd::Doctor { root, json } => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            // Use the binary we're already running for the MCP handshake check —
            // guarantees the version under test matches what's installed.
            let me = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
            let report = mmcg::doctor::run(&root, &me);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", report.render_text());
            }
            // Doctor "failures" are environment problems, not runtime errors.
            // Exit non-zero so CI / shell scripts can react; bypass the
            // generic error-printing path in main().
            if report.has_failures() {
                std::process::exit(1);
            }
        }
        Cmd::RunTask {
            spec,
            root,
            reset,
            pre_only,
            post_only,
            exec,
            allow_no_index,
        } => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            let opts = mmcg::run_task::RunOpts {
                reset,
                pre_only,
                post_only,
                exec,
                allow_no_index,
            };
            let outcome = mmcg::run_task::run(&spec, &root, &index_path, opts);
            // Map outcomes to exit codes: gate failures / contract breaks /
            // executor failures all surface as non-zero so CI / shell pipelines
            // can react. PostDrift is deliberately exit 0 — warnings, not blocks.
            if matches!(
                outcome,
                mmcg::run_task::Outcome::PreFailed
                    | mmcg::run_task::Outcome::PostBroken
                    | mmcg::run_task::Outcome::ExecFailed
            ) {
                std::process::exit(1);
            }
        }
        Cmd::Query(q) => {
            let store = Store::open(&index_path)?;
            let result = match q {
                QueryCmd::Search {
                    name,
                    kind,
                    language,
                    no_collapse_partials,
                } => serde_json::to_value(queries::search(
                    &store,
                    &name,
                    kind.as_deref(),
                    language.as_deref(),
                    !no_collapse_partials,
                )?)?,
                QueryCmd::Callers {
                    name,
                    language,
                    edge_kind,
                } => serde_json::to_value(queries::callers(
                    &store,
                    &name,
                    language.as_deref(),
                    edge_kind.as_deref(),
                )?)?,
                QueryCmd::Callees {
                    name,
                    language,
                    edge_kind,
                } => serde_json::to_value(queries::callees(
                    &store,
                    &name,
                    language.as_deref(),
                    edge_kind.as_deref(),
                )?)?,
                QueryCmd::Impact {
                    name,
                    depth,
                    language,
                } => serde_json::to_value(queries::impact(
                    &store,
                    &name,
                    depth,
                    language.as_deref(),
                )?)?,
                QueryCmd::Files { prefix, language } => serde_json::to_value(queries::files(
                    &store,
                    prefix.as_deref(),
                    language.as_deref(),
                )?)?,
                QueryCmd::SymbolsInFile { file } => {
                    serde_json::to_value(queries::symbols_in_file(&store, &file)?)?
                }
                QueryCmd::Outline { file } => {
                    serde_json::to_value(queries::outline(&store, &file)?)?
                }
                QueryCmd::Recent { since } => serde_json::to_value(
                    queries::recent_changes(&store, &since)
                        .map_err(|e| format!("recent_changes: {e}"))?,
                )?,
                QueryCmd::Unreferenced { kind, language } => serde_json::to_value(
                    queries::unreferenced(&store, kind.as_deref(), language.as_deref())?,
                )?,
                QueryCmd::ApiSurface { prefix, language } => serde_json::to_value(
                    queries::api_surface(&store, &prefix, language.as_deref())?,
                )?,
                QueryCmd::DependencyCycles { language, min_size } => serde_json::to_value(
                    queries::dependency_cycles(&store, language.as_deref(), min_size)?,
                )?,
                QueryCmd::Centrality {
                    prefix,
                    language,
                    kind,
                    top,
                } => serde_json::to_value(queries::centrality(
                    &store,
                    prefix.as_deref(),
                    language.as_deref(),
                    kind.as_deref(),
                    top,
                )?)?,
                QueryCmd::Tasks { query, top } => {
                    serde_json::to_value(queries::tasks(&store, &query, top)?)?
                }
                QueryCmd::SymbolsChangedSince { git_ref, root } => {
                    let root = root
                        .canonicalize()
                        .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
                    let diff = queries::symbols_changed_since(&store, &root, &git_ref)?;
                    serde_json::to_value(diff)?
                }
                QueryCmd::ImportedBy {
                    query,
                    match_kind,
                    language,
                } => serde_json::to_value(queries::imported_by(
                    &store,
                    &query,
                    &match_kind,
                    language.as_deref(),
                )?)?,
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }
    Ok(())
}

/// Strip the HTML-comment "instructions to the user" block from a template so
/// the copied file is what the adopter actually uses, not the template-meta.
fn strip_template_comment(text: &str) -> String {
    // Find the COPY FROM HERE marker; if absent, return text as-is.
    let marker_open = "<!-- ─── COPY FROM HERE ─── -->";
    let marker_close = "<!-- ─── COPY TO HERE ─── -->";
    if let Some(start) = text.find(marker_open) {
        let body_start = start + marker_open.len();
        let body_end = text[body_start..]
            .find(marker_close)
            .map(|i| body_start + i)
            .unwrap_or(text.len());
        text[body_start..body_end].trim().to_string() + "\n"
    } else {
        text.to_string()
    }
}

fn write_if_absent(
    path: &Path,
    contents: &str,
    force: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    if path.exists() && !force {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(true)
}

fn do_init(
    root: &Path,
    with_claude_md: bool,
    force: bool,
    profile: Profile,
    index: bool,
    claude: bool,
    global: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut created: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut context_fill_prompt: Option<String> = None;

    // 1. .mastermind/ — single working-state folder, fully gitignored.
    //    Contains tasks/, the mmcg index, and anything else session-scratch.
    let mastermind_dir = root.join(".mastermind");
    let tasks_dir = mastermind_dir.join("tasks");
    if !mastermind_dir.exists() {
        fs::create_dir_all(&mastermind_dir)?;
        // Defensive inner .gitignore: even if the project's root .gitignore doesn't
        // list .mastermind/, every file under here stays untracked. Add `!pattern`
        // lines below the `*` if you ever want to share a specific file (e.g. a spec
        // worth preserving as an example).
        fs::write(
            mastermind_dir.join(".gitignore"),
            "# Generated by `mastermind init` — local working state, not for commit.\n\
             # To share a specific file, add `!path/to/file` AFTER the `*` line below.\n\
             *\n\
             !.gitignore\n",
        )?;
        created.push(".mastermind/.gitignore".into());
    } else {
        skipped.push(".mastermind/ (already exists)".into());
    }

    // 2. .mastermind/tasks/ — spec files live here (was `.tasks/` at root pre-0.6.0)
    if !tasks_dir.exists() {
        fs::create_dir_all(&tasks_dir)?;
        created.push(".mastermind/tasks/".into());
    } else {
        skipped.push(".mastermind/tasks/ (already exists)".into());
    }

    // Surface legacy `.tasks/` at project root (pre-0.6.0 layout) so the user knows
    // to migrate it. Don't auto-move — user might have it in active use.
    let legacy_tasks = root.join(".tasks");
    if legacy_tasks.exists() {
        warnings.push(
            "legacy `.tasks/` directory exists at project root (pre-0.6.0 layout). \
             Migrate it with: `mv .tasks/* .mastermind/tasks/ && rmdir .tasks` \
             — specs now live under `.mastermind/tasks/`."
                .to_string(),
        );
    }

    // 3. CONTEXT.md from the picked profile template (strip the HTML-comment
    //    instructions block). `--profile generic` keeps the original
    //    stack-agnostic template; stack-specific profiles pre-seed the file
    //    with conventions / commands / gotchas — see `templates/profiles/`.
    let context_path = root.join("CONTEXT.md");
    let context_body = strip_template_comment(profile_template(profile));
    let context_created = write_if_absent(&context_path, &context_body, force)?;
    if context_created {
        let label = match profile {
            Profile::Generic => "CONTEXT.md".to_string(),
            _ => format!("CONTEXT.md (profile: {})", profile_label(profile)),
        };
        created.push(label);
    } else {
        skipped.push("CONTEXT.md (already exists — pass --force to overwrite)".into());
    }

    // 4. CLAUDE.md (optional)
    let mut claude_md_created = false;
    if with_claude_md {
        let claude_path = root.join("CLAUDE.md");
        let claude_body = strip_template_comment(WORKFLOW_TEMPLATE);
        if write_if_absent(&claude_path, &claude_body, force)? {
            created.push("CLAUDE.md".into());
            claude_md_created = true;
        } else {
            skipped.push("CLAUDE.md (already exists — pass --force to overwrite)".into());
        }
    }

    // 5. .mastermind/tasks/_spec-template.md — drop the spec template
    let spec_template_path = tasks_dir.join("_spec-template.md");
    if write_if_absent(&spec_template_path, SPEC_TEMPLATE, force)? {
        created.push(".mastermind/tasks/_spec-template.md".into());
    } else {
        skipped.push(".mastermind/tasks/_spec-template.md (already exists)".into());
    }

    // 6. Index database
    let db_path = mastermind_dir.join("mmcg.db");
    if !db_path.exists() {
        let _ = Store::open(&db_path)?; // creates schema
        created.push(".mastermind/mmcg.db".into());
    } else {
        skipped.push(".mastermind/mmcg.db (already exists)".into());
    }

    // 7. Build the index now so the project is queryable immediately — `init`
    //    should leave you ready to use, not half-configured. `--no-index` opts out.
    if index {
        let mut store = Store::open(&db_path)?;
        let indexer = Indexer::new(root);
        match indexer.index_all(&mut store, false) {
            Ok(stats) => created.push(format!(
                "indexed {} files, {} symbols, {} edges ({} ms)",
                stats.files_indexed, stats.symbols_total, stats.edges_total, stats.duration_ms
            )),
            Err(e) => warnings.push(format!(
                "index build failed: {e} — run `mastermind index .` manually"
            )),
        }
    } else {
        skipped.push("index build (--no-index) — run `mastermind index .` when ready".into());
    }

    // 8. Draft the scaffold's project-specific content from the codebase via
    //    `claude -p` — CONTEXT.md (when freshly created) and, with --with-claude-md,
    //    the CLAUDE.md <PLACEHOLDER> sections. One claude run fills both. Only
    //    targets files we just created (never clobbers user-edited ones). Best-effort:
    //    a missing CLI or non-zero exit never fails init (we fall back to the prompt).
    let fill_context = claude && context_created;
    let fill_claude_md = claude && claude_md_created;
    if fill_context || fill_claude_md {
        let claude_md_path = root.join("CLAUDE.md");
        let prompt = draft_prompt(
            fill_context.then_some(context_path.as_path()),
            fill_claude_md.then_some(claude_md_path.as_path()),
        );
        match run_claude_draft(root, &prompt) {
            Ok(()) => {
                if fill_context {
                    created.push("CONTEXT.md populated via `claude -p`".into());
                }
                if fill_claude_md {
                    created.push("CLAUDE.md placeholders filled via `claude -p`".into());
                }
            }
            Err(e) => {
                warnings.push(format!("claude -p auto-fill skipped: {e}"));
                context_fill_prompt = Some(prompt);
            }
        }
    }

    // 9. Install the workflow subagents + skills into ~/.claude/ — the part the
    //    npm binary alone doesn't give you. Overwrites Mastermind's own files so
    //    they stay current; leaves other files in those dirs alone. `--no-global`
    //    opts out. A cargo install ships no bundle, so it's skipped with a pointer
    //    to the plugin marketplace.
    if global {
        match install_workflow_global() {
            Ok(msg) => created.push(msg),
            Err(e) => warnings.push(format!("global workflow install skipped — {e}")),
        }
    }

    // Report
    println!("Mastermind workflow initialized at {}", root.display());
    if !created.is_empty() {
        println!("\nCreated:");
        for c in &created {
            println!("  + {c}");
        }
    }
    if !warnings.is_empty() {
        println!("\nWarnings:");
        for w in &warnings {
            println!("  ! {w}");
        }
    }
    if !skipped.is_empty() {
        println!("\nSkipped:");
        for s in &skipped {
            println!("  - {s}");
        }
    }

    println!("\nNext steps:");
    println!("  1. Register with Claude Code:  mastermind setup claude --write-mcp");
    println!("     (run once — the global server serves whichever project you open)");
    println!("  2. Add `.mastermind/` to your project's root `.gitignore` (local working state)");
    println!("  3. (Optional) Keep the index fresh in another terminal:  mastermind watch");
    if !with_claude_md {
        println!("  4. Adopt the workflow CLAUDE.md:  re-run `mastermind init --with-claude-md`");
    } else if claude {
        println!(
            "  4. Review the drafted CLAUDE.md — placeholders were filled from your codebase."
        );
    } else {
        println!("  4. Fill CLAUDE.md's <PLACEHOLDER> sections (--no-claude skipped auto-fill).");
    }
    if let Some(prompt) = context_fill_prompt {
        println!(
            "\nScaffold files were left as templates. To fill them, paste this into Claude Code:\n\n  {prompt}"
        );
    }

    Ok(())
}

/// Build the `claude -p` prompt that fills the freshly-scaffolded files from the
/// codebase. Each file gets scoped instructions so Claude fills only what should
/// be project-specific and leaves accumulating sections as templates. Also used
/// as the printed fallback when the Claude CLI is unavailable.
fn draft_prompt(context: Option<&Path>, claude_md: Option<&Path>) -> String {
    let mut s = String::from(
        "Fill in the following freshly-scaffolded Mastermind files for THIS project by reading the \
         codebase (use the mmcg MCP tools and file access). State only things that are true about the \
         project and not trivially derivable; do not invent.",
    );
    if let Some(p) = context {
        s.push_str(&format!(
            "\n- `{}`: fill ONLY the Identity (what it is / what it is not / primary users) and Active \
             goals sections. Leave Decision log, Known gotchas, Domain glossary, External dependencies, \
             and Don't-touch list as the empty templates — those accumulate over time.",
            p.display()
        ));
    }
    if let Some(p) = claude_md {
        s.push_str(&format!(
            "\n- `{}`: replace the <PLACEHOLDER> tokens (project name, and the run / test / typecheck / \
             lint commands) with the project's real values, inferred from package manifests, scripts, and \
             config. Leave the workflow instructions intact — only fill the placeholders.",
            p.display()
        ));
    }
    s
}

/// Best-effort: shell out to `claude -p` to draft the scaffold files from the
/// codebase. Runs in `root` with `--permission-mode acceptEdits` so the file
/// writes apply without a prompt (it still prompts for Bash/other tools). Returns
/// Err on spawn failure or non-zero exit so the caller can fall back to printing
/// the prompt — auto-fill never fails the overall `init`.
fn run_claude_draft(root: &Path, prompt: &str) -> Result<(), String> {
    println!("\nDrafting scaffold files via `claude -p` (pass --no-claude to skip)...\n");
    let status = std::process::Command::new("claude")
        .arg("-p")
        .arg(prompt)
        .arg("--permission-mode")
        .arg("acceptEdits")
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|e| {
            format!("spawn claude: {e} — is the Claude Code CLI installed and on PATH?")
        })?;
    if !status.success() {
        return Err(format!("claude exited with {status}"));
    }
    Ok(())
}

/// Install the bundled workflow subagents + skills into `~/.claude/` so the full
/// Mastermind workflow — not just the codegraph — is available to Claude Code.
/// Reads the bundle from `$MASTERMIND_SHARE_DIR` (set by the npm wrapper); a
/// cargo install ships no bundle, so it's skipped with a marketplace pointer.
/// Only runs when `~/.claude/` already exists (Claude Code is set up). Overwrites
/// the files Mastermind ships; leaves any other files in those dirs untouched.
fn install_workflow_global() -> Result<String, String> {
    let share = std::env::var("MASTERMIND_SHARE_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "no bundle (cargo install?) — add the workflow via the Claude Code plugin \
             marketplace: `/plugin marketplace add xcrft/mastermind`"
                .to_string()
        })?;
    let share = Path::new(&share);
    let home = dirs::home_dir().ok_or_else(|| "no $HOME — cannot locate ~/.claude".to_string())?;
    let claude = home.join(".claude");
    if !claude.exists() {
        return Err(format!(
            "{} not found (is Claude Code installed?)",
            claude.display()
        ));
    }

    // Subagents → ~/.claude/agents/ (flat `.md` files).
    let mut agents = 0usize;
    let src_agents = share.join("agents");
    if src_agents.is_dir() {
        let dst = claude.join("agents");
        fs::create_dir_all(&dst).map_err(|e| format!("create {}: {e}", dst.display()))?;
        for entry in fs::read_dir(&src_agents).map_err(|e| e.to_string())? {
            let p = entry.map_err(|e| e.to_string())?.path();
            if p.extension().and_then(|x| x.to_str()) == Some("md") {
                let name = p.file_name().expect("dir entry has a name");
                fs::copy(&p, dst.join(name)).map_err(|e| format!("copy {}: {e}", p.display()))?;
                agents += 1;
            }
        }
    }

    // Skills → ~/.claude/skills/<name>/ (directories, copied recursively).
    let mut skills = 0usize;
    let src_skills = share.join("skills");
    if src_skills.is_dir() {
        let dst_root = claude.join("skills");
        for entry in fs::read_dir(&src_skills).map_err(|e| e.to_string())? {
            let p = entry.map_err(|e| e.to_string())?.path();
            if p.is_dir() {
                let name = p.file_name().expect("dir entry has a name");
                copy_dir_all(&p, &dst_root.join(name))
                    .map_err(|e| format!("copy skill {}: {e}", p.display()))?;
                skills += 1;
            }
        }
    }

    Ok(format!(
        "installed {agents} subagents + {skills} skills into {}",
        claude.display()
    ))
}

/// Recursively copy `src` into `dst` (created if missing), overwriting files.
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Reverse of `do_init`. `Project` scope removes `.mastermind/` + the project
/// `.mcp.json` mmcg entry; `Global` removes the `~/.claude/.mcp.json` entry;
/// `All` does both. Safe by default — prints the plan and exits unless `force`.
/// Never touches CONTEXT.md / CLAUDE.md (user-edited).
fn do_uninstall(
    root: &Path,
    scope: UninstallScope,
    force: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let do_project = matches!(scope, UninstallScope::Project | UninstallScope::All);
    let do_global = matches!(scope, UninstallScope::Global | UninstallScope::All);
    println!(
        "=== mastermind uninstall — scope: {} ===",
        scope_label(scope)
    );

    if do_project {
        let mastermind_dir = root.join(".mastermind");
        if mastermind_dir.exists() {
            if force {
                fs::remove_dir_all(&mastermind_dir)?;
                println!(
                    "Removed {} (index, tasks, run-state).",
                    mastermind_dir.display()
                );
            } else {
                println!(
                    "Would remove {} (index, tasks, run-state).",
                    mastermind_dir.display()
                );
            }
        } else {
            println!(
                "No `.mastermind/` at {} — nothing to remove there.",
                root.display()
            );
        }
        // Prints its own diff + dry-run / written notice.
        mmcg::setup::remove_claude(&mmcg::setup::Target::project(root), force);
    }

    if do_global {
        mmcg::setup::remove_claude(&mmcg::setup::Target::global()?, force);
    }

    if !force {
        println!("\n(dry-run — pass --force to apply. CONTEXT.md / CLAUDE.md are never touched.)");
    }
    Ok(())
}

fn scope_label(s: UninstallScope) -> &'static str {
    match s {
        UninstallScope::Project => "project (.mastermind/ + project .mcp.json)",
        UninstallScope::Global => "global (~/.claude/.mcp.json)",
        UninstallScope::All => "all (project + global)",
    }
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
