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

use serde::{de, Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const CONFIG_MAX_BYTES: usize = 2 * 1024 * 1024;
pub const PROCESS_OUTPUT_MAX_BYTES: usize = 64 * 1024;
pub const PROCESS_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Client {
    Claude,
    Cursor,
    Codex,
    Continue,
    Generic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Project,
    User,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub client: Client,
    pub scope: Scope,
    pub root: PathBuf,
    pub config: Option<PathBuf>,
    pub write: bool,
    pub remove: bool,
    pub force: bool,
}

pub fn run(request: &Request, mmcg_binary: &Path) -> Outcome {
    let entry = mmcg_entry(mmcg_binary);
    if let Err(class) = validate_request(request) {
        return finish_error(request, "unresolved", operation(request), &entry, class);
    }
    match (request.client, request.scope) {
        (Client::Claude | Client::Codex, Scope::User) => run_native(request, &entry),
        (Client::Continue, _) => match target_for(request) {
            Ok(target) => run_continue(request, &target, &entry),
            Err(class) => finish_error(request, "unresolved", operation(request), &entry, class),
        },
        _ => match target_for(request) {
            Ok(target) => run_json(request, &target, &entry),
            Err(class) => finish_error(request, "unresolved", operation(request), &entry, class),
        },
    }
}

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
    let request = Request {
        client: Client::Claude,
        scope: Scope::Project,
        root: target
            .path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        config: None,
        write: opts.write,
        remove: false,
        force: opts.force,
    };
    run_json(&request, target, &mmcg_entry(mmcg_binary))
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
    #[cfg(test)]
    if let Some(entry) = TEST_CANONICAL_ENTRY.with(|slot| slot.borrow().clone()) {
        return entry;
    }
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

#[cfg(test)]
thread_local! {
    static TEST_CANONICAL_ENTRY: std::cell::RefCell<Option<Value>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
struct TestCanonicalEntryGuard {
    previous: Option<Value>,
}

#[cfg(test)]
impl TestCanonicalEntryGuard {
    fn new(entry: Value) -> Self {
        let previous = TEST_CANONICAL_ENTRY.with(|slot| slot.replace(Some(entry)));
        Self { previous }
    }
}

#[cfg(test)]
impl Drop for TestCanonicalEntryGuard {
    fn drop(&mut self) {
        TEST_CANONICAL_ENTRY.with(|slot| {
            slot.replace(self.previous.take());
        });
    }
}

pub(crate) fn canonical_entry(mmcg_binary: &Path) -> Value {
    mmcg_entry(mmcg_binary)
}

pub(crate) fn read_json_mmcg(path: &Path) -> Result<Option<Value>, String> {
    let Some(bytes) = read_capped(path)? else {
        return Ok(None);
    };
    if bytes.is_empty() {
        return Ok(None);
    }
    let value = parse_json_unique(&bytes)?;
    if !value.is_object() {
        return Err("invalid_json_root".into());
    }
    let servers = match value.get("mcpServers") {
        Some(servers) if !servers.is_object() => return Err("invalid_mcp_servers".into()),
        Some(servers) => servers,
        None => return Ok(None),
    };
    Ok(servers.get("mmcg").cloned())
}

/// Deep-merge `mmcg` into `existing.mcpServers`, preserving other servers.
fn merge_mmcg_entry(existing: &Value, mmcg_entry: &Value) -> Result<Value, String> {
    let mut proposed = existing.clone();
    if !proposed.is_object() {
        return Err("JSON root must be an object".into());
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
    let request = Request {
        client: Client::Claude,
        scope: Scope::Project,
        root: target
            .path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        config: None,
        write,
        remove: true,
        force: write,
    };
    run_json(&request, target, &json!({}))
}

/// Register mmcg at Claude Code's **user scope** via the official `claude mcp
/// add` CLI, which writes `~/.claude.json` — the location Claude Code actually
/// reads for global servers. (The previous approach wrote `~/.claude/.mcp.json`,
/// which Claude Code ignores, so global registration silently never took
/// effect.) Safe by default: prints the command and exits unless `opts.write`.
pub fn add_claude_user(mmcg_binary: &Path, opts: Opts) -> Outcome {
    run_native(
        &Request {
            client: Client::Claude,
            scope: Scope::User,
            root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            config: None,
            write: opts.write,
            remove: false,
            force: opts.force,
        },
        &mmcg_entry(mmcg_binary),
    )
}

/// Remove mmcg from Claude Code's user scope via `claude mcp remove`. Safe by
/// default: prints the command, exits unless `write`.
pub fn remove_claude_user(write: bool) -> Outcome {
    run_native(
        &Request {
            client: Client::Claude,
            scope: Scope::User,
            root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            config: None,
            write,
            remove: true,
            force: write,
        },
        &json!({}),
    )
}

fn validate_request(request: &Request) -> Result<(), &'static str> {
    match request.client {
        Client::Generic if request.config.is_none() => Err("generic_requires_config"),
        Client::Generic => Ok(()),
        _ if request.config.is_some() => Err("config_only_supported_for_generic"),
        Client::Codex if request.scope == Scope::Project => Err("codex_project_unsupported"),
        _ => Ok(()),
    }
}

fn setup_home_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(home) = TEST_HOME_DIR.with(|slot| slot.borrow().clone()) {
        return Some(home);
    }
    dirs::home_dir()
}

fn target_for(request: &Request) -> Result<Target, &'static str> {
    let home = || setup_home_dir().ok_or("home_directory_unavailable");
    let path = match (request.client, request.scope) {
        (Client::Claude, Scope::Project) => request.root.join(".mcp.json"),
        (Client::Cursor, Scope::Project) => request.root.join(".cursor/mcp.json"),
        (Client::Cursor, Scope::User) => home()?.join(".cursor/mcp.json"),
        (Client::Continue, Scope::Project) => {
            request.root.join(".continue/mcpServers/mastermind.yaml")
        }
        (Client::Continue, Scope::User) => home()?.join(".continue/mcpServers/mastermind.yaml"),
        (Client::Generic, _) => request.config.clone().ok_or("generic_requires_config")?,
        _ => return Err("native_target_has_no_config_path"),
    };
    Ok(Target {
        label: path.display().to_string(),
        path,
    })
}

fn run_json(request: &Request, target: &Target, entry: &Value) -> Outcome {
    let observed = match read_capped(&target.path) {
        Ok(value) => value,
        Err(class) => {
            return finish_error(request, &target.label, operation(request), entry, &class)
        }
    };
    let mut config = match observed.as_deref() {
        None => json!({}),
        Some([]) => json!({}),
        Some(bytes) => match parse_json_unique(bytes) {
            Ok(value) if value.is_object() => value,
            Ok(_) => {
                return finish_error(
                    request,
                    &target.label,
                    operation(request),
                    entry,
                    "invalid_json_root",
                )
            }
            Err(class) => {
                return finish_error(request, &target.label, operation(request), entry, &class)
            }
        },
    };
    let existing = match config.get("mcpServers") {
        Some(servers) if !servers.is_object() => {
            return finish_error(
                request,
                &target.label,
                operation(request),
                entry,
                "invalid_mcp_servers",
            )
        }
        Some(servers) => servers.get("mmcg").cloned(),
        None => None,
    };

    if request.remove {
        let Some(current) = existing else {
            return finish_outcome(request, &target.label, "remove", entry, Outcome::NoChange);
        };
        let customized = current != *entry;
        if customized && !request.force {
            return finish_outcome(
                request,
                &target.label,
                "remove",
                entry,
                Outcome::RefusedOverwrite,
            );
        }
        if !request.write {
            return finish_outcome(request, &target.label, "remove", entry, Outcome::DryRun);
        }
        if customized {
            if let Some(bytes) = observed.as_deref() {
                if backup_private(&target.path, bytes).is_err() {
                    return finish_error(request, &target.label, "remove", entry, "backup_failed");
                }
            }
        }
        if let Some(servers) = config.get_mut("mcpServers").and_then(Value::as_object_mut) {
            servers.remove("mmcg");
        }
    } else {
        if existing.as_ref() == Some(entry) {
            return finish_outcome(request, &target.label, "install", entry, Outcome::NoChange);
        }
        let customized = existing.is_some();
        if customized && !request.force {
            return finish_outcome(
                request,
                &target.label,
                "install",
                entry,
                Outcome::RefusedOverwrite,
            );
        }
        if !request.write {
            return finish_outcome(request, &target.label, "install", entry, Outcome::DryRun);
        }
        if customized {
            if let Some(bytes) = observed.as_deref() {
                if backup_private(&target.path, bytes).is_err() {
                    return finish_error(request, &target.label, "install", entry, "backup_failed");
                }
            }
        }
        config = match merge_mmcg_entry(&config, entry) {
            Ok(value) => value,
            Err(class) => return finish_error(request, &target.label, "install", entry, &class),
        };
    }

    let mut body = match serde_json::to_vec_pretty(&config) {
        Ok(body) => body,
        Err(_) => {
            return finish_error(
                request,
                &target.label,
                operation(request),
                entry,
                "serialization_failed",
            )
        }
    };
    body.push(b'\n');
    match safe_replace(&target.path, observed.as_deref(), &body) {
        Ok(()) => finish_outcome(
            request,
            &target.label,
            operation(request),
            entry,
            Outcome::Wrote,
        ),
        Err(class) => finish_error(request, &target.label, operation(request), entry, &class),
    }
}

fn continue_entry(entry: &Value) -> Value {
    json!({
        "schema": 1,
        "owner": "mastermind",
        "name": "mmcg",
        "command": entry.get("command").cloned().unwrap_or(Value::Null),
        "args": entry.get("args").cloned().unwrap_or_else(|| json!([])),
    })
}

fn run_continue(request: &Request, target: &Target, entry: &Value) -> Outcome {
    let observed = match read_capped(&target.path) {
        Ok(value) => value,
        Err(class) => {
            return finish_error(request, &target.label, operation(request), entry, &class)
        }
    };
    let canonical = continue_entry(entry);
    let existing = match observed.as_deref() {
        None => None,
        Some(bytes) => match std::str::from_utf8(bytes)
            .map_err(|_| "invalid_yaml_encoding".to_string())
            .and_then(|body| {
                serde_norway::from_str::<Value>(body).map_err(|_| "invalid_yaml".to_string())
            }) {
            Ok(value) => Some(value),
            Err(class) => {
                return finish_error(request, &target.label, operation(request), entry, &class)
            }
        },
    };

    if request.remove {
        let Some(current) = existing.as_ref() else {
            return finish_outcome(request, &target.label, "remove", entry, Outcome::NoChange);
        };
        let customized = current != &canonical;
        if customized && !request.force {
            return finish_outcome(
                request,
                &target.label,
                "remove",
                entry,
                Outcome::RefusedOverwrite,
            );
        }
        if !request.write {
            return finish_outcome(request, &target.label, "remove", entry, Outcome::DryRun);
        }
        if customized {
            if let Some(bytes) = observed.as_deref() {
                if backup_private(&target.path, bytes).is_err() {
                    return finish_error(request, &target.label, "remove", entry, "backup_failed");
                }
            }
        }
        return match safe_remove(&target.path, observed.as_deref()) {
            Ok(()) => finish_outcome(request, &target.label, "remove", entry, Outcome::Wrote),
            Err(class) => finish_error(request, &target.label, "remove", entry, &class),
        };
    }

    if existing.as_ref() == Some(&canonical) {
        return finish_outcome(request, &target.label, "install", entry, Outcome::NoChange);
    }
    if existing.is_some() && !request.force {
        return finish_outcome(
            request,
            &target.label,
            "install",
            entry,
            Outcome::RefusedOverwrite,
        );
    }
    if !request.write {
        return finish_outcome(request, &target.label, "install", entry, Outcome::DryRun);
    }
    if existing.is_some() {
        if let Some(bytes) = observed.as_deref() {
            if backup_private(&target.path, bytes).is_err() {
                return finish_error(request, &target.label, "install", entry, "backup_failed");
            }
        }
    }
    let body = match serde_norway::to_string(&canonical) {
        Ok(body) => body,
        Err(_) => {
            return finish_error(
                request,
                &target.label,
                "install",
                entry,
                "serialization_failed",
            )
        }
    };
    match safe_replace(&target.path, observed.as_deref(), body.as_bytes()) {
        Ok(()) => finish_outcome(request, &target.label, "install", entry, Outcome::Wrote),
        Err(class) => finish_error(request, &target.label, "install", entry, &class),
    }
}

#[derive(Debug)]
struct BoundedOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    #[cfg(all(test, unix))]
    stderr: Vec<u8>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedNativeState {
    Claude {
        command_fields: Vec<String>,
        args_fields: Vec<String>,
    },
    Codex {
        command: String,
        args: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NativeState {
    Absent,
    Canonical(ParsedNativeState),
    Customized(ParsedNativeState),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecutableIdentity {
    canonical_path: PathBuf,
    size: u64,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
    #[cfg(unix)]
    uid: u32,
    #[cfg(unix)]
    mtime_seconds: i64,
    #[cfg(unix)]
    mtime_nanoseconds: i64,
    #[cfg(not(unix))]
    readonly: bool,
    #[cfg(not(unix))]
    modified: Option<std::time::SystemTime>,
}

impl ExecutableIdentity {
    fn capture(path: &Path) -> Result<Self, String> {
        let canonical_path = path
            .canonicalize()
            .map_err(|_| "native_executable_identity_failed".to_string())?;
        let metadata = std::fs::metadata(&canonical_path)
            .map_err(|_| "native_executable_identity_failed".to_string())?;
        if !metadata.is_file() {
            return Err("native_executable_identity_failed".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                canonical_path,
                size: metadata.len(),
                device: metadata.dev(),
                inode: metadata.ino(),
                mode: metadata.mode(),
                uid: metadata.uid(),
                mtime_seconds: metadata.mtime(),
                mtime_nanoseconds: metadata.mtime_nsec(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                canonical_path,
                size: metadata.len(),
                readonly: metadata.permissions().readonly(),
                modified: metadata.modified().ok(),
            })
        }
    }

    fn verify(&self, path: &Path) -> Result<(), String> {
        match Self::capture(path) {
            Ok(current) if current == *self => Ok(()),
            _ => Err("native_executable_changed".into()),
        }
    }
}

fn run_native(request: &Request, entry: &Value) -> Outcome {
    let name = match request.client {
        Client::Claude => "claude",
        Client::Codex => "codex",
        _ => {
            return finish_error(
                request,
                "native",
                operation(request),
                entry,
                "invalid_native_client",
            )
        }
    };
    let program = match resolve_native(name, &request.root) {
        Ok(program) => program,
        Err(class) => return finish_error(request, "native", operation(request), entry, &class),
    };
    let identity = match ExecutableIdentity::capture(&program) {
        Ok(identity) => identity,
        Err(class) => return finish_error(request, "native", operation(request), entry, &class),
    };
    let inspected = match native_inspect(request.client, &program, &identity, entry) {
        Ok(state) => state,
        Err(class) => return finish_error(request, "native", operation(request), entry, &class),
    };
    let present = !matches!(inspected, NativeState::Absent);
    let canonical = matches!(inspected, NativeState::Canonical(_));

    if request.remove {
        if !present {
            return finish_outcome(request, "native", "remove", entry, Outcome::NoChange);
        }
        if !canonical && !request.force {
            return finish_outcome(
                request,
                "native",
                "remove",
                entry,
                Outcome::RefusedOverwrite,
            );
        }
    } else if canonical {
        return finish_outcome(request, "native", "install", entry, Outcome::NoChange);
    } else if present && !request.force {
        return finish_outcome(
            request,
            "native",
            "install",
            entry,
            Outcome::RefusedOverwrite,
        );
    }

    if !request.write {
        return finish_outcome(
            request,
            "native",
            operation(request),
            entry,
            Outcome::DryRun,
        );
    }
    let rechecked = match native_inspect(request.client, &program, &identity, entry) {
        Ok(state) => state,
        Err(class) => return finish_error(request, "native", operation(request), entry, &class),
    };
    if inspected != rechecked {
        return finish_error(
            request,
            "native",
            operation(request),
            entry,
            "native_state_changed",
        );
    }

    if present {
        let remove_args = native_remove_args(request.client);
        match run_native_checked(&program, &identity, &remove_args) {
            Ok(output) if output.status.success() => {}
            _ => {
                return finish_error(
                    request,
                    "native",
                    operation(request),
                    entry,
                    "native_remove_failed",
                )
            }
        }
    }
    if request.remove {
        return finish_outcome(request, "native", "remove", entry, Outcome::Wrote);
    }
    let add_args = native_add_args(request.client, entry);
    match run_native_checked(&program, &identity, &add_args) {
        Ok(output) if output.status.success() => {
            finish_outcome(request, "native", "install", entry, Outcome::Wrote)
        }
        _ => finish_error(request, "native", "install", entry, "native_add_failed"),
    }
}

fn run_native_checked(
    program: &Path,
    identity: &ExecutableIdentity,
    args: &[String],
) -> Result<BoundedOutput, String> {
    identity.verify(program)?;
    run_bounded(program, args)
}

fn native_inspect(
    client: Client,
    program: &Path,
    identity: &ExecutableIdentity,
    entry: &Value,
) -> Result<NativeState, String> {
    let mut args = vec!["mcp".into(), "get".into(), "mmcg".into()];
    if client == Client::Codex {
        args.push("--json".into());
    };
    let output = run_native_checked(program, identity, &args)?;
    native_matches(client, &output, entry)
}

fn native_matches(
    client: Client,
    output: &BoundedOutput,
    entry: &Value,
) -> Result<NativeState, String> {
    if output.stdout_truncated || output.stderr_truncated {
        return Err("native_output_truncated".into());
    }
    if !output.status.success() {
        return Ok(NativeState::Absent);
    }
    let command = entry
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| "native_entry_invalid".to_string())?;
    let args = entry
        .get("args")
        .and_then(Value::as_array)
        .ok_or_else(|| "native_entry_invalid".to_string())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "native_entry_invalid".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;

    let parsed = match client {
        Client::Claude => parse_claude_native(&output.stdout)?,
        Client::Codex => parse_codex_native(&output.stdout)?,
        _ => return Err("invalid_native_client".into()),
    };
    let canonical = match &parsed {
        ParsedNativeState::Claude {
            command_fields,
            args_fields,
        } => command_fields.as_slice() == [command] && args_fields.as_slice() == [args.join(" ")],
        ParsedNativeState::Codex {
            command: observed_command,
            args: observed_args,
        } => observed_command == command && observed_args == &args,
    };
    if canonical {
        Ok(NativeState::Canonical(parsed))
    } else {
        Ok(NativeState::Customized(parsed))
    }
}

fn parse_claude_native(bytes: &[u8]) -> Result<ParsedNativeState, String> {
    let body = std::str::from_utf8(bytes).map_err(|_| "native_parse_failed".to_string())?;
    let mut command_fields = Vec::new();
    let mut args_fields = Vec::new();
    for line in body.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("Command:") {
            command_fields.push(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Args:") {
            args_fields.push(value.trim().to_string());
        }
    }
    Ok(ParsedNativeState::Claude {
        command_fields,
        args_fields,
    })
}

fn parse_codex_native(bytes: &[u8]) -> Result<ParsedNativeState, String> {
    let value = parse_json_unique(bytes).map_err(|_| "native_parse_failed".to_string())?;
    let server = value
        .as_object()
        .ok_or_else(|| "native_parse_failed".to_string())?;
    if server.get("name").and_then(Value::as_str) != Some("mmcg") {
        return Err("native_parse_failed".into());
    }
    let transport = server
        .get("transport")
        .and_then(Value::as_object)
        .ok_or_else(|| "native_parse_failed".to_string())?;
    if transport.get("type").and_then(Value::as_str) != Some("stdio") {
        return Err("native_parse_failed".into());
    }
    let command = transport
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| "native_parse_failed".to_string())?
        .to_string();
    let args = transport
        .get("args")
        .and_then(Value::as_array)
        .ok_or_else(|| "native_parse_failed".to_string())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "native_parse_failed".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ParsedNativeState::Codex { command, args })
}

fn native_remove_args(client: Client) -> Vec<String> {
    match client {
        Client::Claude => vec![
            "mcp".into(),
            "remove".into(),
            "mmcg".into(),
            "-s".into(),
            "user".into(),
        ],
        Client::Codex => vec!["mcp".into(), "remove".into(), "mmcg".into()],
        _ => Vec::new(),
    }
}

fn native_add_args(client: Client, entry: &Value) -> Vec<String> {
    let mut args = match client {
        Client::Claude => vec![
            "mcp".into(),
            "add".into(),
            "--scope".into(),
            "user".into(),
            "mmcg".into(),
            "--".into(),
        ],
        Client::Codex => vec!["mcp".into(), "add".into(), "mmcg".into(), "--".into()],
        _ => Vec::new(),
    };
    if let Some(command) = entry.get("command").and_then(Value::as_str) {
        args.push(command.into());
    }
    if let Some(entry_args) = entry.get("args").and_then(Value::as_array) {
        args.extend(
            entry_args
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string),
        );
    }
    args
}

fn parse_json_unique(bytes: &[u8]) -> Result<Value, String> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueValue::deserialize(&mut deserializer)
        .map_err(|_| "invalid_or_duplicate_json".to_string())?;
    deserializer
        .end()
        .map_err(|_| "invalid_or_duplicate_json".to_string())?;
    Ok(value.0)
}

struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> de::Visitor<'de> for UniqueVisitor {
    type Value = UniqueValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value.into())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: de::SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element::<UniqueValue>()? {
            values.push(value.0);
        }
        Ok(UniqueValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: de::MapAccess<'de>,
    {
        let mut values = BTreeMap::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate object key"));
            }
            let value = map.next_value::<UniqueValue>()?;
            values.insert(key, value.0);
        }
        Ok(UniqueValue(Value::Object(values.into_iter().collect())))
    }
}

fn read_capped(path: &Path) -> Result<Option<Vec<u8>>, String> {
    read_config_capped(path)
}

pub(crate) fn read_config_capped(path: &Path) -> Result<Option<Vec<u8>>, String> {
    ensure_safe_target(path)?;
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("config_read_failed".into()),
    };
    if !metadata.is_file() {
        return Err("config_not_regular".into());
    }
    if metadata.len() > CONFIG_MAX_BYTES as u64 {
        return Err("config_too_large".into());
    }
    let file = std::fs::File::open(path).map_err(|_| "config_read_failed".to_string())?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(CONFIG_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "config_read_failed".to_string())?;
    if bytes.len() > CONFIG_MAX_BYTES {
        return Err("config_too_large".into());
    }
    Ok(Some(bytes))
}

fn redact_entry(entry: &Value) -> Value {
    let mut redacted = serde_json::Map::new();
    if let Some(command) = entry.get("command").and_then(Value::as_str) {
        redacted.insert("command".into(), Value::String(command.into()));
    }
    let arg_count = entry
        .get("args")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    redacted.insert("arg_count".into(), json!(arg_count));
    if let Some(fields) = entry.as_object() {
        for key in fields
            .keys()
            .filter(|key| *key != "command" && *key != "args")
        {
            redacted.insert(key.clone(), Value::String("<redacted>".into()));
        }
    }
    Value::Object(redacted)
}

fn safe_replace(path: &Path, observed: Option<&[u8]>, body: &[u8]) -> Result<(), String> {
    ensure_safe_target(path)?;
    let parent = path.parent().ok_or_else(|| "invalid_target".to_string())?;
    std::fs::create_dir_all(parent).map_err(|_| "parent_create_failed".to_string())?;
    ensure_safe_target(path)?;

    static NEXT_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file_name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new("config"))
        .to_string_lossy();
    let temp = parent.join(format!(
        ".{file_name}.mastermind-{}-{id}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp)
            .map_err(|_| "temp_create_failed".to_string())?;
        if let Ok(metadata) = std::fs::metadata(path) {
            file.set_permissions(metadata.permissions())
                .map_err(|_| "mode_preserve_failed".to_string())?;
        }
        file.write_all(body)
            .map_err(|_| "temp_write_failed".to_string())?;
        file.sync_all()
            .map_err(|_| "temp_sync_failed".to_string())?;
        drop(file);

        let current = match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("config_recheck_failed".into()),
        };
        let unchanged = match (observed, current.as_deref()) {
            (None, None) => true,
            (Some(before), Some(now)) => before == now,
            _ => false,
        };
        if !unchanged {
            return Err("config_changed_concurrently".into());
        }
        atomic_replace(&temp, path)?;
        sync_parent(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn safe_remove(path: &Path, observed: Option<&[u8]>) -> Result<(), String> {
    ensure_safe_target(path)?;
    let current = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err("config_recheck_failed".into()),
    };
    let unchanged = match (observed, current.as_deref()) {
        (None, None) => true,
        (Some(before), Some(now)) => before == now,
        _ => false,
    };
    if !unchanged {
        return Err("config_changed_concurrently".into());
    }
    std::fs::remove_file(path).map_err(|_| "config_remove_failed".to_string())?;
    sync_parent(path.parent().ok_or_else(|| "invalid_target".to_string())?)
}

fn backup_private(path: &Path, body: &[u8]) -> Result<PathBuf, String> {
    let home = setup_home_dir().ok_or_else(|| "home_directory_unavailable".to_string())?;
    let directory = home.join(".mastermind/setup-backups");
    std::fs::create_dir_all(&directory).map_err(|_| "backup_directory_failed".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "backup_directory_mode_failed".to_string())?;
    }
    static NEXT_BACKUP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT_BACKUP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let source = path
        .file_name()
        .unwrap_or_else(|| OsStr::new("config"))
        .to_string_lossy();
    let backup = directory.join(format!("{source}-{}-{id}.bak", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&backup)
        .map_err(|_| "backup_create_failed".to_string())?;
    file.write_all(body)
        .map_err(|_| "backup_write_failed".to_string())?;
    file.sync_all()
        .map_err(|_| "backup_sync_failed".to_string())?;
    Ok(backup)
}

fn ensure_safe_target(path: &Path) -> Result<(), String> {
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err("parent_traversal_rejected".into());
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| "current_directory_unavailable".to_string())?
            .join(path)
    };
    let mut current = PathBuf::new();
    for component in absolute.components() {
        current.push(component.as_os_str());
        // A Windows verbatim/UNC prefix is not a path that can be inspected
        // until its root and at least one normal component have been joined.
        // Filesystem roots cannot themselves be symlinks on either platform.
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("symlink_target_rejected".into())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("target_inspection_failed".into()),
        }
    }
    Ok(())
}

#[cfg(windows)]
fn atomic_replace(temp: &Path, path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let temp: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: both pointers reference NUL-terminated buffers that remain alive
    // for the duration of the call. The paths were constructed by this module.
    let replaced = unsafe {
        MoveFileExW(
            temp.as_ptr(),
            path.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        return Err("atomic_replace_failed".into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn atomic_replace(temp: &Path, path: &Path) -> Result<(), String> {
    std::fs::rename(temp, path).map_err(|_| "atomic_replace_failed".to_string())
}

#[cfg(test)]
thread_local! {
    static TEST_NATIVE_BIN: std::cell::RefCell<Option<PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
    static TEST_HOME_DIR: std::cell::RefCell<Option<PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(all(test, unix))]
struct TestNativeBinGuard {
    previous: Option<PathBuf>,
}

#[cfg(all(test, unix))]
impl TestNativeBinGuard {
    fn new(directory: PathBuf) -> Self {
        let previous = TEST_NATIVE_BIN.with(|slot| slot.replace(Some(directory)));
        Self { previous }
    }
}

#[cfg(all(test, unix))]
impl Drop for TestNativeBinGuard {
    fn drop(&mut self) {
        TEST_NATIVE_BIN.with(|slot| {
            slot.replace(self.previous.take());
        });
    }
}

#[cfg(test)]
struct TestHomeDirGuard {
    previous: Option<PathBuf>,
}

#[cfg(test)]
impl TestHomeDirGuard {
    fn new(directory: PathBuf) -> Self {
        let previous = TEST_HOME_DIR.with(|slot| slot.replace(Some(directory)));
        Self { previous }
    }
}

#[cfg(test)]
impl Drop for TestHomeDirGuard {
    fn drop(&mut self) {
        TEST_HOME_DIR.with(|slot| {
            slot.replace(self.previous.take());
        });
    }
}

/// Name every non-absolute `PATH` entry, or `None` when all of them are safe.
///
/// An empty entry is the dangerous case and the common one: POSIX reads it as
/// the current directory, so the resolved binary would depend on where the
/// command was run from.
pub(crate) fn describe_unsafe_path_entries(path: &std::ffi::OsStr) -> Option<String> {
    let mut offenders = Vec::new();
    for (index, entry) in std::env::split_paths(path).enumerate() {
        if entry.is_absolute() {
            continue;
        }
        offenders.push(if entry.as_os_str().is_empty() {
            format!("#{index} is empty, which POSIX reads as the current directory")
        } else {
            format!("#{index} is relative: {}", entry.display())
        });
    }
    if offenders.is_empty() {
        return None;
    }
    Some(format!(
        "PATH entry {}. A binary resolved through it depends on the working directory, \
so setup refuses to record the command. An empty entry is usually a stray colon in a \
shell profile — fix PATH and retry.",
        offenders.join("; ")
    ))
}

fn error_hint(class: &str) -> Option<String> {
    match class {
        "unsafe_path_entry" => {
            describe_unsafe_path_entries(&std::env::var_os("PATH").unwrap_or_default())
        }
        "path_unavailable" => Some("PATH is not set in this environment".to_string()),
        _ => None,
    }
}

fn resolve_native(name: &str, root: &Path) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|_| "root_resolution_failed".to_string())?;
    #[cfg(test)]
    if let Some(directory) = TEST_NATIVE_BIN.with(|slot| slot.borrow().clone()) {
        return resolve_native_from_directories(name, &root, [directory]);
    }
    let path = std::env::var_os("PATH").ok_or_else(|| "path_unavailable".to_string())?;
    let entries: Vec<PathBuf> = std::env::split_paths(&path).collect();
    if entries.is_empty() || entries.iter().any(|entry| !entry.is_absolute()) {
        return Err("unsafe_path_entry".into());
    }
    resolve_native_from_directories(name, &root, entries)
}

fn resolve_native_from_directories(
    name: &str,
    root: &Path,
    directories: impl IntoIterator<Item = PathBuf>,
) -> Result<PathBuf, String> {
    for directory in directories {
        if !directory.is_absolute() {
            return Err("unsafe_path_entry".into());
        }
        #[cfg(not(windows))]
        let candidate = directory.join(name);
        #[cfg(windows)]
        let candidate = {
            let plain = directory.join(name);
            if plain.exists() {
                plain
            } else {
                directory.join(format!("{name}.exe"))
            }
        };
        if !candidate.is_file() {
            continue;
        }
        let candidate = candidate
            .canonicalize()
            .map_err(|_| "native_resolution_failed".to_string())?;
        if candidate.starts_with(root) {
            return Err("repository_native_rejected".into());
        }
        return Ok(candidate);
    }
    Err("native_cli_not_found".into())
}

fn run_bounded(program: &Path, args: &[String]) -> Result<BoundedOutput, String> {
    run_bounded_with_timeout(program, args, PROCESS_TIMEOUT)
}

fn run_bounded_with_timeout(
    program: &Path,
    args: &[String],
    timeout: Duration,
) -> Result<BoundedOutput, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "native_spawn_failed".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "native_pipe_failed".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "native_pipe_failed".to_string())?;
    let (stdout_sender, stdout_receiver) = std::sync::mpsc::sync_channel(1);
    let (stderr_sender, stderr_receiver) = std::sync::mpsc::sync_channel(1);
    let stdout_reader = std::thread::spawn(move || {
        let _ = stdout_sender.send(drain_bounded(stdout));
    });
    let stderr_reader = std::thread::spawn(move || {
        let _ = stderr_sender.send(drain_bounded(stderr));
    });
    let started = Instant::now();
    let mut status = None;
    let mut stdout_result = None;
    let mut stderr_result = None;
    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(completed)) => status = Some(completed),
                Ok(None) => {}
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("native_wait_failed".into());
                }
            }
        }
        if stdout_result.is_none() {
            stdout_result = channel_result(&stdout_receiver);
        }
        if stderr_result.is_none() {
            stderr_result = channel_result(&stderr_receiver);
        }
        match (status.take(), stdout_result.take(), stderr_result.take()) {
            (Some(status), Some(stdout), Some(stderr)) => {
                stdout_reader
                    .join()
                    .map_err(|_| "native_reader_failed".to_string())?;
                stderr_reader
                    .join()
                    .map_err(|_| "native_reader_failed".to_string())?;
                let (stdout, stdout_truncated) = stdout?;
                let (stderr, stderr_truncated) = stderr?;
                #[cfg(not(all(test, unix)))]
                let _ = stderr;
                return Ok(BoundedOutput {
                    status,
                    stdout,
                    #[cfg(all(test, unix))]
                    stderr,
                    stdout_truncated,
                    stderr_truncated,
                });
            }
            (pending_status, pending_stdout, pending_stderr) => {
                status = pending_status;
                stdout_result = pending_stdout;
                stderr_result = pending_stderr;
            }
        }
        if started.elapsed() >= timeout {
            if status.is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
            if stdout_result.is_none() {
                stdout_result = stdout_receiver
                    .recv_timeout(Duration::from_millis(100))
                    .ok();
            }
            if stderr_result.is_none() {
                stderr_result = stderr_receiver
                    .recv_timeout(Duration::from_millis(100))
                    .ok();
            }
            if stdout_result.is_some() {
                let _ = stdout_reader.join();
            }
            if stderr_result.is_some() {
                let _ = stderr_reader.join();
            }
            return Err("native_timeout".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn channel_result(
    receiver: &std::sync::mpsc::Receiver<Result<(Vec<u8>, bool), String>>,
) -> Option<Result<(Vec<u8>, bool), String>> {
    match receiver.try_recv() {
        Ok(result) => Some(result),
        Err(std::sync::mpsc::TryRecvError::Empty) => None,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            Some(Err("native_reader_failed".into()))
        }
    }
}

fn drain_bounded<R: Read>(mut reader: R) -> Result<(Vec<u8>, bool), String> {
    let mut retained = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| "native_read_failed".to_string())?;
        if read == 0 {
            break;
        }
        let remaining = PROCESS_OUTPUT_MAX_BYTES.saturating_sub(retained.len());
        let keep = remaining.min(read);
        retained.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    Ok((retained, truncated))
}

fn sync_parent(_parent: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::fs::File::open(_parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "parent_sync_failed".to_string())?;
    }
    Ok(())
}

fn render_plan_summary(
    request: &Request,
    target: &str,
    action: &str,
    entry: &Value,
    outcome: Outcome,
) -> String {
    format!(
        "client={} scope={} target={} action={} outcome={} command={}",
        client_label(request.client),
        scope_label(request.scope),
        target,
        action,
        outcome_label(outcome),
        serde_json::to_string(&redact_entry(entry)).unwrap_or_else(|_| "{}".into())
    )
}

fn print_plan_summary(
    request: &Request,
    target: &str,
    action: &str,
    entry: &Value,
    outcome: Outcome,
) {
    println!(
        "{}",
        render_plan_summary(request, target, action, entry, outcome)
    );
}

fn finish_outcome(
    request: &Request,
    target: &str,
    action: &str,
    entry: &Value,
    outcome: Outcome,
) -> Outcome {
    print_plan_summary(request, target, action, entry, outcome);
    outcome
}

fn finish_error(
    request: &Request,
    target: &str,
    action: &str,
    entry: &Value,
    class: &str,
) -> Outcome {
    print_plan_summary(request, target, action, entry, Outcome::Error);
    eprintln!("setup error: {class}");
    if let Some(hint) = error_hint(class) {
        eprintln!("  {hint}");
    }
    Outcome::Error
}

fn operation(request: &Request) -> &'static str {
    if request.remove {
        "remove"
    } else {
        "install"
    }
}

fn outcome_label(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::DryRun => "dry_run",
        Outcome::Wrote => "wrote",
        Outcome::NoChange => "no_change",
        Outcome::RefusedOverwrite => "refused",
        Outcome::Error => "error",
    }
}

fn client_label(client: Client) -> &'static str {
    match client {
        Client::Claude => "claude",
        Client::Cursor => "cursor",
        Client::Codex => "codex",
        Client::Continue => "continue",
        Client::Generic => "generic",
    }
}

fn scope_label(scope: Scope) -> &'static str {
    match scope {
        Scope::Project => "project",
        Scope::User => "user",
    }
}

/// Line diff: common prefix/suffix, marks only the changed window with up to 3
/// context lines. Assumes a single contiguous change region (safe here — the
/// merge is always additive on one key). Use `similar` if multi-region diffs are
/// ever needed.
#[cfg(test)]
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

/// Compatibility wrapper for callers that do not need conflict metadata.
#[allow(dead_code)]
fn write_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    safe_replace(path, None, body.as_bytes()).map_err(std::io::Error::other)
}

#[cfg(test)]
mod path_entry_tests {
    use super::describe_unsafe_path_entries;
    use std::ffi::OsString;

    // `split_paths` and `is_absolute` are both platform-defined, so the fixtures
    // have to be too: `;` and a drive letter on Windows, `:` and a leading slash
    // elsewhere. Hard-coding POSIX syntax passes locally and fails the matrix.
    #[cfg(not(windows))]
    const SEP: &str = ":";
    #[cfg(windows)]
    const SEP: &str = ";";

    #[cfg(not(windows))]
    const ABS_A: &str = "/usr/bin";
    #[cfg(windows)]
    const ABS_A: &str = r"C:\Windows";

    #[cfg(not(windows))]
    const ABS_B: &str = "/bin";
    #[cfg(windows)]
    const ABS_B: &str = r"C:\Windows\System32";

    #[cfg(not(windows))]
    const REL: &str = "node_modules/.bin";
    #[cfg(windows)]
    const REL: &str = r"node_modules\.bin";

    fn describe(entries: &[&str]) -> Option<String> {
        describe_unsafe_path_entries(&OsString::from(entries.join(SEP)))
    }

    #[test]
    fn absolute_entries_are_safe() {
        assert!(describe(&[ABS_A, ABS_B]).is_none());
    }

    #[test]
    fn an_empty_entry_is_named_as_the_current_directory() {
        let detail = describe(&[ABS_A, "", ABS_B]).expect("empty entry must be reported");
        assert!(detail.contains("#1"), "{detail}");
        assert!(detail.contains("current directory"), "{detail}");

        // A trailing separator is the same defect and the most common spelling of it.
        let trailing = describe(&[ABS_A, ""]).expect("trailing separator must be reported");
        assert!(trailing.contains("current directory"), "{trailing}");
    }

    #[test]
    fn a_relative_entry_is_named_with_its_value() {
        let detail = describe(&[ABS_A, REL]).expect("relative entry must be reported");
        assert!(detail.contains("#1"), "{detail}");
        assert!(detail.contains(REL), "{detail}");
        assert!(!detail.contains("current directory"), "{detail}");
    }

    #[test]
    fn every_offender_is_listed_not_just_the_first() {
        let detail = describe(&[ABS_A, REL, "", ABS_B]).expect("multiple offenders");
        assert!(detail.contains("#1"), "{detail}");
        assert!(detail.contains("#2"), "{detail}");
    }

    #[test]
    fn an_unset_path_reads_as_a_single_current_directory_entry() {
        // `split_paths("")` yields one empty entry rather than none, so an unset
        // PATH is the current-directory case and not a separate one.
        let detail = describe(&[""]).expect("an empty PATH must still be reported");
        assert!(detail.contains("#0"), "{detail}");
        assert!(detail.contains("current directory"), "{detail}");
    }
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
        p.canonicalize().unwrap()
    }

    #[cfg(unix)]
    fn successful_status() -> std::process::ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(0)
    }

    #[cfg(windows)]
    fn successful_status() -> std::process::ExitStatus {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(0)
    }

    fn bounded_stdout(stdout: &[u8], truncated: bool) -> BoundedOutput {
        BoundedOutput {
            status: successful_status(),
            stdout: stdout.to_vec(),
            #[cfg(unix)]
            stderr: Vec::new(),
            stdout_truncated: truncated,
            stderr_truncated: false,
        }
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
        let dir = tmp("force_overwrites");
        let _home = TestHomeDirGuard::new(dir.clone());
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

    #[test]
    fn client_scope_matrix_rejects_invalid_combinations_before_io() {
        let root = tmp("matrix").join("does-not-exist");
        let invalid = [
            Request {
                client: Client::Codex,
                scope: Scope::Project,
                root: root.clone(),
                config: None,
                write: true,
                remove: false,
                force: false,
            },
            Request {
                client: Client::Generic,
                scope: Scope::User,
                root: root.clone(),
                config: None,
                write: true,
                remove: false,
                force: false,
            },
            Request {
                client: Client::Cursor,
                scope: Scope::Project,
                root: root.clone(),
                config: Some(root.join("config.json")),
                write: true,
                remove: false,
                force: false,
            },
        ];
        for request in invalid {
            assert_eq!(run(&request, Path::new("/bin/mmcg")), Outcome::Error);
        }
        assert!(!root.exists());
    }

    #[test]
    fn json_setup_preserves_unrelated_servers_and_rejects_duplicate_keys() {
        let root = tmp("json-merge");
        let config = root.join("config.json");
        fs::write(
            &config,
            r#"{"other":{"kept":true},"mcpServers":{"other":{"command":"other"}}}"#,
        )
        .unwrap();
        let request = Request {
            client: Client::Generic,
            scope: Scope::Project,
            root: root.clone(),
            config: Some(config.clone()),
            write: true,
            remove: false,
            force: false,
        };
        assert_eq!(run(&request, Path::new("/bin/mmcg")), Outcome::Wrote);
        let value: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
        assert_eq!(value.pointer("/other/kept"), Some(&json!(true)));
        assert!(value.pointer("/mcpServers/other").is_some());
        assert!(value.pointer("/mcpServers/mmcg").is_some());

        fs::write(&config, br#"{"mcpServers":{},"nested":{"x":1,"x":2}}"#).unwrap();
        assert_eq!(run(&request, Path::new("/bin/mmcg")), Outcome::Error);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn dry_run_and_errors_never_render_existing_secrets() {
        let secret = "never-render-this-secret";
        let entry = json!({
            "command": "/bin/mmcg",
            "args": ["serve"],
            "env": {"TOKEN": secret},
            "unknown": secret,
        });
        let rendered = serde_json::to_string(&redact_entry(&entry)).unwrap();
        assert!(!rendered.contains(secret));
        assert!(rendered.contains("<redacted>"));
        let error =
            parse_json_unique(br#"{"secret":"never-render-this-secret","x":1,"x":2}"#).unwrap_err();
        assert!(!error.contains(secret));
    }

    #[test]
    fn every_outcome_prints_redacted_plan_summary() {
        let request = Request {
            client: Client::Continue,
            scope: Scope::User,
            root: PathBuf::from("/trusted/root"),
            config: None,
            write: false,
            remove: false,
            force: false,
        };
        let secret = "summary-secret";
        let entry = json!({
            "command": "/trusted/mmcg",
            "args": ["serve"],
            "env": {"TOKEN": secret},
        });
        for (outcome, label) in [
            (Outcome::DryRun, "dry_run"),
            (Outcome::Wrote, "wrote"),
            (Outcome::NoChange, "no_change"),
            (Outcome::RefusedOverwrite, "refused"),
            (Outcome::Error, "error"),
        ] {
            let summary = render_plan_summary(&request, "user-config", "install", &entry, outcome);
            assert!(summary.contains("client=continue"));
            assert!(summary.contains("scope=user"));
            assert!(summary.contains("target=user-config"));
            assert!(summary.contains("action=install"));
            assert!(summary.contains(&format!("outcome={label}")));
            assert!(summary.contains("<redacted>"));
            assert!(!summary.contains(secret));
        }
    }

    #[test]
    fn continue_removes_only_owned_shape_and_force_backs_up_customized_content() {
        let root = tmp("continue-owned");
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        let _home = TestHomeDirGuard::new(home.clone());
        let target = root.join(".continue/mcpServers/mastermind.yaml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "schema: 1\nowner: someone-else\nsecret: hidden\n").unwrap();
        let mut request = Request {
            client: Client::Continue,
            scope: Scope::Project,
            root: root.clone(),
            config: None,
            write: true,
            remove: true,
            force: false,
        };
        assert_eq!(
            run(&request, Path::new("/bin/mmcg")),
            Outcome::RefusedOverwrite
        );
        assert!(target.exists());
        request.force = true;
        request.remove = false;
        assert_eq!(run(&request, Path::new("/bin/mmcg")), Outcome::Wrote);
        let backups = fs::read_dir(home.join(".mastermind/setup-backups"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(backups.len(), 1);
        assert!(fs::read_to_string(backups[0].path())
            .unwrap()
            .contains("hidden"));
        request.force = false;
        request.remove = true;
        assert_eq!(run(&request, Path::new("/bin/mmcg")), Outcome::Wrote);
        assert!(!target.exists());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn read_capped_rejects_input_over_two_mib() {
        let root = tmp("read-capped-limit");
        let target = root.join("config.json");
        fs::write(&target, vec![b'x'; CONFIG_MAX_BYTES + 1]).unwrap();
        assert_eq!(read_capped(&target).unwrap_err(), "config_too_large");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn safe_replace_rejects_symlinked_parent() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let root = tmp("safe-replace-parent-link");
            let real_parent = root.join("real");
            let linked_parent = root.join("linked");
            fs::create_dir_all(&real_parent).unwrap();
            symlink(&real_parent, &linked_parent).unwrap();
            let target = linked_parent.join("config.json");
            assert_eq!(
                safe_replace(&target, None, b"body").unwrap_err(),
                "symlink_target_rejected"
            );
            assert!(!real_parent.join("config.json").exists());
            fs::remove_dir_all(root).ok();
        }
    }

    #[test]
    fn safe_replace_preserves_unix_mode_and_cleans_unique_temp() {
        let root = tmp("safe-replace-mode");
        let target = root.join("config.json");
        fs::write(&target, b"before").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        }
        let observed = fs::read(&target).unwrap();
        safe_replace(&target, Some(&observed), b"after").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
        let stale = fs::read(&target).unwrap();
        fs::write(&target, b"changed").unwrap();
        assert_eq!(
            safe_replace(&target, Some(&stale), b"rejected").unwrap_err(),
            "config_changed_concurrently"
        );
        let temp_prefix = ".config.json.mastermind-";
        let temp_files = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(temp_prefix))
            .count();
        assert_eq!(temp_files, 0);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn private_backup_directory_and_file_modes_are_restrictive() {
        let root = tmp("private-backup-modes");
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        let _home = TestHomeDirGuard::new(home.clone());
        let backup = backup_private(&root.join("config.json"), b"private").unwrap();
        assert_eq!(fs::read(&backup).unwrap(), b"private");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let directory_mode = fs::metadata(backup.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            let file_mode = fs::metadata(&backup).unwrap().permissions().mode() & 0o777;
            assert_eq!(directory_mode, 0o700);
            assert_eq!(file_mode, 0o600);
        }
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn safe_replace_rejects_symlinks_and_detects_lost_update() {
        let root = tmp("safe-replace");
        let target = root.join("config.json");
        fs::write(&target, b"before").unwrap();
        let observed = fs::read(&target).unwrap();
        fs::write(&target, b"changed").unwrap();
        assert_eq!(
            safe_replace(&target, Some(&observed), b"after").unwrap_err(),
            "config_changed_concurrently"
        );
        assert_eq!(fs::read(&target).unwrap(), b"changed");

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = root.join("link.json");
            symlink(&target, &link).unwrap();
            assert_eq!(
                safe_replace(&link, Some(b"changed"), b"after").unwrap_err(),
                "symlink_target_rejected"
            );
        }
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn native_matching_is_structural_not_substring_based() {
        let entry = json!({"command": "/bin/mmcg", "args": ["serve"]});
        let claude = bounded_stdout(b"Command: /bin/mmcg\nArgs: serve\n", false);
        assert!(matches!(
            native_matches(Client::Claude, &claude, &entry).unwrap(),
            NativeState::Canonical(_)
        ));
        let claude_superset =
            bounded_stdout(b"Command: /bin/mmcg-custom\nArgs: serve --extra\n", false);
        assert!(matches!(
            native_matches(Client::Claude, &claude_superset, &entry).unwrap(),
            NativeState::Customized(_)
        ));

        let codex = bounded_stdout(
            br#"{"name":"mmcg","transport":{"type":"stdio","command":"/bin/mmcg","args":["serve"]}}"#,
            false,
        );
        assert!(matches!(
            native_matches(Client::Codex, &codex, &entry).unwrap(),
            NativeState::Canonical(_)
        ));
        let codex_superset = bounded_stdout(
            br#"{"name":"mmcg","transport":{"type":"stdio","command":"/bin/mmcg-custom","args":["serve","--extra"]}}"#,
            false,
        );
        assert!(matches!(
            native_matches(Client::Codex, &codex_superset, &entry).unwrap(),
            NativeState::Customized(_)
        ));
    }

    #[test]
    fn native_state_rejects_truncation_and_detects_tail_change() {
        let entry = json!({"command": "/bin/mmcg", "args": ["serve"]});
        let truncated = bounded_stdout(b"Command: /bin/mmcg\nArgs: serve\n", true);
        assert_eq!(
            native_matches(Client::Claude, &truncated, &entry).unwrap_err(),
            "native_output_truncated"
        );

        let first = native_matches(
            Client::Claude,
            &bounded_stdout(b"Command: /bin/mmcg\nArgs: serve\n", false),
            &entry,
        )
        .unwrap();
        let changed = native_matches(
            Client::Claude,
            &bounded_stdout(b"Command: /bin/mmcg\nArgs: serve --tail\n", false),
            &entry,
        )
        .unwrap();
        assert_ne!(first, changed);
    }

    #[test]
    fn native_runner_preserves_argv_without_shell_interpolation() {
        #[cfg(unix)]
        {
            let root = tmp("native-argv");
            let script = root.join("capture");
            let output = root.join("argv");
            let marker = root.join("interpolated");
            fs::write(
                &script,
                "#!/bin/sh\nout=$1\nshift\nprintf '%s\\n' \"$@\" > \"$out\"\n",
            )
            .unwrap();
            let args = vec![
                output.to_string_lossy().into_owned(),
                format!("; touch {}", marker.display()),
                format!("$(touch {})", marker.display()),
                "space value".into(),
            ];
            let mut shell_args = vec![script.to_string_lossy().into_owned()];
            shell_args.extend(args.clone());
            assert!(run_bounded(Path::new("/bin/sh"), &shell_args)
                .unwrap()
                .status
                .success());
            assert_eq!(
                fs::read_to_string(output)
                    .unwrap()
                    .lines()
                    .collect::<Vec<_>>(),
                args[1..]
            );
            assert!(!marker.exists());
            fs::remove_dir_all(root).ok();
        }
    }

    #[test]
    fn native_executable_identity_change_is_rejected_before_mutation() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let root = tmp("native-identity");
            let script = root.join("native");
            let replacement = root.join("replacement");
            let marker = root.join("mutated");
            let body = "#!/bin/sh\n: > \"$1\"\n";
            fs::write(&script, body).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
            let identity = ExecutableIdentity::capture(&script).unwrap();
            fs::write(&replacement, body).unwrap();
            fs::set_permissions(&replacement, fs::Permissions::from_mode(0o700)).unwrap();
            fs::rename(replacement, &script).unwrap();
            assert_eq!(
                run_native_checked(&script, &identity, &[marker.to_string_lossy().into_owned()])
                    .unwrap_err(),
                "native_executable_changed"
            );
            assert!(!marker.exists());
            fs::remove_dir_all(root).ok();
        }
    }

    #[test]
    fn native_timeout_returns_when_descendant_holds_pipes_open() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let root = tmp("native-descendant-timeout");
            let script = root.join("descendant");
            fs::write(&script, "#!/bin/sh\nsleep 5 &\nexit 0\n").unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
            let started = Instant::now();
            assert_eq!(
                run_bounded_with_timeout(&script, &[], Duration::from_millis(50)).unwrap_err(),
                "native_timeout"
            );
            assert!(started.elapsed() < Duration::from_secs(1));
            fs::remove_dir_all(root).ok();
        }
    }

    #[test]
    fn native_runner_bounds_output_and_kills_timeout() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let root = tmp("native-bounded");
            let noisy = root.join("noisy");
            fs::write(
                &noisy,
                "#!/bin/sh\nhead -c 70000 /dev/zero\nhead -c 70000 /dev/zero >&2\n",
            )
            .unwrap();
            fs::set_permissions(&noisy, fs::Permissions::from_mode(0o700)).unwrap();
            let output = run_bounded(&noisy, &[]).unwrap();
            assert_eq!(output.stdout.len(), PROCESS_OUTPUT_MAX_BYTES);
            assert_eq!(output.stderr.len(), PROCESS_OUTPUT_MAX_BYTES);
            assert!(output.stdout_truncated && output.stderr_truncated);

            let hanging = root.join("hanging");
            fs::write(&hanging, "#!/bin/sh\nwhile :; do :; done\n").unwrap();
            fs::set_permissions(&hanging, fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(
                run_bounded_with_timeout(&hanging, &[], Duration::from_millis(50)).unwrap_err(),
                "native_timeout"
            );
            fs::remove_dir_all(root).ok();
        }
    }

    #[test]
    fn all_setup_clients_are_idempotent() {
        let _canonical_entry = TestCanonicalEntryGuard::new(json!({
            "command": "/bin/mmcg",
            "args": ["serve"],
        }));
        let root = tmp("all-idempotent");
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        let _home = TestHomeDirGuard::new(home);
        let mmcg = Path::new("/bin/mmcg");
        let requests = [
            Request {
                client: Client::Claude,
                scope: Scope::Project,
                root: root.join("claude"),
                config: None,
                write: true,
                remove: false,
                force: false,
            },
            Request {
                client: Client::Cursor,
                scope: Scope::Project,
                root: root.join("cursor"),
                config: None,
                write: true,
                remove: false,
                force: false,
            },
            Request {
                client: Client::Cursor,
                scope: Scope::User,
                root: root.join("cursor-user"),
                config: None,
                write: true,
                remove: false,
                force: false,
            },
            Request {
                client: Client::Continue,
                scope: Scope::Project,
                root: root.join("continue"),
                config: None,
                write: true,
                remove: false,
                force: false,
            },
            Request {
                client: Client::Continue,
                scope: Scope::User,
                root: root.join("continue-user"),
                config: None,
                write: true,
                remove: false,
                force: false,
            },
            Request {
                client: Client::Generic,
                scope: Scope::Project,
                root: root.clone(),
                config: Some(root.join("generic/config.json")),
                write: true,
                remove: false,
                force: false,
            },
            Request {
                client: Client::Generic,
                scope: Scope::User,
                root: root.clone(),
                config: Some(root.join("generic-user/config.json")),
                write: true,
                remove: false,
                force: false,
            },
        ];
        for request in requests {
            assert_eq!(run(&request, mmcg), Outcome::Wrote);
            assert_eq!(run(&request, mmcg), Outcome::NoChange);
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let native_root = tmp("native-request-root");
            let bin = root.join("native-bin");
            fs::create_dir_all(&bin).unwrap();
            for name in ["claude", "codex"] {
                let executable = bin.join(name);
                let script = if name == "claude" {
                    "#!/bin/sh\nSTATE_FILE=\"$0.state\"\nif [ \"$2\" = \"get\" ]; then [ -f \"$STATE_FILE\" ] || exit 1; STATE=\"\"; read -r STATE < \"$STATE_FILE\" || :; [ \"$STATE\" = \"removed\" ] && exit 1; printf 'Command: /bin/mmcg\\nArgs: serve\\n'; exit 0; fi\nif [ \"$2\" = \"add\" ]; then : > \"$STATE_FILE\"; exit 0; fi\nif [ \"$2\" = \"remove\" ]; then printf 'removed\\n' > \"$STATE_FILE\"; exit 0; fi\nexit 1\n"
                } else {
                    "#!/bin/sh\nSTATE_FILE=\"$0.state\"\nif [ \"$2\" = \"get\" ]; then [ \"$4\" = \"--json\" ] || exit 2; [ -f \"$STATE_FILE\" ] || exit 1; STATE=\"\"; read -r STATE < \"$STATE_FILE\" || :; [ \"$STATE\" = \"removed\" ] && exit 1; printf '%s\\n' '{\"name\":\"mmcg\",\"enabled\":true,\"transport\":{\"type\":\"stdio\",\"command\":\"/bin/mmcg\",\"args\":[\"serve\"],\"env\":null,\"env_vars\":[],\"cwd\":null}}'; exit 0; fi\nif [ \"$2\" = \"add\" ]; then : > \"$STATE_FILE\"; exit 0; fi\nif [ \"$2\" = \"remove\" ]; then printf 'removed\\n' > \"$STATE_FILE\"; exit 0; fi\nexit 1\n"
                };
                fs::write(&executable, script).unwrap();
                fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            }
            for client in [Client::Claude, Client::Codex] {
                let name = client_label(client);
                let state = bin.join(format!("{name}.state"));
                let _native_bin = TestNativeBinGuard::new(bin.clone());
                let request = Request {
                    client,
                    scope: Scope::User,
                    root: native_root.clone(),
                    config: None,
                    write: true,
                    remove: false,
                    force: false,
                };
                assert_eq!(run(&request, mmcg), Outcome::Wrote);
                assert_eq!(run(&request, mmcg), Outcome::NoChange);
                assert!(state.is_file());
            }
            fs::remove_dir_all(native_root).ok();
        }
        fs::remove_dir_all(root).ok();
    }
}
