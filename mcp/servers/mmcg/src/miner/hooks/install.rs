//! Project-local native hook configuration. Setup never grants client trust.

use crate::bounded_fs::{
    self, AtomicWriteExpectation, BoundedReadError, ReadControl, RootCapability, StableFileIdentity,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::path::{Path, PathBuf};

const CONFIG_MAX_BYTES: u64 = 1024 * 1024;
const COMMON_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "SessionEnd",
    "PreCompact",
    "SubagentStart",
    "SubagentStop",
];

struct Snapshot {
    root: RootCapability,
    target: PathBuf,
    bytes: Vec<u8>,
    identity: StableFileIdentity,
}

/// Prepare or apply the native project hook configuration. The caller owns the
/// independent capture grant; writing config does not approve profile claims.
pub(super) fn configure(
    client: &str,
    project_root: &Path,
    write: bool,
    remove: bool,
) -> Result<Value, Box<dyn Error>> {
    if !cfg!(unix) {
        return Err("native hook command installation currently requires a Unix platform".into());
    }
    let (relative, extra_event, documentation) = match client {
        "claude" => (
            ".claude/settings.local.json",
            "PostToolUseFailure",
            "https://code.claude.com/docs/en/hooks",
        ),
        "codex" => (
            ".codex/hooks.json",
            "Interrupt",
            "https://learn.chatgpt.com/docs/hooks",
        ),
        _ => return Err("hook client must be claude or codex".into()),
    };
    let project = project_root.canonicalize()?;
    if !project.is_dir() {
        return Err("hook project root must be an existing directory".into());
    }
    let project_text = project
        .to_str()
        .ok_or("hook project root must be valid UTF-8")?;
    let path = project.join(relative);
    let observed = read_snapshot(&path)?;
    let original = observed
        .as_ref()
        .map(|snapshot| crate::setup::parse_json_unique(&snapshot.bytes))
        .transpose()?
        .unwrap_or_else(|| json!({}));
    let mut config = original.clone();
    let executable = std::env::current_exe()?.canonicalize()?;
    let executable = executable
        .to_str()
        .ok_or("hook executable path must be valid UTF-8")?;
    let registration = format!(
        "# mastermind-persona-hook-v1:{client}:{}",
        crate::hex::encode(&Sha256::digest(project_text.as_bytes()))
    );
    let command = format!(
        "{} miner hooks receive --client {} --project-root {} {registration}",
        shell_quote(executable),
        shell_quote(client),
        shell_quote(project_text)
    );
    let handler = json!({
        "type": "command",
        "command": command,
        "timeout": 3,
        "statusMessage": "Saving Mastermind interaction evidence",
    });
    let mut events = COMMON_EVENTS.to_vec();
    events.push(extra_event);
    let mut generated = serde_json::Map::new();
    for event in &events {
        generated.insert(
            (*event).to_string(),
            json!([{ "hooks": [handler.clone()] }]),
        );
    }
    merge(&mut config, &registration, &generated, remove)?;
    let changed = config != original;
    let mut bytes = serde_json::to_vec_pretty(&config)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > CONFIG_MAX_BYTES {
        return Err("native hook configuration exceeds the 1 MiB limit".into());
    }
    if write && changed {
        replace(&path, observed.as_ref(), &bytes)?;
    }
    let limits: &[&str] = if client == "codex" {
        &[
            "Capture starts only after the project layer and exact hook definitions are trusted in Codex /hooks.",
            "Hosted tools, opted-out tool paths, and write_stdin inputs have incomplete native hook coverage.",
            "SessionEnd and Interrupt do not cover subagents; no PostToolUseFailure event is documented.",
            "Existing hooks in other active config layers also run; this installer does not modify them.",
        ]
    } else {
        &[
            "The client must load project-local settings and allow these hooks; managed settings can disable hooks.",
            "Stop does not fire on user interruption; no general Interrupt event is documented.",
            "Existing hooks in other active config layers also run; this installer does not modify them.",
        ]
    };
    Ok(json!({
        "schema": 1,
        "client": client,
        "scope": "project",
        "project_root": project,
        "config_path": path,
        "operation": if remove { "remove" } else { "install" },
        "changed": changed,
        "written": write && changed,
        "events": events,
        "hook_config": if remove { json!({}) } else { json!({"hooks": generated}) },
        "requires_client_trust": client == "codex" && !remove,
        "client_activation": if remove {
            "Reload the client configuration; capture revocation is handled separately."
        } else if client == "codex" {
            "Open /hooks in a trusted Codex project and review the generated definitions before enabling capture."
        } else {
            "Reload the client configuration and inspect /hooks in Claude Code."
        },
        "local_hooks_disabled": original.get("disableAllHooks").and_then(Value::as_bool) == Some(true),
        "platform": "unix",
        "coverage": "native_events_only",
        "purpose": "local mining capture; these hooks do not enforce action permissions",
        "limits": limits,
        "documentation": documentation,
    }))
}

fn merge(
    config: &mut Value,
    registration: &str,
    generated: &serde_json::Map<String, Value>,
    remove: bool,
) -> Result<(), Box<dyn Error>> {
    let object = config
        .as_object_mut()
        .ok_or("native hook configuration must be a JSON object")?;
    if !object.contains_key("hooks") {
        if remove {
            return Ok(());
        }
        object.insert("hooks".into(), json!({}));
    }
    let hooks = object
        .get_mut("hooks")
        .and_then(Value::as_object_mut)
        .ok_or("native hooks must be a JSON object")?;
    let mut empty_events = Vec::new();
    let mut removed_owned_handler = false;
    for (event, groups) in hooks.iter_mut() {
        let groups = groups
            .as_array_mut()
            .ok_or("native hook event must contain an array of matcher groups")?;
        let mut empty_groups = Vec::new();
        let mut removed_from_event = false;
        for (index, group) in groups.iter_mut().enumerate() {
            let group = group
                .as_object_mut()
                .ok_or("native hook matcher group must be an object")?;
            let handlers = group
                .get_mut("hooks")
                .and_then(Value::as_array_mut)
                .ok_or("native hook matcher group must contain a hooks array")?;
            if handlers.iter().any(|handler| !handler.is_object()) {
                return Err("native hook handler must be an object".into());
            }
            let before = handlers.len();
            handlers.retain(|handler| {
                !(handler.get("type").and_then(Value::as_str) == Some("command")
                    && handler
                        .get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|command| command.ends_with(registration)))
            });
            removed_from_event |= before != handlers.len();
            if before != handlers.len() && handlers.is_empty() && group.len() == 1 {
                empty_groups.push(index);
            }
        }
        for index in empty_groups.into_iter().rev() {
            groups.remove(index);
        }
        removed_owned_handler |= removed_from_event;
        if groups.is_empty() && removed_from_event {
            empty_events.push(event.clone());
        }
    }
    for event in empty_events {
        hooks.remove(&event);
    }
    if !remove {
        for (event, groups) in generated {
            hooks
                .entry(event.clone())
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .ok_or("native hook event must contain an array")?
                .extend(groups.as_array().expect("generated hook groups").clone());
        }
    }
    if hooks.is_empty() && removed_owned_handler {
        object.remove("hooks");
    }
    Ok(())
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

fn read_snapshot(path: &Path) -> Result<Option<Snapshot>, Box<dyn Error>> {
    let (root, target) = match bounded_fs::open_file_target(path) {
        Ok(value) => value,
        Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => {
            return Err(format!("cannot open native hook configuration: {error:?}").into())
        }
    };
    match bounded_fs::read_regular_file_with_capability(
        &root,
        &target,
        CONFIG_MAX_BYTES,
        CONFIG_MAX_BYTES,
        ReadControl::default(),
    ) {
        Ok(file) => Ok(Some(Snapshot {
            root,
            target,
            bytes: file.bytes,
            identity: file.identity,
        })),
        Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(error) => Err(format!("cannot read native hook configuration: {error:?}").into()),
    }
}

fn replace(path: &Path, observed: Option<&Snapshot>, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    if let Some(observed) = observed {
        let current = bounded_fs::read_regular_file_expected(
            &observed.root,
            &observed.target,
            CONFIG_MAX_BYTES,
            CONFIG_MAX_BYTES,
            ReadControl::default(),
            Some(observed.identity),
        )
        .map_err(|error| format!("native hook configuration changed: {error:?}"))?;
        if current.bytes != observed.bytes {
            return Err("native hook configuration changed concurrently".into());
        }
        #[cfg(unix)]
        let mode = observed.identity.attributes() as u32 & 0o777;
        #[cfg(not(unix))]
        let mode = 0o600;
        return bounded_fs::write_atomic_regular_file_expected_with_capability_mode(
            &observed.root,
            &observed.target,
            bytes,
            mode,
            AtomicWriteExpectation::File(observed.identity),
        )
        .map_err(|error| format!("native hook configuration write failed: {error:?}").into());
    }
    let (root, target) = bounded_fs::prepare_file_target(path)
        .map_err(|error| format!("cannot prepare native hook configuration: {error:?}"))?;
    let missing = bounded_fs::inspect_absent_path(&root, &target, ReadControl::default())
        .map_err(|error| format!("cannot inspect native hook configuration: {error:?}"))?
        .ok_or("native hook configuration changed concurrently")?;
    bounded_fs::write_atomic_regular_file_expected_with_capability_mode(
        &root,
        &target,
        bytes,
        0o600,
        AtomicWriteExpectation::Missing(missing),
    )
    .map_err(|error| format!("native hook configuration write failed: {error:?}").into())
}
