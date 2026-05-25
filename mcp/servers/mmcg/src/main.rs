//! CLI entry point for mmcg.
//!
//! Subcommands:
//!   mmcg index [PATH]   — build or refresh the index
//!   mmcg serve          — run as MCP stdio server
//!   mmcg status         — print index health
//!   mmcg query <kind>   — one-shot query from the CLI (handy for debugging)

use clap::{Parser, Subcommand};
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

#[derive(Parser)]
#[command(
    name = "mmcg",
    version,
    about = "Mastermind Codegraph — Python code indexer with MCP interface",
    long_about = None
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
    /// Scaffold a project for the Mastermind workflow: create .mastermind/tasks/, .mastermind/,
    /// CONTEXT.md (only if missing), and an initial empty index. Does NOT touch
    /// existing CLAUDE.md — adopt the workflow template from agents/claude-md/ manually.
    Init {
        /// Project root. Defaults to cwd.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Also drop a copy of the Mastermind workflow CLAUDE.md template into the project root.
        /// Refuses to overwrite an existing CLAUDE.md unless --force is passed.
        #[arg(long)]
        with_claude_md: bool,
        /// Overwrite existing files (CONTEXT.md, CLAUDE.md). Off by default.
        #[arg(long)]
        force: bool,
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

fn default_index_path() -> PathBuf {
    PathBuf::from(".mastermind/mmcg.db")
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
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
                "indexed {} (unchanged {}, purged {}, failed {}) / scanned {} | {} symbols | {} edges | {} ms",
                stats.files_indexed,
                stats.files_unchanged,
                stats.files_purged,
                stats.files_failed,
                stats.files_scanned,
                stats.symbols_total,
                stats.edges_total,
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
        } => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            do_init(&root, with_claude_md, force)?;
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
) -> Result<(), Box<dyn std::error::Error>> {
    let mut created: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

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
            "# Generated by `mmcg init` — local working state, not for commit.\n\
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

    // 3. CONTEXT.md from the template (strip the HTML-comment instructions block)
    let context_path = root.join("CONTEXT.md");
    let context_body = strip_template_comment(CONTEXT_TEMPLATE);
    if write_if_absent(&context_path, &context_body, force)? {
        created.push("CONTEXT.md".into());
    } else {
        skipped.push("CONTEXT.md (already exists — pass --force to overwrite)".into());
    }

    // 4. CLAUDE.md (optional)
    if with_claude_md {
        let claude_path = root.join("CLAUDE.md");
        let claude_body = strip_template_comment(WORKFLOW_TEMPLATE);
        if write_if_absent(&claude_path, &claude_body, force)? {
            created.push("CLAUDE.md".into());
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

    // 6. Initial empty index
    let db_path = mastermind_dir.join("mmcg.db");
    if !db_path.exists() {
        let _ = Store::open(&db_path)?; // creates schema
        created.push(".mastermind/mmcg.db (empty index — run `mmcg index .` to populate)".into());
    } else {
        skipped.push(".mastermind/mmcg.db (already exists)".into());
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
    println!("  1. Run `mmcg index .` to populate the index");
    println!("  2. (Optional) Run `mmcg watch` in another terminal to keep the index fresh");
    println!("  3. Register mmcg with your MCP client (see mcp/servers/mmcg/README.md for config)");
    println!("  4. Add `.mastermind/` to your project's root `.gitignore` (everything under it is local working state)");
    if !with_claude_md {
        println!("  5. Adopt the workflow CLAUDE.md: copy from agents/claude-md/mastermind-workflow.md\n     or re-run `mmcg init --with-claude-md` to drop it in automatically");
    } else {
        println!("  5. Review the dropped CLAUDE.md — it has <PLACEHOLDER> sections to fill in for your project");
    }
    println!(
        "  Note: task specs now live under `.mastermind/tasks/` (was `.tasks/` in pre-0.6.0)."
    );

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
