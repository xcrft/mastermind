//! `mastermind doctor` — environment health-check for setup adoption.
//!
//! Runs a fixed set of fail-soft checks against the project at `root` (CWD by
//! default) and prints a structured report. Each check is independent; one
//! failing check does NOT abort the rest. Exit code is 0 unless any check
//! returns `Fail`.
//!
//! Checks (in order):
//!
//! | # | Name                  | Catches                                                |
//! |---|-----------------------|--------------------------------------------------------|
//! | 1 | `mmcg binary`          | sanity — we're running, report our version             |
//! | 2 | `index database`       | `.mastermind/mmcg.db` exists                           |
//! | 3 | `symbols indexed`      | non-empty index (catches "I ran init but not index")   |
//! | 4 | `index freshness`      | no source file newer than the index                    |
//! | 5 | `gitignore`            | `.mastermind/` is excluded from VCS                    |
//! | 6 | `CLAUDE.md`            | exists and references the workflow                     |
//! | 7 | `MCP config`           | mmcg registered in `~/.claude.json` (user) or `./.mcp.json` (project) |
//! | 8 | `MCP serve handshake`  | spawning `mastermind serve` responds to `initialize` + `tools/list` |
//!
//! Output is human-readable by default. `--json` switches to a machine-
//! parseable format for piping into other tools.

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

        let home = dirs::home_dir();
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
/// `mmcg_binary` is the path to the `mmcg` binary used for the MCP-serve
/// handshake check. Usually `std::env::current_exe()`. Pass it in so tests
/// can override.
pub fn run(root: &Path, mmcg_binary: &Path) -> Report {
    let checks = vec![
        check_binary(),
        check_index_db(root),
        check_symbols_indexed(root),
        check_index_freshness(root),
        check_gitignore(root),
        check_claude_md(root),
        check_mcp_config(root),
        check_mcp_handshake(root, mmcg_binary),
    ];
    Report::from_checks(root, checks)
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

fn db_path(root: &Path) -> PathBuf {
    root.join(".mastermind").join("mmcg.db")
}

fn check_index_db(root: &Path) -> Check {
    let p = db_path(root);
    if p.is_file() {
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        Check {
            name: "index database",
            status: Status::Ok,
            message: format!(".mastermind/mmcg.db ({})", format_bytes(size)),
            hint: None,
        }
    } else {
        Check {
            name: "index database",
            status: Status::Fail,
            message: ".mastermind/mmcg.db not found".into(),
            hint: Some("run `mastermind init` then `mastermind index .`".into()),
        }
    }
}

fn check_symbols_indexed(root: &Path) -> Check {
    let p = db_path(root);
    if !p.is_file() {
        return Check {
            name: "symbols indexed",
            status: Status::Warn,
            message: "skipped — no index database".into(),
            hint: None,
        };
    }
    let store = match crate::store::Store::open(&p) {
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
    // Cheap counts via existing helpers.
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

/// Walk the project for any extension we can index and check if any file's
/// mtime is newer than the database mtime. Stops at the first 10 hits to keep
/// the doctor fast on large repos.
fn check_index_freshness(root: &Path) -> Check {
    let p = db_path(root);
    if !p.is_file() {
        return Check {
            name: "index freshness",
            status: Status::Warn,
            message: "skipped — no index database".into(),
            hint: None,
        };
    }
    let db_mtime = match std::fs::metadata(&p).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => {
            return Check {
                name: "index freshness",
                status: Status::Warn,
                message: "cannot stat db mtime".into(),
                hint: None,
            };
        }
    };

    let mut stale: Vec<String> = Vec::new();
    let limit = 10usize;
    'walk: for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_skipped_dir(e.file_name().to_str().unwrap_or("")))
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if crate::indexer::extractor_for_path(path).is_none() {
            continue;
        }
        let fs_mtime = match path.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if fs_mtime > db_mtime {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string();
            stale.push(rel);
            if stale.len() >= limit {
                break 'walk;
            }
        }
    }
    if stale.is_empty() {
        Check {
            name: "index freshness",
            status: Status::Ok,
            message: "no source files newer than the index".into(),
            hint: None,
        }
    } else {
        // Show up to 3 by name. If we hit the scan limit, total is ≥10 with
        // "or more" — we can't know the exact total without finishing the walk.
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

/// Look for `mmcg` registered in known MCP config locations. We don't try
/// every editor — just the two locations Claude Code uses today.
fn check_mcp_config(root: &Path) -> Check {
    // The two locations Claude Code actually reads: the project `.mcp.json`
    // (project scope) and `~/.claude.json` top-level `mcpServers` (user scope,
    // written by `claude mcp add --scope user`). NOT `~/.claude/.mcp.json`, which
    // Claude Code ignores.
    let candidates: Vec<(PathBuf, &'static str)> =
        std::iter::once((root.join(".mcp.json"), "project .mcp.json"))
            .chain(
                dirs::home_dir().map(|h| (h.join(".claude.json"), "~/.claude.json (user scope)")),
            )
            .collect();

    for (path, label) in &candidates {
        if !path.is_file() {
            continue;
        }
        let body = std::fs::read_to_string(path).unwrap_or_default();
        let v: serde_json::Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(_) => continue,
        };
        // Two possible shapes: `{"mcpServers": {"mmcg": {...}}}` (Claude Code)
        // or `{"servers": {...}}` (some VS Code extensions).
        let has_mmcg = v.get("mcpServers").and_then(|m| m.get("mmcg")).is_some()
            || v.get("servers").and_then(|m| m.get("mmcg")).is_some();
        if has_mmcg {
            return Check {
                name: "MCP config",
                status: Status::Ok,
                message: format!("registered in {label}"),
                hint: None,
            };
        }
    }

    Check {
        name: "MCP config",
        status: Status::Warn,
        message: "mmcg not registered (project `.mcp.json` or `~/.claude.json` user scope)".into(),
        hint: Some("run `mastermind setup claude --write-mcp`".into()),
    }
}

/// Spawn `mmcg --index <db> serve`, write `initialize` + `tools/list`, read
/// back the responses, count tools. Tight 3-second budget. If the binary
/// can't be found OR the protocol handshake fails, fall through with a Fail.
fn check_mcp_handshake(root: &Path, binary: &Path) -> Check {
    let db = db_path(root);
    if !db.is_file() {
        return Check {
            name: "MCP handshake",
            status: Status::Warn,
            message: "skipped — no index database to serve".into(),
            hint: None,
        };
    }

    let mut child = match std::process::Command::new(binary)
        .arg("--index")
        .arg(&db)
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

fn perform_handshake(child: &mut std::process::Child) -> Result<usize, String> {
    use std::io::{BufRead, BufReader, Write};

    let stdin = child.stdin.as_mut().ok_or("no stdin pipe")?;
    let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    let tools_list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
    writeln!(stdin, "{initialize}").map_err(|e| format!("write initialize: {e}"))?;
    writeln!(stdin, "{tools_list}").map_err(|e| format!("write tools/list: {e}"))?;
    stdin.flush().map_err(|e| format!("flush: {e}"))?;

    let stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let mut reader = BufReader::new(stdout);

    // Read two response lines. Timeout is implemented by spawning a thread
    // that does the read and joining with a deadline — simplest portable form.
    let (tx, rx) = std::sync::mpsc::channel::<Result<usize, String>>();
    std::thread::spawn(move || {
        let mut line = String::new();
        // initialize response
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
        // tools/list response
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
        let _ = tx.send(Ok(tools));
    });

    rx.recv_timeout(std::time::Duration::from_secs(3))
        .map_err(|_| "timeout waiting for MCP server response (3s)".to_string())?
}

// ----- helpers -------------------------------------------------------------

fn is_skipped_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".mastermind"
            | ".venv"
            | "venv"
            | "__pycache__"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".tox"
            | ".pytest_cache"
            | ".mypy_cache"
            | ".ruff_cache"
            | ".next"
            | ".turbo"
            | ".cache"
    )
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        // Process-global monotonic counter. `process::id()` is identical across
        // cargo's parallel test threads and the nanosecond clock can collide
        // when two threads enter here in the same bucket — that collision is
        // the root cause of the historical `check_gitignore` flake, where one
        // test's `remove_dir_all` wiped another's working dir mid-run. The
        // atomic `fetch_add` guarantees every call gets a distinct value, so no
        // two `tmp()` invocations can ever resolve to the same directory.
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
    fn check_index_db_fails_when_missing() {
        let root = tmp();
        let c = check_index_db(&root);
        assert_eq!(c.status, Status::Fail);
        assert!(c.hint.as_deref().unwrap().contains("mastermind init"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn check_index_db_ok_when_present() {
        let root = tmp();
        fs::create_dir_all(root.join(".mastermind")).unwrap();
        fs::write(root.join(".mastermind/mmcg.db"), b"junk").unwrap();
        let c = check_index_db(&root);
        assert_eq!(c.status, Status::Ok);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn check_gitignore_warns_when_missing_or_unset() {
        let root = tmp();
        // No .gitignore at all.
        assert_eq!(check_gitignore(&root).status, Status::Warn);
        // .gitignore without .mastermind.
        fs::write(root.join(".gitignore"), "node_modules\n").unwrap();
        assert_eq!(check_gitignore(&root).status, Status::Warn);
        // Now with .mastermind.
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
    fn check_mcp_config_finds_local_or_home() {
        let root = tmp();
        // No config anywhere → warn (we can't unset HOME safely, so just
        // assert the local-config branch).
        let c = check_mcp_config(&root);
        // Status may be Ok (if user has a real ~/.claude/.mcp.json with mmcg)
        // or Warn. Tolerate both — the precise assertion is "doesn't crash".
        assert!(matches!(c.status, Status::Ok | Status::Warn));
        // Add a project-local config — should bump to Ok.
        fs::write(
            root.join(".mcp.json"),
            r#"{"mcpServers":{"mmcg":{"command":"mmcg","args":["serve"]}}}"#,
        )
        .unwrap();
        assert_eq!(check_mcp_config(&root).status, Status::Ok);
        fs::remove_dir_all(&root).ok();
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
}
