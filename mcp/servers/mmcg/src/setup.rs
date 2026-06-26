//! `mastermind setup claude` — MCP-config registrar.
//!
//! Registers mmcg with Claude Code. Two scopes:
//!   - user (global, default): `claude mcp add --scope user` → `~/.claude.json`
//!     (why that path, not `~/.claude/.mcp.json`: see `add_claude_user`).
//!   - project (`--project .`): writes `<root>/.mcp.json`, merging into an
//!     existing `mcpServers` object without clobbering others (see `run_claude`).
//!
//! Safe by default: prints what it will do and exits unless `--write-mcp`.
//!
//! Why hand-rolled diff (vs `similar` crate): the change is always additive +
//! contiguous (one entry into one object), and `serde_json::to_string_pretty`
//! gives stable line counts, so a prefix/suffix line-match covers it in ~30
//! lines with no new dependency. Trade-off documented in `render_line_diff`.

use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// MCP config file location on disk + a short label for diff headers.
#[derive(Debug, Clone)]
pub struct Target {
    pub path: PathBuf,
    pub label: String,
}

impl Target {
    /// Project-local `.mcp.json` at the given root.
    pub fn project(root: &Path) -> Self {
        Self {
            path: root.join(".mcp.json"),
            label: format!("{}/.mcp.json", root.display()),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Opts {
    /// Actually write the file. Else prints diff + advisory.
    pub write: bool,
    /// Overwrite an existing customized `mmcg` entry.
    pub force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Showed diff only (no `--write-mcp`).
    DryRun,
    /// Wrote the MCP config.
    Wrote,
    /// Config already had an mmcg entry equivalent to the proposed one.
    NoChange,
    /// Refused to overwrite a customized mmcg entry without `--force`.
    RefusedOverwrite,
    /// I/O or parse error.
    Error,
}

/// Top-level entry point. Reads existing config, computes the merged proposal,
/// renders a diff, optionally writes, optionally drops a CLAUDE.md.
pub fn run_claude(target: &Target, mmcg_binary: &Path, opts: Opts) -> Outcome {
    println!("=== mastermind setup claude ({}) ===", target.label);

    let existing_text = match std::fs::read_to_string(&target.path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            eprintln!("error: reading `{}`: {e}", target.label);
            return Outcome::Error;
        }
    };
    let existing: Value = if existing_text.trim().is_empty() {
        json!({})
    } else {
        match serde_json::from_str(&existing_text) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("error: `{}` is not valid JSON: {e}", target.label);
                return Outcome::Error;
            }
        }
    };

    let proposed_entry = mmcg_entry(mmcg_binary);
    let existing_entry = existing
        .get("mcpServers")
        .and_then(|s| s.get("mmcg"))
        .cloned();

    // Equivalent-existing → no-op.
    if existing_entry.as_ref() == Some(&proposed_entry) {
        println!(
            "mmcg already registered in {} (no change needed).",
            target.label
        );
        return Outcome::NoChange;
    }

    // Customized-existing → refuse without --force.
    if let Some(current) = existing_entry.as_ref() {
        if !opts.force {
            println!(
                "mmcg is already registered in {} with different settings:\n  current  → {}\n  proposed → {}\n\nRe-run with --force to overwrite, or hand-edit the file.",
                target.label,
                serde_json::to_string(current).unwrap_or_default(),
                serde_json::to_string(&proposed_entry).unwrap_or_default(),
            );
            return Outcome::RefusedOverwrite;
        }
    }

    let proposed = match merge_mmcg_entry(&existing, &proposed_entry) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: merging proposed entry: {e}");
            return Outcome::Error;
        }
    };

    let before_pretty = if existing_text.trim().is_empty() {
        // Empty before-state (not `{}`) so the diff reads honestly as creation.
        String::new()
    } else {
        serde_json::to_string_pretty(&existing).unwrap_or_else(|_| existing_text.clone())
    };
    let after_pretty = serde_json::to_string_pretty(&proposed).unwrap_or_default();

    println!("--- {} (current)", target.label);
    println!("+++ {} (proposed)", target.label);
    let diff = render_line_diff(&before_pretty, &after_pretty);
    print!("{diff}");

    let outcome = if opts.write {
        match write_atomic(&target.path, &after_pretty) {
            Ok(()) => {
                println!("\nWrote {} ({} bytes)", target.label, after_pretty.len());
                Outcome::Wrote
            }
            Err(e) => {
                eprintln!("error: writing `{}`: {e}", target.label);
                Outcome::Error
            }
        }
    } else {
        println!("\n(dry-run) Pass --write-mcp to apply.");
        Outcome::DryRun
    };

    outcome
}

/// Decide `command` + `args` for the MCP config from how this binary was
/// invoked. Two sources, checked in order:
///
/// 1. `MASTERMIND_INSTALL_MODE` env var set by the npm JS wrapper:
///    - `npx` → one-shot `npx -y @xcraftmind/mastermind`. Pin to
///      `npx -y @xcraftmind/mastermind@<version> serve` so the MCP client
///      launches the same version every time.
///    - `global` → `npm install -g`. Use the wrapper name `mastermind` (on PATH).
///    - `project` → `npm install -D`. Use the project-local bin path so the
///      version pin is honored.
///    - `unknown` → couldn't classify; treat as `global` (safest default).
/// 2. No env var → cargo-installed `mmcg` (original path). Use the running
///    binary's absolute path, same as before npm support.
///
/// Two-mode design keeps `cargo install mmcg` unchanged while giving npm users a
/// config that travels with their install method.
fn mmcg_entry(mmcg_binary: &Path) -> Value {
    let install_mode = std::env::var("MASTERMIND_INSTALL_MODE").ok();
    let version = std::env::var("MASTERMIND_VERSION").ok();
    let package = std::env::var("MASTERMIND_PACKAGE")
        .ok()
        .unwrap_or_else(|| "@xcraftmind/mastermind".to_string());

    match install_mode.as_deref() {
        Some("npx") => {
            // Pin the version for reproducibility; unpinned
            // `npx @xcraftmind/mastermind` would silently upgrade and could
            // break the integration on a future release.
            let pinned = match version {
                Some(v) => format!("{package}@{v}"),
                None => package,
            };
            json!({
                "command": "npx",
                "args": ["-y", pinned, "serve"],
            })
        }
        Some("project") => {
            // Project-local install. Path relative to the project root (where
            // `.mcp.json` lives), so it survives `cd` into subdirs.
            let bin = if cfg!(windows) {
                "./node_modules/.bin/mastermind.cmd"
            } else {
                "./node_modules/.bin/mastermind"
            };
            json!({
                "command": bin,
                "args": ["serve"],
            })
        }
        Some("global") | Some("unknown") => {
            // `mastermind` is on PATH via npm's global bin directory.
            json!({
                "command": "mastermind",
                "args": ["serve"],
            })
        }
        _ => {
            // No env var → invoked directly (cargo install, manual build, etc.).
            // Absolute path of the running binary guarantees the MCP client
            // launches the exact binary the user just ran.
            json!({
                "command": mmcg_binary.display().to_string(),
                "args": ["serve"],
            })
        }
    }
}

/// Deep-merge `mmcg` into `existing.mcpServers`, preserving other servers.
fn merge_mmcg_entry(existing: &Value, mmcg_entry: &Value) -> Result<Value, String> {
    let mut proposed = existing.clone();
    if !proposed.is_object() {
        // Replace non-object roots (`null`, array) with `{}` — covers an
        // empty-or-malformed file we already accepted.
        proposed = json!({});
    }
    let obj = proposed.as_object_mut().unwrap();
    let servers = obj
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        return Err("`mcpServers` is present but not a JSON object".into());
    }
    let servers_obj = servers.as_object_mut().unwrap();
    servers_obj.insert("mmcg".to_string(), mmcg_entry.clone());
    Ok(proposed)
}

/// Remove the `mmcg` entry from an MCP config file. Leaves other servers intact
/// and leaves an empty `mcpServers` object rather than deleting the file. Safe
/// by default — prints a diff and exits unless `write`.
pub fn remove_claude(target: &Target, write: bool) -> Outcome {
    println!("=== mastermind uninstall claude ({}) ===", target.label);

    let existing_text = match std::fs::read_to_string(&target.path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("No MCP config at `{}` — nothing to remove.", target.label);
            return Outcome::NoChange;
        }
        Err(e) => {
            eprintln!("error: reading `{}`: {e}", target.label);
            return Outcome::Error;
        }
    };

    let mut config: Value = match serde_json::from_str(&existing_text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: `{}` is not valid JSON: {e}", target.label);
            return Outcome::Error;
        }
    };

    if config.pointer("/mcpServers/mmcg").is_none() {
        println!("No `mmcg` entry in `{}` — nothing to remove.", target.label);
        return Outcome::NoChange;
    }

    if let Some(servers) = config.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
        servers.remove("mmcg");
    }

    let after = format!("{}\n", serde_json::to_string_pretty(&config).unwrap());
    println!("\n--- {} (current)", target.label);
    println!("+++ {} (proposed)\n", target.label);
    print!("{}", render_line_diff(&existing_text, &after));

    if !write {
        println!("\n(dry-run — pass --force to apply)");
        return Outcome::DryRun;
    }

    if let Err(e) = write_atomic(&target.path, &after) {
        eprintln!("error: writing `{}`: {e}", target.label);
        return Outcome::Error;
    }
    println!("\nRemoved `mmcg` from {}.", target.label);
    Outcome::Wrote
}

/// Register mmcg at Claude Code's **user scope** via the official `claude mcp
/// add` CLI, which writes `~/.claude.json` — the location Claude Code actually
/// reads for global servers. (The previous approach wrote `~/.claude/.mcp.json`,
/// which Claude Code ignores, so global registration silently never took
/// effect.) Safe by default: prints the command and exits unless `opts.write`.
pub fn add_claude_user(mmcg_binary: &Path, opts: Opts) -> Outcome {
    println!("=== mmcg setup claude (user scope → ~/.claude.json via `claude mcp add`) ===");

    let entry = mmcg_entry(mmcg_binary);
    let command = entry
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("mastermind")
        .to_string();
    let args: Vec<String> = entry
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let preview = format!(
        "claude mcp add --scope user mmcg -- {command} {}",
        args.join(" ")
    );

    let already = claude_mcp_has("mmcg");

    if already && !opts.force {
        println!(
            "mmcg is already registered with Claude Code. Re-run with --force to re-register.\n  would run: {preview}"
        );
        return Outcome::NoChange;
    }

    println!("Will run:\n  {preview}");
    if !opts.write {
        println!("\n(dry-run) Pass --write-mcp to apply.");
        return Outcome::DryRun;
    }

    // Re-register cleanly: drop any existing entry first (best-effort), then add.
    if already {
        let _ = std::process::Command::new("claude")
            .args(["mcp", "remove", "mmcg", "-s", "user"])
            .stdin(std::process::Stdio::null())
            .status();
    }
    let status = std::process::Command::new("claude")
        .args(["mcp", "add", "--scope", "user", "mmcg", "--"])
        .arg(&command)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .status();
    let outcome = match status {
        Ok(s) if s.success() => {
            println!("\n✓ Registered mmcg with Claude Code (user scope, ~/.claude.json). Restart Claude Code to load the tools.");
            Outcome::Wrote
        }
        Ok(s) => {
            eprintln!("error: `claude mcp add` exited with {s}");
            Outcome::Error
        }
        Err(e) => {
            eprintln!(
                "error: could not run `claude mcp add`: {e} — is the Claude Code CLI installed and on PATH?"
            );
            Outcome::Error
        }
    };
    outcome
}

/// Remove mmcg from Claude Code's user scope via `claude mcp remove`. Safe by
/// default: prints the command, exits unless `write`.
pub fn remove_claude_user(write: bool) -> Outcome {
    println!("=== mmcg uninstall claude (user scope via `claude mcp remove`) ===");
    if !claude_mcp_has("mmcg") {
        println!("mmcg is not registered with Claude Code — nothing to remove.");
        return Outcome::NoChange;
    }
    println!("Will run:\n  claude mcp remove mmcg -s user");
    if !write {
        println!("\n(dry-run — pass --force to apply)");
        return Outcome::DryRun;
    }
    match std::process::Command::new("claude")
        .args(["mcp", "remove", "mmcg", "-s", "user"])
        .stdin(std::process::Stdio::null())
        .status()
    {
        Ok(s) if s.success() => {
            println!("\nRemoved mmcg from Claude Code (user scope).");
            Outcome::Wrote
        }
        Ok(s) => {
            eprintln!("error: `claude mcp remove` exited with {s}");
            Outcome::Error
        }
        Err(e) => {
            eprintln!("error: could not run `claude mcp remove`: {e}");
            Outcome::Error
        }
    }
}

/// True if `claude mcp get <name>` reports the server registered (any scope).
/// Best-effort: false if the Claude CLI is missing or errors.
fn claude_mcp_has(name: &str) -> bool {
    std::process::Command::new("claude")
        .args(["mcp", "get", name])
        .stdin(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Line diff: common prefix/suffix, marks only the changed window with up to 3
/// context lines. Assumes a single contiguous change region (safe here — the
/// merge is always additive on one key). Use `similar` if multi-region diffs are
/// ever needed.
fn render_line_diff(before: &str, after: &str) -> String {
    if before == after {
        return "(no changes)\n".to_string();
    }
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();

    let common_max = before_lines.len().min(after_lines.len());
    let prefix_len = (0..common_max)
        .take_while(|&i| before_lines[i] == after_lines[i])
        .count();
    let max_suffix = common_max - prefix_len;
    let suffix_len = (0..max_suffix)
        .take_while(|&i| {
            before_lines[before_lines.len() - 1 - i] == after_lines[after_lines.len() - 1 - i]
        })
        .count();

    let mut out = String::new();
    let ctx_pre_start = prefix_len.saturating_sub(3);
    for line in &before_lines[ctx_pre_start..prefix_len] {
        out.push_str(&format!("  {line}\n"));
    }
    for line in &before_lines[prefix_len..before_lines.len() - suffix_len] {
        out.push_str(&format!("- {line}\n"));
    }
    for line in &after_lines[prefix_len..after_lines.len() - suffix_len] {
        out.push_str(&format!("+ {line}\n"));
    }
    let ctx_post_start = before_lines.len() - suffix_len;
    let ctx_post_end = (ctx_post_start + 3).min(before_lines.len());
    for line in &before_lines[ctx_post_start..ctx_post_end] {
        out.push_str(&format!("  {line}\n"));
    }
    out
}

/// Write `body` to `path` atomically (tmp + rename). Creates parent dirs.
fn write_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // `mmcg_entry` reads process-global MASTERMIND_* env vars. Cargo runs tests
    // in parallel in one process, so an env mutation in one test can leak into
    // another. Every test that sets OR depends on the default of these vars holds
    // this lock for its duration, making them mutually exclusive. Poison-tolerant
    // — one panicking test must not cascade-fail the rest.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "mmcg-setup-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn render_line_diff_marks_added_and_removed_lines() {
        let before = "a\nb\nc\nd\ne\n";
        let after = "a\nb\nX\nY\nd\ne\n";
        let d = render_line_diff(before, after);
        assert!(d.contains("- c"));
        assert!(d.contains("+ X"));
        assert!(d.contains("+ Y"));
        assert!(d.contains("  b")); // prefix context
        assert!(d.contains("  d")); // suffix context
    }

    #[test]
    fn render_line_diff_returns_no_changes_for_identical() {
        let s = "a\nb\nc\n";
        assert!(render_line_diff(s, s).contains("no changes"));
    }

    #[test]
    fn merge_preserves_other_servers() {
        let _env = env_lock();
        let existing = json!({
            "mcpServers": {
                "other-server": {"command": "other", "args": ["run"]}
            }
        });
        let entry = mmcg_entry(Path::new("/usr/local/bin/mmcg"));
        let merged = merge_mmcg_entry(&existing, &entry).unwrap();
        let servers = merged
            .get("mcpServers")
            .and_then(|s| s.as_object())
            .unwrap();
        assert!(
            servers.contains_key("other-server"),
            "other server should be preserved"
        );
        assert!(servers.contains_key("mmcg"));
        assert_eq!(
            servers.get("mmcg").and_then(|m| m.get("command")),
            Some(&json!("/usr/local/bin/mmcg"))
        );
    }

    #[test]
    fn merge_creates_mcp_servers_object_when_absent() {
        let existing = json!({});
        let entry = mmcg_entry(Path::new("/usr/bin/mmcg"));
        let merged = merge_mmcg_entry(&existing, &entry).unwrap();
        assert!(merged.get("mcpServers").is_some());
    }

    #[test]
    fn merge_rejects_non_object_mcp_servers() {
        let existing = json!({"mcpServers": "bogus-string"});
        let entry = mmcg_entry(Path::new("/usr/bin/mmcg"));
        assert!(merge_mmcg_entry(&existing, &entry).is_err());
    }

    #[test]
    fn run_claude_dry_run_does_not_write_file() {
        let dir = tmp("dry_run_no_write");
        let target = Target {
            path: dir.join(".mcp.json"),
            label: "<test>".into(),
        };
        let outcome = run_claude(
            &target,
            Path::new("/usr/local/bin/mmcg"),
            Opts::default(), // write=false
        );
        assert_eq!(outcome, Outcome::DryRun);
        assert!(!target.path.exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_claude_write_creates_file_with_merged_entry() {
        let _env = env_lock();
        let dir = tmp("write_creates");
        let target = Target {
            path: dir.join(".mcp.json"),
            label: "<test>".into(),
        };
        let outcome = run_claude(
            &target,
            Path::new("/usr/local/bin/mmcg"),
            Opts {
                write: true,
                ..Default::default()
            },
        );
        assert_eq!(outcome, Outcome::Wrote);
        let body = fs::read_to_string(&target.path).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v.pointer("/mcpServers/mmcg/command").is_some());
        assert_eq!(
            v.pointer("/mcpServers/mmcg/command").unwrap(),
            &json!("/usr/local/bin/mmcg")
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_claude_no_change_when_entry_matches() {
        let _env = env_lock();
        let dir = tmp("no_change_match");
        let target = Target {
            path: dir.join(".mcp.json"),
            label: "<test>".into(),
        };
        // Seed the exact entry mmcg_entry would produce.
        fs::write(
            &target.path,
            r#"{
  "mcpServers": {
    "mmcg": {
      "command": "/usr/local/bin/mmcg",
      "args": [
        "serve"
      ]
    }
  }
}"#,
        )
        .unwrap();
        let outcome = run_claude(
            &target,
            Path::new("/usr/local/bin/mmcg"),
            Opts {
                write: true,
                ..Default::default()
            },
        );
        assert_eq!(outcome, Outcome::NoChange);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_claude_refuses_overwrite_without_force() {
        let dir = tmp("refuse_overwrite");
        let target = Target {
            path: dir.join(".mcp.json"),
            label: "<test>".into(),
        };
        // Pre-existing customized entry.
        fs::write(
            &target.path,
            r#"{"mcpServers":{"mmcg":{"command":"/opt/custom/mmcg","args":["serve","--verbose"]}}}"#,
        )
        .unwrap();
        let outcome = run_claude(
            &target,
            Path::new("/usr/local/bin/mmcg"),
            Opts {
                write: true,
                ..Default::default()
            },
        );
        assert_eq!(outcome, Outcome::RefusedOverwrite);
        // Original entry intact.
        let body = fs::read_to_string(&target.path).unwrap();
        assert!(body.contains("/opt/custom/mmcg"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_claude_force_overwrites_customized_entry() {
        let _env = env_lock();
        let dir = tmp("force_overwrites");
        let target = Target {
            path: dir.join(".mcp.json"),
            label: "<test>".into(),
        };
        fs::write(
            &target.path,
            r#"{"mcpServers":{"mmcg":{"command":"/opt/custom/mmcg","args":["serve","--verbose"]}}}"#,
        )
        .unwrap();
        let outcome = run_claude(
            &target,
            Path::new("/usr/local/bin/mmcg"),
            Opts {
                write: true,
                force: true,
            },
        );
        assert_eq!(outcome, Outcome::Wrote);
        let body = fs::read_to_string(&target.path).unwrap();
        assert!(body.contains("/usr/local/bin/mmcg"));
        assert!(!body.contains("--verbose"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mmcg_entry_cargo_mode_uses_absolute_binary_path() {
        // No MASTERMIND_INSTALL_MODE → cargo-install / direct-invocation. Must
        // use the absolute binary path so the MCP client launches the exact
        // binary the user just configured. Clear the vars to avoid cross-test
        // contamination.
        let _guard = EnvGuard::clear(&[
            "MASTERMIND_INSTALL_MODE",
            "MASTERMIND_VERSION",
            "MASTERMIND_PACKAGE",
        ]);
        let entry = mmcg_entry(Path::new("/opt/cargo/bin/mmcg"));
        assert_eq!(
            entry.get("command").and_then(|v| v.as_str()),
            Some("/opt/cargo/bin/mmcg")
        );
        assert_eq!(entry.get("args"), Some(&serde_json::json!(["serve"])));
    }

    #[test]
    fn mmcg_entry_global_mode_uses_mastermind_on_path() {
        let _guard = EnvGuard::set(&[
            ("MASTERMIND_INSTALL_MODE", "global"),
            ("MASTERMIND_VERSION", "0.22.0"),
            ("MASTERMIND_PACKAGE", "@xcraftmind/mastermind"),
        ]);
        let entry = mmcg_entry(Path::new("/ignored/path/mmcg"));
        assert_eq!(
            entry.get("command").and_then(|v| v.as_str()),
            Some("mastermind"),
            "global install should write the PATH name, not the npm cache path"
        );
    }

    #[test]
    fn mmcg_entry_project_mode_writes_node_modules_bin() {
        let _guard = EnvGuard::set(&[("MASTERMIND_INSTALL_MODE", "project")]);
        let entry = mmcg_entry(Path::new("/ignored"));
        let cmd = entry.get("command").and_then(|v| v.as_str()).unwrap();
        // Unix path on non-Windows; .cmd on Windows.
        if cfg!(windows) {
            assert!(
                cmd.ends_with("node_modules/.bin/mastermind.cmd")
                    || cmd.ends_with(r"node_modules\.bin\mastermind.cmd")
            );
        } else {
            assert_eq!(cmd, "./node_modules/.bin/mastermind");
        }
    }

    #[test]
    fn mmcg_entry_npx_mode_pins_version() {
        let _guard = EnvGuard::set(&[
            ("MASTERMIND_INSTALL_MODE", "npx"),
            ("MASTERMIND_VERSION", "0.22.0"),
            ("MASTERMIND_PACKAGE", "@xcraftmind/mastermind"),
        ]);
        let entry = mmcg_entry(Path::new("/ignored"));
        assert_eq!(entry.get("command").and_then(|v| v.as_str()), Some("npx"));
        let args = entry.get("args").and_then(|v| v.as_array()).unwrap();
        // Version pinned in the package spec arg.
        assert!(
            args.iter()
                .any(|a| a.as_str() == Some("@xcraftmind/mastermind@0.22.0")),
            "expected version-pinned npx package arg; got {:?}",
            args
        );
    }

    #[test]
    fn mmcg_entry_npx_mode_falls_back_when_version_absent() {
        // No MASTERMIND_VERSION → unpinned npx command (wrapper should warn).
        // Still valid MCP config, just not pinned.
        let _guard = EnvGuard::set(&[("MASTERMIND_INSTALL_MODE", "npx")]);
        std::env::remove_var("MASTERMIND_VERSION");
        let entry = mmcg_entry(Path::new("/ignored"));
        let args = entry.get("args").and_then(|v| v.as_array()).unwrap();
        assert!(
            args.iter()
                .any(|a| a.as_str() == Some("@xcraftmind/mastermind")),
            "expected unpinned package arg when MASTERMIND_VERSION absent"
        );
    }

    /// RAII helper that sets/unsets env vars for a test scope and restores prior
    /// values on drop. Holds `ENV_LOCK` for its lifetime so env-driven tests are
    /// mutually exclusive even under cargo's parallel runner — reader tests that
    /// depend on the default environment take the same lock via `env_lock()`. No
    /// `--test-threads=1` required.
    struct EnvGuard {
        prior: Vec<(String, Option<String>)>,
        // Held until drop so env mutation + restore happen while no other
        // env-touching test runs. Declared after `prior` so it releases only
        // after the Drop impl has restored the prior values.
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(pairs: &[(&str, &str)]) -> Self {
            let _lock = env_lock();
            let prior = pairs
                .iter()
                .map(|(k, _)| (k.to_string(), std::env::var(k).ok()))
                .collect();
            for (k, v) in pairs {
                std::env::set_var(k, v);
            }
            Self { prior, _lock }
        }
        fn clear(keys: &[&str]) -> Self {
            let _lock = env_lock();
            let prior = keys
                .iter()
                .map(|k| (k.to_string(), std::env::var(k).ok()))
                .collect();
            for k in keys {
                std::env::remove_var(k);
            }
            Self { prior, _lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.prior {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    #[test]
    fn run_claude_preserves_other_servers_through_write() {
        let dir = tmp("other_servers_through_write");
        let target = Target {
            path: dir.join(".mcp.json"),
            label: "<test>".into(),
        };
        fs::write(
            &target.path,
            r#"{"mcpServers":{"other":{"command":"x","args":[]}}}"#,
        )
        .unwrap();
        let _ = run_claude(
            &target,
            Path::new("/bin/mmcg"),
            Opts {
                write: true,
                ..Default::default()
            },
        );
        let body = fs::read_to_string(&target.path).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        let servers = v.get("mcpServers").and_then(|s| s.as_object()).unwrap();
        assert!(servers.contains_key("other"));
        assert!(servers.contains_key("mmcg"));
        fs::remove_dir_all(&dir).ok();
    }
}
