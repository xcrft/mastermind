//! `mastermind doctor` — environment health-check for setup adoption.
//!
//! Runs fail-soft checks against the project at `root` (CWD by default) and
//! prints a structured report. Checks are independent — one failure does NOT
//! abort the rest. Exit code 0 unless any check returns `Fail`.
//!
//! Checks (in order):
//!
//! | # | Name                  | Catches                                                |
//! |---|-----------------------|--------------------------------------------------------|
//! | 1 | `mmcg binary`          | sanity — we're running, report our version             |
//! | 2 | `index database`       | selected index exists                                  |
//! | 3 | `index repository`     | selected index belongs to the requested repository     |
//! | 4 | `symbols indexed`      | non-empty index (catches "I ran init but not index")   |
//! | 5 | `index freshness`      | no source file newer than the index                    |
//! | 5 | `gitignore`            | `.mastermind/` is excluded from VCS                    |
//! | 6 | `CLAUDE.md`            | exists and references the workflow                     |
//! | 7 | `MCP config`           | mmcg registered in `~/.claude.json` (user) or `./.mcp.json` (project) |
//! | 8 | `MCP serve handshake`  | spawning `mastermind serve` responds to `initialize` + `tools/list` |
//! | 9 | `subagent MCP scoping` | every subagent `mcpServers:` entry names a registered server |
//! | 10 | `style profile`       | author's `~/.mastermind/style.md` has fallen behind their commits |
//!
//! Human-readable by default; `--json` switches to a machine-parseable format.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// Per-check verdict. Order matters — Display picks the marker by severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    /// One-line summary printed next to the marker.
    pub message: String,
    /// Optional second line — suggested remediation when status != Ok.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub root: String,
    pub checks: Vec<Check>,
    pub summary: Summary,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub ok: u32,
    pub warn: u32,
    pub fail: u32,
}

impl Report {
    pub fn from_checks(root: &Path, checks: Vec<Check>) -> Self {
        let summary = Summary {
            ok: checks.iter().filter(|c| c.status == Status::Ok).count() as u32,
            warn: checks.iter().filter(|c| c.status == Status::Warn).count() as u32,
            fail: checks.iter().filter(|c| c.status == Status::Fail).count() as u32,
        };
        Self {
            root: root.display().to_string(),
            checks,
            summary,
        }
    }

    pub fn render_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "mastermind doctor — checking environment at {}\n\n",
            self.root
        ));
        let name_width = self
            .checks
            .iter()
            .map(|c| c.name.chars().count())
            .max()
            .unwrap_or(20);
        for c in &self.checks {
            let marker = match c.status {
                Status::Ok => "✅",
                Status::Warn => "⚠️ ",
                Status::Fail => "❌",
            };
            out.push_str(&format!(
                "  {marker} {name:<width$}  {msg}\n",
                marker = marker,
                name = c.name,
                width = name_width,
                msg = c.message,
            ));
            if let Some(hint) = &c.hint {
                out.push_str(&format!("       → {hint}\n"));
            }
        }
        out.push_str(&format!(
            "\n{} ok, {} warn, {} fail\n",
            self.summary.ok, self.summary.warn, self.summary.fail
        ));
        out
    }

    pub fn has_failures(&self) -> bool {
        self.summary.fail > 0
    }

    pub fn render_explain(&self, binary: &std::path::Path, index_path: &std::path::Path) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "mastermind doctor --explain — environment at {}\n\n",
            self.root
        ));

        out.push_str("Paths:\n");
        out.push_str(&format!("  binary        {}\n", binary.display()));
        out.push_str(&format!("  index         {}\n", index_path.display()));

        let home = std::env::home_dir();
        let user_cfg = home
            .as_ref()
            .map(|h| h.join(".claude.json"))
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(home dir unknown)".into());
        out.push_str(&format!("  ~/.claude.json  {user_cfg}\n"));
        out.push('\n');

        let name_width = self
            .checks
            .iter()
            .map(|c| c.name.chars().count())
            .max()
            .unwrap_or(20);
        for c in &self.checks {
            let marker = match c.status {
                Status::Ok => "✅",
                Status::Warn => "⚠️ ",
                Status::Fail => "❌",
            };
            out.push_str(&format!(
                "  {marker} {name:<width$}  {msg}\n",
                marker = marker,
                name = c.name,
                width = name_width,
                msg = c.message,
            ));
            if let Some(hint) = &c.hint {
                out.push_str(&format!("       → {hint}\n"));
            } else if c.status == Status::Ok {
                out.push_str("       → OK\n");
            }
        }
        out.push_str(&format!(
            "\n{} ok, {} warn, {} fail\n",
            self.summary.ok, self.summary.warn, self.summary.fail
        ));

        if self.summary.fail > 0 || self.summary.warn > 0 {
            out.push_str("\nCommon fixes:\n");
            out.push_str("  No index       run `mastermind init` or `mastermind index .`\n");
            out.push_str("  No MCP config  run `mastermind setup claude --write-mcp`\n");
            out.push_str("  Stale index    run `mastermind index .` or start `mastermind watch`\n");
            out.push_str("  No .gitignore  add `.mastermind/` to your .gitignore\n");
        }
        out
    }
}

/// Run every check against `root`. Returns a structured report.
///
/// `mmcg_binary` is the binary used for the MCP-serve handshake check (usually
/// `std::env::current_exe()`). Passed in so tests can override.
pub fn run(root: &Path, mmcg_binary: &Path) -> Report {
    run_with_index(root, mmcg_binary, &root.join(".mastermind/mmcg.db"))
}

/// Run every check against `root` using the selected index database.
pub fn run_with_index(root: &Path, mmcg_binary: &Path, index_path: &Path) -> Report {
    let checks = vec![
        check_binary(),
        check_path_entries(),
        check_index_db(index_path),
        check_index_root(root, index_path),
        check_symbols_indexed(index_path),
        check_index_freshness(root, index_path),
        check_gitignore(root),
        check_claude_md(root),
        check_mcp_config(root),
        check_mcp_handshake(index_path, mmcg_binary),
        check_subagent_mcp_servers(root),
        check_style_profile(root),
    ];
    Report::from_checks(root, checks)
}

/// Nudge a re-mine when the author's style profile has fallen behind their
/// commits. An absent profile is fine — the feature is opt-in — so it's Ok.
const STYLE_REFRESH_HINT: &str = "refresh with `mastermind miner profile`";

fn check_style_profile(root: &Path) -> Check {
    use crate::miner::profile::Staleness;
    match crate::miner::profile::staleness(root) {
        Staleness::Absent => Check {
            name: "style profile",
            status: Status::Ok,
            message: "none — `mastermind miner profile` to seed (optional)".into(),
            hint: None,
        },
        Staleness::Legacy => Check {
            name: "style profile",
            status: Status::Warn,
            message: "legacy format — refresh removes identity metadata and preserves the new profile contract".into(),
            hint: Some(STYLE_REFRESH_HINT.into()),
        },
        Staleness::Fresh { mined_through } => Check {
            name: "style profile",
            status: Status::Ok,
            message: format!("fresh (mined through {mined_through})"),
            hint: None,
        },
        Staleness::Stale {
            mined_through,
            new_commits,
        } => Check {
            name: "style profile",
            status: Status::Warn,
            message: format!("{new_commits} new commits since last mined ({mined_through})"),
            hint: Some(STYLE_REFRESH_HINT.into()),
        },
    }
}

// ----- individual checks ---------------------------------------------------

fn check_binary() -> Check {
    Check {
        name: "mmcg binary",
        status: Status::Ok,
        message: format!("v{}", env!("CARGO_PKG_VERSION")),
        hint: None,
    }
}

fn check_path_entries() -> Check {
    let path = std::env::var_os("PATH");
    let Some(path) = path else {
        return Check {
            name: "PATH entries",
            status: Status::Fail,
            message: "PATH is not set".into(),
            hint: Some("native client setup cannot resolve a binary without PATH".into()),
        };
    };
    match crate::setup::describe_unsafe_path_entries(&path) {
        None => Check {
            name: "PATH entries",
            status: Status::Ok,
            message: format!("{} absolute entries", std::env::split_paths(&path).count()),
            hint: None,
        },
        Some(detail) => Check {
            name: "PATH entries",
            status: Status::Fail,
            message: detail,
            hint: Some(
                "`mastermind setup <client> --scope user` fails closed with `unsafe_path_entry` until every PATH entry is absolute"
                    .into(),
            ),
        },
    }
}

fn check_index_db(index_path: &Path) -> Check {
    if index_path.is_file() {
        let size = std::fs::metadata(index_path).map(|m| m.len()).unwrap_or(0);
        Check {
            name: "index database",
            status: Status::Ok,
            message: format!("{} ({})", index_path.display(), format_bytes(size)),
            hint: None,
        }
    } else {
        Check {
            name: "index database",
            status: Status::Fail,
            message: format!("{} not found", index_path.display()),
            hint: Some("run `mastermind init` then `mastermind index .`".into()),
        }
    }
}

fn check_index_root(root: &Path, index_path: &Path) -> Check {
    if !index_path.is_file() {
        return Check {
            name: "index repository",
            status: Status::Warn,
            message: "skipped — no index database".into(),
            hint: None,
        };
    }
    match crate::store::Store::open(index_path)
        .map_err(|error| format!("can't open db: {error}"))
        .and_then(|store| crate::indexer::validate_index_root(&store, root))
    {
        Ok(()) => Check {
            name: "index repository",
            status: Status::Ok,
            message: format!("{} belongs to {}", index_path.display(), root.display()),
            hint: None,
        },
        Err(error) => Check {
            name: "index repository",
            status: Status::Fail,
            message: error,
            hint: Some(
                "pass the correct `--index` or rebuild the index for this repository".into(),
            ),
        },
    }
}

fn check_symbols_indexed(index_path: &Path) -> Check {
    if !index_path.is_file() {
        return Check {
            name: "symbols indexed",
            status: Status::Warn,
            message: "skipped — no index database".into(),
            hint: None,
        };
    }
    let store = match crate::store::Store::open(index_path) {
        Ok(s) => s,
        Err(e) => {
            return Check {
                name: "symbols indexed",
                status: Status::Fail,
                message: format!("can't open db: {e}"),
                hint: Some("delete `.mastermind/mmcg.db` and re-run `mastermind index .`".into()),
            };
        }
    };
    let file_count = store.file_count().unwrap_or(0);
    let symbol_count = store.symbol_count().unwrap_or(0);
    if file_count == 0 || symbol_count == 0 {
        Check {
            name: "symbols indexed",
            status: Status::Fail,
            message: format!("{file_count} files, {symbol_count} symbols"),
            hint: Some("index is empty — run `mastermind index .` from the project root".into()),
        }
    } else {
        Check {
            name: "symbols indexed",
            status: Status::Ok,
            message: format!("{symbol_count} symbols across {file_count} files"),
            hint: None,
        }
    }
}

/// Compare each indexable path with its stored source mtime.
/// Stops at 10 hits to keep the doctor fast on large repos.
fn check_index_freshness(root: &Path, index_path: &Path) -> Check {
    if !index_path.is_file() {
        return Check {
            name: "index freshness",
            status: Status::Warn,
            message: "skipped — no index database".into(),
            hint: None,
        };
    }
    if crate::workflow_status::db_extractor_contract_current(index_path) != Some(true) {
        return Check {
            name: "index freshness",
            status: Status::Warn,
            message: "extractor contract changed since this index was built".into(),
            hint: Some("run `mastermind index .` to rebuild structural data".into()),
        };
    }
    let limit = 10usize;
    let Some(stale) = crate::workflow_status::stale_paths(root, index_path, limit) else {
        return Check {
            name: "index freshness",
            status: Status::Warn,
            message: "cannot inspect index freshness".into(),
            hint: Some("run `mastermind index .` to rebuild the index".into()),
        };
    };
    if stale.is_empty() {
        Check {
            name: "index freshness",
            status: Status::Ok,
            message: "indexable source paths match stored mtimes".into(),
            hint: None,
        }
    } else {
        // Show up to 3 by name. At the scan limit we report ≥N — the exact
        // total is unknown without finishing the walk.
        let preview = stale.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        let count_label = if stale.len() >= limit {
            format!("≥{} stale source files", stale.len())
        } else {
            format!("{} stale source file(s)", stale.len())
        };
        Check {
            name: "index freshness",
            status: Status::Warn,
            message: format!("{count_label} (e.g. {preview})"),
            hint: Some(
                "run `mastermind index .` or start `mastermind watch` for live updates".into(),
            ),
        }
    }
}

fn check_gitignore(root: &Path) -> Check {
    let p = root.join(".gitignore");
    if !p.is_file() {
        return Check {
            name: "gitignore",
            status: Status::Warn,
            message: "no .gitignore at project root".into(),
            hint: Some(
                "add `.mastermind/` to .gitignore — the index + specs are local state".into(),
            ),
        };
    }
    let body = std::fs::read_to_string(&p).unwrap_or_default();
    let lines: Vec<&str> = body
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let mentioned = lines.iter().any(|l| {
        *l == ".mastermind" || *l == ".mastermind/" || *l == "/.mastermind" || *l == "/.mastermind/"
    });
    if mentioned {
        Check {
            name: "gitignore",
            status: Status::Ok,
            message: ".mastermind/ excluded".into(),
            hint: None,
        }
    } else {
        Check {
            name: "gitignore",
            status: Status::Warn,
            message: ".mastermind/ NOT in .gitignore".into(),
            hint: Some(
                "add `.mastermind/` to .gitignore — leaking the local index pollutes commits"
                    .into(),
            ),
        }
    }
}

fn check_claude_md(root: &Path) -> Check {
    let p = root.join("CLAUDE.md");
    if !p.is_file() {
        return Check {
            name: "CLAUDE.md",
            status: Status::Warn,
            message: "not present at project root".into(),
            hint: Some(
                "drop in a workflow template — see `agents/claude-md/mastermind-workflow.md`"
                    .into(),
            ),
        };
    }
    let body = std::fs::read_to_string(&p).unwrap_or_default();
    // Canonical markers our templates use. Match any of them so we accept
    // hand-customized variants that mention the workflow by name.
    const MARKERS: &[&str] = &[
        "mastermind-workflow",
        "mastermind-task-planning",
        "mastermind-task-executor",
        ".mastermind/tasks/",
    ];
    let found = MARKERS.iter().any(|m| body.contains(m));
    if found {
        Check {
            name: "CLAUDE.md",
            status: Status::Ok,
            message: "present + references the mastermind workflow".into(),
            hint: None,
        }
    } else {
        Check {
            name: "CLAUDE.md",
            status: Status::Warn,
            message: "present but no mastermind workflow markers".into(),
            hint: Some(
                "append the workflow section — see `agents/claude-md/mastermind-workflow.md`"
                    .into(),
            ),
        }
    }
}

fn check_mcp_config(root: &Path) -> Check {
    let trusted_binary = match std::env::current_exe() {
        Ok(binary) => binary,
        Err(_) => {
            return Check {
                name: "MCP config",
                status: Status::Warn,
                message: "trusted-binary=unavailable".into(),
                hint: Some("rerun `mastermind doctor` from the installed binary".into()),
            }
        }
    };
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    check_mcp_config_at(
        &canonical_root,
        std::env::home_dir().as_deref(),
        &crate::setup::canonical_entry(&trusted_binary),
    )
}

fn check_mcp_config_at(root: &Path, home: Option<&Path>, canonical: &serde_json::Value) -> Check {
    let mut statuses = Vec::new();
    let mut canonical_found = false;
    let mut config_found = false;

    let mut json_candidates = vec![
        ("claude-project", root.join(".mcp.json")),
        ("cursor-project", root.join(".cursor/mcp.json")),
    ];
    if let Some(home) = home {
        json_candidates.push(("claude-user", home.join(".claude.json")));
        json_candidates.push(("cursor-user", home.join(".cursor/mcp.json")));
    }
    for (label, path) in json_candidates {
        let present = std::fs::symlink_metadata(&path).is_ok();
        if present {
            config_found = true;
        }
        let status = match crate::setup::read_json_mmcg(&path) {
            Ok(Some(entry)) if entry == *canonical => {
                canonical_found = true;
                "canonical"
            }
            Ok(Some(_)) => "customized",
            Ok(None) if present => "missing",
            Ok(None) => "absent",
            Err(class) => doctor_config_error_class(&class),
        };
        statuses.push(format!("{label}={status}"));
    }

    let continue_shape = crate::setup::continue_entry(canonical);
    let mut continue_candidates = vec![(
        "continue-project",
        root.join(".continue/mcpServers/mastermind.yaml"),
    )];
    if let Some(home) = home {
        continue_candidates.push((
            "continue-user",
            home.join(".continue/mcpServers/mastermind.yaml"),
        ));
    }
    for (label, path) in continue_candidates {
        let present = std::fs::symlink_metadata(&path).is_ok();
        if present {
            config_found = true;
        }
        let status = match doctor_read_capped(&path).and_then(|bytes| {
            bytes
                .map(|bytes| {
                    std::str::from_utf8(&bytes)
                        .map_err(|_| "invalid_encoding".to_string())
                        .and_then(|body| {
                            serde_norway::from_str::<serde_json::Value>(body)
                                .map_err(|_| "invalid_yaml".to_string())
                        })
                })
                .transpose()
        }) {
            Ok(Some(value)) if value == continue_shape => {
                canonical_found = true;
                "canonical"
            }
            Ok(Some(_)) => "customized",
            Ok(None) => "absent",
            Err(class) => doctor_config_error_class(&class),
        };
        statuses.push(format!("{label}={status}"));
    }

    if let Some(home) = home {
        let path = home.join(".codex/config.toml");
        let present = std::fs::symlink_metadata(&path).is_ok();
        if present {
            config_found = true;
        }
        let status = match doctor_read_capped(&path).and_then(|bytes| {
            bytes
                .map(|bytes| parse_codex_mmcg(&bytes))
                .transpose()
                .map(Option::flatten)
        }) {
            Ok(Some(entry)) if entry == *canonical => {
                canonical_found = true;
                "canonical"
            }
            Ok(Some(_)) => "customized",
            Ok(None) if present => "missing",
            Ok(None) => "absent",
            Err(class) => doctor_config_error_class(&class),
        };
        statuses.push(format!("codex-user={status}"));
    }

    Check {
        name: "MCP config",
        status: if canonical_found {
            Status::Ok
        } else {
            Status::Warn
        },
        message: statuses.join(", "),
        hint: if canonical_found {
            None
        } else if config_found {
            Some("run `mastermind setup <client>` to preview a canonical repair".into())
        } else {
            Some("run `mastermind setup <client>` to preview registration".into())
        },
    }
}

fn doctor_read_capped(path: &Path) -> Result<Option<Vec<u8>>, String> {
    crate::setup::read_config_capped(path)
}

fn doctor_config_error_class(class: &str) -> &'static str {
    match class {
        "parent_traversal_rejected"
        | "symlink_target_rejected"
        | "target_inspection_failed"
        | "config_not_regular" => "unsafe_path",
        "config_too_large" => "too_large",
        _ => "malformed",
    }
}

fn parse_codex_mmcg(bytes: &[u8]) -> Result<Option<serde_json::Value>, String> {
    let body = std::str::from_utf8(bytes).map_err(|_| "invalid_encoding".to_string())?;
    let value: toml::Value = toml::from_str(body).map_err(|_| "invalid_toml".to_string())?;
    let root = value
        .as_table()
        .ok_or_else(|| "invalid_toml_shape".to_string())?;
    let Some(servers) = root.get("mcp_servers") else {
        return Ok(None);
    };
    let servers = servers
        .as_table()
        .ok_or_else(|| "invalid_toml_shape".to_string())?;
    let Some(mmcg) = servers.get("mmcg") else {
        return Ok(None);
    };
    let mmcg = mmcg
        .as_table()
        .ok_or_else(|| "invalid_toml_shape".to_string())?;
    let command = mmcg
        .get("command")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "invalid_codex_command".to_string())?;
    let args = mmcg
        .get("args")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "invalid_codex_args".to_string())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "invalid_codex_args".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(serde_json::json!({
        "command": command,
        "args": args,
    })))
}

/// Spawn `mmcg --index <db> serve`, write `initialize` + `tools/list`, read
/// the responses, count tools. 3-second budget. Fails if the binary is missing
/// OR the handshake fails.
fn check_mcp_handshake(index_path: &Path, binary: &Path) -> Check {
    if !index_path.is_file() {
        return Check {
            name: "MCP handshake",
            status: Status::Warn,
            message: "skipped — no index database to serve".into(),
            hint: None,
        };
    }

    let mut child = match std::process::Command::new(binary)
        .arg("--index")
        .arg(index_path)
        .arg("serve")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return Check {
                name: "MCP handshake",
                status: Status::Fail,
                message: format!("could not spawn `{}`: {e}", binary.display()),
                hint: Some("ensure the mmcg binary is on PATH".into()),
            };
        }
    };

    let result = perform_handshake(&mut child);
    // Always kill — `mastermind serve` reads stdin forever otherwise.
    let _ = child.kill();
    let _ = child.wait();

    match result {
        Ok(tool_count) => Check {
            name: "MCP handshake",
            status: Status::Ok,
            message: format!(
                "`mastermind serve` responded to initialize + tools/list ({tool_count} tools)"
            ),
            hint: None,
        },
        Err(e) => Check {
            name: "MCP handshake",
            status: Status::Fail,
            message: format!("handshake failed: {e}"),
            hint: Some(
                "re-run `mastermind index .` and try again; report bug if it persists".into(),
            ),
        },
    }
}

fn doctor_initialize_request() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "mastermind-doctor", "version": env!("CARGO_PKG_VERSION") }
        }
    })
}

fn doctor_ready_requests() -> (serde_json::Value, serde_json::Value) {
    (
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
    )
}

fn perform_handshake(child: &mut std::process::Child) -> Result<usize, String> {
    use std::io::{BufRead, BufReader, Write};

    let stdin = child.stdin.as_mut().ok_or("no stdin pipe")?;
    let initialize = doctor_initialize_request();
    writeln!(stdin, "{initialize}").map_err(|e| format!("write initialize: {e}"))?;
    stdin
        .flush()
        .map_err(|e| format!("flush initialize: {e}"))?;

    let stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let mut reader = BufReader::new(stdout);
    let (tx, rx) = std::sync::mpsc::channel::<Result<Option<usize>, String>>();
    std::thread::spawn(move || {
        let mut line = String::new();
        if let Err(e) = reader.read_line(&mut line) {
            let _ = tx.send(Err(format!("read initialize: {e}")));
            return;
        }
        if line.trim().is_empty() {
            let _ = tx.send(Err("empty initialize response".into()));
            return;
        }
        let v: serde_json::Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.send(Err(format!("parse initialize: {e}")));
                return;
            }
        };
        if v.get("error").is_some() {
            let _ = tx.send(Err(format!("initialize error: {}", v["error"])));
            return;
        }
        if v.get("result")
            .and_then(|result| result.get("protocolVersion"))
            .and_then(serde_json::Value::as_str)
            != Some("2025-11-25")
        {
            let _ = tx.send(Err("unexpected initialize protocol version".into()));
            return;
        }
        if tx.send(Ok(None)).is_err() {
            return;
        }
        line.clear();
        if let Err(e) = reader.read_line(&mut line) {
            let _ = tx.send(Err(format!("read tools/list: {e}")));
            return;
        }
        let v: serde_json::Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.send(Err(format!("parse tools/list: {e}")));
                return;
            }
        };
        let tools = v
            .get("result")
            .and_then(|r| r.get("tools"))
            .and_then(|t| t.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let _ = tx.send(Ok(Some(tools)));
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    match rx
        .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        .map_err(|_| "timeout waiting for MCP server response (3s)".to_string())??
    {
        None => {}
        Some(_) => return Err("unexpected tools response before initialized".into()),
    }

    let (initialized, tools_list) = doctor_ready_requests();
    writeln!(stdin, "{initialized}").map_err(|e| format!("write initialized: {e}"))?;
    writeln!(stdin, "{tools_list}").map_err(|e| format!("write tools/list: {e}"))?;
    stdin
        .flush()
        .map_err(|e| format!("flush tools/list: {e}"))?;

    match rx
        .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        .map_err(|_| "timeout waiting for MCP server response (3s)".to_string())??
    {
        Some(tool_count) => Ok(tool_count),
        None => Err("missing tools response".into()),
    }
}

// ----- helpers -------------------------------------------------------------

fn format_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if n >= GB {
        format!("{:.1} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.1} KB", n as f64 / KB as f64)
    } else {
        format!("{n} B")
    }
}

/// Server names registered for Claude Code: keys under `mcpServers` (or legacy
/// `servers`) in project `.mcp.json` and user `~/.claude.json`.
fn registered_servers(root: &Path) -> std::collections::BTreeSet<String> {
    let mut set = std::collections::BTreeSet::new();
    let candidates: Vec<PathBuf> = std::iter::once(root.join(".mcp.json"))
        .chain(std::env::home_dir().map(|h| h.join(".claude.json")))
        .collect();
    for path in candidates {
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
            continue;
        };
        for key in ["mcpServers", "servers"] {
            if let Some(map) = v.get(key).and_then(|m| m.as_object()) {
                set.extend(map.keys().cloned());
            }
        }
    }
    set
}

/// YAML frontmatter block between the opening `---` and the next `---`.
/// `None` if the text doesn't open with frontmatter.
fn frontmatter_block(md: &str) -> Option<&str> {
    let rest = md
        .strip_prefix("---\n")
        .or_else(|| md.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

/// MCP server names a subagent references in its top-level `mcpServers:` field —
/// list entries or mapping keys (inline definitions). Empty if the field is
/// absent or the frontmatter won't parse.
fn subagent_mcp_refs(md: &str) -> Vec<String> {
    let Some(fm) = frontmatter_block(md) else {
        return vec![];
    };
    let Ok(v) = serde_norway::from_str::<serde_norway::Value>(fm) else {
        return vec![];
    };
    match v.get("mcpServers") {
        Some(serde_norway::Value::Sequence(seq)) => seq
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect(),
        Some(serde_norway::Value::Mapping(map)) => map
            .iter()
            .filter_map(|(k, _)| k.as_str().map(String::from))
            .collect(),
        _ => vec![],
    }
}

/// Pure core of `check_subagent_mcp_servers`: scan `agent_dirs` for subagent
/// `.md` files; return `(any_declared, sorted unregistered "server (in file)"
/// descriptions)`. Split out so tests use a controlled directory, not the real
/// `~/.claude/agents`.
fn unregistered_subagent_servers(
    agent_dirs: &[PathBuf],
    registered: &std::collections::BTreeSet<String>,
) -> (bool, Vec<String>) {
    let mut declared = false;
    let mut missing: Vec<String> = Vec::new();
    for dir in agent_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for ent in entries.flatten() {
            let p = ent.path();
            if p.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let body = std::fs::read_to_string(&p).unwrap_or_default();
            for server in subagent_mcp_refs(&body) {
                declared = true;
                if !registered.contains(&server) {
                    let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    missing.push(format!("{server} (in {fname})"));
                }
            }
        }
    }
    missing.sort();
    missing.dedup();
    (declared, missing)
}

/// Every server a subagent scopes via `mcpServers:` must be registered —
/// otherwise the subagent silently gets nothing for that entry. Scans project
/// `.claude/agents/` and user `~/.claude/agents/`.
fn check_subagent_mcp_servers(root: &Path) -> Check {
    let registered = registered_servers(root);
    let mut agent_dirs: Vec<PathBuf> = vec![root.join(".claude").join("agents")];
    if let Some(h) = std::env::home_dir() {
        agent_dirs.push(h.join(".claude").join("agents"));
    }
    let (declared, missing) = unregistered_subagent_servers(&agent_dirs, &registered);

    if !declared {
        return Check {
            name: "subagent MCP scoping",
            status: Status::Ok,
            message: "no subagent declares `mcpServers:` — nothing to verify".into(),
            hint: None,
        };
    }
    if missing.is_empty() {
        return Check {
            name: "subagent MCP scoping",
            status: Status::Ok,
            message: "every subagent `mcpServers:` entry names a registered server".into(),
            hint: None,
        };
    }
    Check {
        name: "subagent MCP scoping",
        status: Status::Warn,
        message: format!(
            "subagent scopes an unregistered MCP server: {}",
            missing.join(", ")
        ),
        hint: Some(
            "register it (project `.mcp.json` / `mastermind setup claude --write-mcp`) or drop the `mcpServers:` entry"
                .into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        // Process-global monotonic counter. `process::id()` is identical across
        // cargo's parallel test threads, and the nanosecond clock can collide
        // when two threads land in the same bucket — the root cause of the old
        // `check_gitignore` flake, where one test's `remove_dir_all` wiped
        // another's working dir mid-run. `fetch_add` hands every call a distinct
        // value, so no two `tmp()` invocations resolve to the same directory.
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "mmcg-doctor-{}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            rand_suffix()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn rand_suffix() -> String {
        use std::time::SystemTime;
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos().to_string())
            .unwrap_or_default()
    }

    #[test]
    fn mcp_handshake_sequence_uses_current_revision() {
        let initialize = doctor_initialize_request();
        assert_eq!(initialize["method"], "initialize");
        assert_eq!(initialize["params"]["protocolVersion"], "2025-11-25");
        assert!(initialize["params"]["capabilities"].is_object());
        assert_eq!(
            initialize["params"]["clientInfo"]["name"],
            "mastermind-doctor"
        );

        let (initialized, tools_list) = doctor_ready_requests();
        assert_eq!(initialized["method"], "notifications/initialized");
        assert!(initialized.get("id").is_none());
        assert_eq!(tools_list["method"], "tools/list");
        assert_eq!(tools_list["id"], 2);
    }

    #[test]
    fn check_index_db_fails_when_missing() {
        let root = tmp();
        let c = check_index_db(&root.join("custom.db"));
        assert_eq!(c.status, Status::Fail);
        assert!(c.hint.as_deref().unwrap().contains("mastermind init"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn check_index_db_ok_when_present() {
        let root = tmp();
        fs::create_dir_all(root.join(".mastermind")).unwrap();
        fs::write(root.join(".mastermind/mmcg.db"), b"junk").unwrap();
        let c = check_index_db(&root.join(".mastermind/mmcg.db"));
        assert_eq!(c.status, Status::Ok);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn check_index_root_rejects_a_database_from_another_repository() {
        let requested = tmp().canonicalize().unwrap();
        let indexed = tmp().canonicalize().unwrap();
        let db = indexed.join("custom.db");
        let store = crate::store::Store::open(&db).unwrap();
        store
            .set_meta("index_root", &indexed.to_string_lossy())
            .unwrap();
        drop(store);

        let check = check_index_root(&requested, &db);
        assert_eq!(check.status, Status::Fail);
        assert!(check.message.contains("index belongs to"));
        assert!(check.message.contains(&requested.display().to_string()));
        fs::remove_dir_all(requested).ok();
        fs::remove_dir_all(indexed).ok();
    }

    #[test]
    fn check_gitignore_warns_when_missing_or_unset() {
        let root = tmp();
        // No .gitignore.
        assert_eq!(check_gitignore(&root).status, Status::Warn);
        // .gitignore without .mastermind.
        fs::write(root.join(".gitignore"), "node_modules\n").unwrap();
        assert_eq!(check_gitignore(&root).status, Status::Warn);
        // With .mastermind.
        fs::write(root.join(".gitignore"), ".mastermind/\n").unwrap();
        assert_eq!(check_gitignore(&root).status, Status::Ok);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn check_claude_md_detects_workflow_markers() {
        let root = tmp();
        assert_eq!(
            check_claude_md(&root).status,
            Status::Warn,
            "missing file warns"
        );
        fs::write(root.join("CLAUDE.md"), "# Plain readme\n").unwrap();
        assert_eq!(
            check_claude_md(&root).status,
            Status::Warn,
            "no workflow marker warns"
        );
        fs::write(
            root.join("CLAUDE.md"),
            "# project\nUses mastermind-workflow.\n",
        )
        .unwrap();
        assert_eq!(check_claude_md(&root).status, Status::Ok);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn style_refresh_preserves_other_repo_contributions() {
        assert!(STYLE_REFRESH_HINT.contains("miner profile"));
        assert!(!STYLE_REFRESH_HINT.contains("--force"));
    }

    #[test]
    fn check_mcp_config_finds_local_or_home() {
        let root = tmp();
        // No config anywhere → warn. Can't unset HOME safely, so only assert
        // the local-config branch.
        let c = check_mcp_config(&root);
        // Ok (if the user has a real ~/.claude/.mcp.json with mmcg) or Warn —
        // tolerate both; the real assertion is "doesn't crash".
        assert!(matches!(c.status, Status::Ok | Status::Warn));
        // Project-local config — should bump to Ok.
        fs::write(
            root.join(".mcp.json"),
            serde_json::to_vec(&serde_json::json!({
                "mcpServers": {
                    "mmcg": crate::setup::canonical_entry(&std::env::current_exe().unwrap())
                }
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(check_mcp_config(&root).status, Status::Ok);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn check_mcp_config_recognizes_each_supported_data_shape() {
        let canonical = serde_json::json!({
            "command": "/trusted/mmcg",
            "args": ["serve"],
        });
        for label in [
            "claude-project",
            "claude-user",
            "cursor-project",
            "cursor-user",
            "continue-project",
            "continue-user",
            "codex-user",
        ] {
            let root = tmp().canonicalize().unwrap();
            let home = tmp().canonicalize().unwrap();
            let path = match label {
                "claude-project" => root.join(".mcp.json"),
                "claude-user" => home.join(".claude.json"),
                "cursor-project" => root.join(".cursor/mcp.json"),
                "cursor-user" => home.join(".cursor/mcp.json"),
                "continue-project" => root.join(".continue/mcpServers/mastermind.yaml"),
                "continue-user" => home.join(".continue/mcpServers/mastermind.yaml"),
                "codex-user" => home.join(".codex/config.toml"),
                _ => unreachable!(),
            };
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            if label.starts_with("continue") {
                let shape = crate::setup::continue_entry(&canonical);
                fs::write(&path, serde_norway::to_string(&shape).unwrap()).unwrap();
            } else if label == "codex-user" {
                fs::write(
                    &path,
                    "[mcp_servers.mmcg]\ncommand = \"/trusted/mmcg\"\nargs = [\"serve\"]\n",
                )
                .unwrap();
            } else {
                fs::write(
                    &path,
                    serde_json::to_vec(&serde_json::json!({
                        "mcpServers": {"mmcg": canonical.clone()}
                    }))
                    .unwrap(),
                )
                .unwrap();
            }
            let check = check_mcp_config_at(&root, Some(&home), &canonical);
            assert_eq!(check.status, Status::Ok, "{label}: {}", check.message);
            assert!(check.message.contains(&format!("{label}=canonical")));
            fs::remove_dir_all(root).ok();
            fs::remove_dir_all(home).ok();
        }
    }

    #[test]
    fn check_mcp_config_never_executes_configured_command() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let root = tmp().canonicalize().unwrap();
            let home = tmp().canonicalize().unwrap();
            let marker = root.join("executed");
            let sentinel = root.join("configured-command");
            fs::write(&sentinel, "#!/bin/sh\n: > \"$1\"\n").unwrap();
            fs::set_permissions(&sentinel, fs::Permissions::from_mode(0o700)).unwrap();
            let command = sentinel.to_string_lossy().into_owned();
            let marker_arg = marker.to_string_lossy().into_owned();
            let configured = serde_json::json!({
                "command": command,
                "args": [marker_arg],
            });

            for path in [
                root.join(".mcp.json"),
                root.join(".cursor/mcp.json"),
                home.join(".claude.json"),
                home.join(".cursor/mcp.json"),
            ] {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(
                    path,
                    serde_json::to_vec(&serde_json::json!({
                        "mcpServers": {"mmcg": configured.clone()}
                    }))
                    .unwrap(),
                )
                .unwrap();
            }
            let continue_shape = crate::setup::continue_entry(&configured);
            for path in [
                root.join(".continue/mcpServers/mastermind.yaml"),
                home.join(".continue/mcpServers/mastermind.yaml"),
            ] {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, serde_norway::to_string(&continue_shape).unwrap()).unwrap();
            }
            let codex = home.join(".codex/config.toml");
            fs::create_dir_all(codex.parent().unwrap()).unwrap();
            fs::write(
                codex,
                format!("[mcp_servers.mmcg]\ncommand = {command:?}\nargs = [{marker_arg:?}]\n"),
            )
            .unwrap();

            let canonical = serde_json::json!({"command": "/trusted/mmcg", "args": ["serve"]});
            let check = check_mcp_config_at(&root, Some(&home), &canonical);
            assert_eq!(check.status, Status::Warn);
            for label in [
                "claude-project=customized",
                "claude-user=customized",
                "cursor-project=customized",
                "cursor-user=customized",
                "continue-project=customized",
                "continue-user=customized",
                "codex-user=customized",
            ] {
                assert!(check.message.contains(label));
            }
            assert!(!marker.exists());
            fs::remove_dir_all(root).ok();
            fs::remove_dir_all(home).ok();
        }
    }

    #[test]
    fn check_mcp_config_rejects_symlinked_continue_and_codex_parents() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let root = tmp().canonicalize().unwrap();
            let home = tmp().canonicalize().unwrap();
            let continue_real = root.join("continue-real/mcpServers");
            let codex_real = home.join("codex-real");
            fs::create_dir_all(&continue_real).unwrap();
            fs::create_dir_all(&codex_real).unwrap();
            fs::write(
                continue_real.join("mastermind.yaml"),
                serde_norway::to_string(&crate::setup::continue_entry(
                    &serde_json::json!({"command": "/trusted/mmcg", "args": ["serve"]}),
                ))
                .unwrap(),
            )
            .unwrap();
            fs::write(
                codex_real.join("config.toml"),
                "[mcp_servers.mmcg]\ncommand = '/trusted/mmcg'\nargs = ['serve']\n",
            )
            .unwrap();
            symlink(root.join("continue-real"), root.join(".continue")).unwrap();
            symlink(&codex_real, home.join(".codex")).unwrap();

            let canonical = serde_json::json!({"command": "/trusted/mmcg", "args": ["serve"]});
            let check = check_mcp_config_at(&root, Some(&home), &canonical);
            assert_eq!(check.status, Status::Warn);
            assert!(check.message.contains("continue-project=unsafe_path"));
            assert!(check.message.contains("codex-user=unsafe_path"));
            fs::remove_dir_all(root).ok();
            fs::remove_dir_all(home).ok();
        }
    }

    #[test]
    fn check_mcp_config_accepts_valid_codex_toml_variants() {
        let canonical = serde_json::json!({"command": "/trusted/mmcg", "args": ["serve"]});
        let variants = [
            "# before\n[mcp_servers.mmcg]\ncommand = '/trusted/mmcg'\nargs = [\n  'serve', # inline\n]\n[unrelated]\nenabled = true\n",
            "[unrelated]\nvalue = 1\n\n[mcp_servers.mmcg] # table comment\ncommand = \"/trusted/mmcg\"\nargs = [\"serve\"]\n",
        ];
        for body in variants {
            let root = tmp().canonicalize().unwrap();
            let home = tmp().canonicalize().unwrap();
            let codex = home.join(".codex/config.toml");
            fs::create_dir_all(codex.parent().unwrap()).unwrap();
            fs::write(codex, body).unwrap();
            let check = check_mcp_config_at(&root, Some(&home), &canonical);
            assert_eq!(check.status, Status::Ok, "{}", check.message);
            assert!(check.message.contains("codex-user=canonical"));
            fs::remove_dir_all(root).ok();
            fs::remove_dir_all(home).ok();
        }
    }

    #[test]
    fn check_mcp_config_rejects_duplicate_or_wrong_typed_codex_toml() {
        let canonical = serde_json::json!({"command": "/trusted/mmcg", "args": ["serve"]});
        let invalid = [
            "[mcp_servers.mmcg]\ncommand = '/trusted/mmcg'\ncommand = '/other'\nargs = ['serve']\n",
            "[mcp_servers.mmcg]\ncommand = 42\nargs = ['serve']\n",
            "[mcp_servers.mmcg]\ncommand = '/trusted/mmcg'\nargs = ['serve', 42]\n",
        ];
        for body in invalid {
            let root = tmp().canonicalize().unwrap();
            let home = tmp().canonicalize().unwrap();
            let codex = home.join(".codex/config.toml");
            fs::create_dir_all(codex.parent().unwrap()).unwrap();
            fs::write(codex, body).unwrap();
            let check = check_mcp_config_at(&root, Some(&home), &canonical);
            assert_eq!(check.status, Status::Warn);
            assert!(check.message.contains("codex-user=malformed"));
            fs::remove_dir_all(root).ok();
            fs::remove_dir_all(home).ok();
        }
    }

    #[test]
    fn check_mcp_config_does_not_render_secret_values() {
        let root = tmp().canonicalize().unwrap();
        let home = tmp().canonicalize().unwrap();
        let secret = "doctor-secret-value";
        fs::write(
            root.join(".mcp.json"),
            serde_json::to_vec(&serde_json::json!({
                "mcpServers": {
                    "mmcg": {
                        "command": "/custom/mmcg",
                        "args": ["serve"],
                        "env": {"TOKEN": secret}
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let canonical = serde_json::json!({"command": "/trusted/mmcg", "args": ["serve"]});
        let check = check_mcp_config_at(&root, Some(&home), &canonical);
        let rendered = format!("{} {}", check.message, check.hint.unwrap_or_default());
        assert!(!rendered.contains(secret));
        assert!(rendered.contains("claude-project=customized"));
        fs::remove_dir_all(root).ok();
        fs::remove_dir_all(home).ok();
    }

    #[test]
    fn report_renders_human_text_with_summary() {
        let root = PathBuf::from("/tmp/test");
        let report = Report::from_checks(
            &root,
            vec![Check {
                name: "binary",
                status: Status::Ok,
                message: "v0.14.0".into(),
                hint: None,
            }],
        );
        let txt = report.render_text();
        assert!(txt.contains("mastermind doctor"));
        assert!(txt.contains("v0.14.0"));
        assert!(txt.contains("1 ok, 0 warn, 0 fail"));
        assert!(!report.has_failures());
    }

    #[test]
    fn subagent_mcp_refs_parses_list_and_handles_absence() {
        let list = "---\nname: r\ndescription: d\nmcpServers: [mmcg, foo]\n---\nbody";
        assert_eq!(
            subagent_mcp_refs(list),
            vec!["mmcg".to_string(), "foo".to_string()]
        );
        let none = "---\nname: r\ndescription: d\ntools: Read\n---\nbody";
        assert!(subagent_mcp_refs(none).is_empty());
        let no_fm = "# heading only\n";
        assert!(subagent_mcp_refs(no_fm).is_empty());
    }

    #[test]
    fn unregistered_subagent_servers_flags_missing_then_clears() {
        let root = tmp();
        let agents = root.join("agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("r.md"),
            "---\nname: r\ndescription: d\nmcpServers: [mmcg]\nmetadata:\n  version: 0.1.0\n---\nb",
        )
        .unwrap();
        fs::write(
            agents.join("p.md"),
            "---\nname: p\ndescription: d\ntools: Read\nmetadata:\n  version: 0.1.0\n---\nb",
        )
        .unwrap();

        let mut reg = std::collections::BTreeSet::new();
        let (declared, missing) =
            unregistered_subagent_servers(std::slice::from_ref(&agents), &reg);
        assert!(declared, "r.md declares mcpServers");
        assert_eq!(missing, vec!["mmcg (in r.md)".to_string()]);

        reg.insert("mmcg".to_string());
        let (declared2, missing2) =
            unregistered_subagent_servers(std::slice::from_ref(&agents), &reg);
        assert!(declared2);
        assert!(missing2.is_empty(), "mmcg now registered");

        fs::remove_dir_all(&root).ok();
    }
}
