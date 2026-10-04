//! Native inference with the captured client's model and existing login.
//! Authentication stays in the native client. No credentials are copied.

use super::{semantic, Error};
use serde_json::Value;
use std::path::Path;

pub(super) const VERSION: &str = "persona-native-processor-v2";

pub(super) fn provider<'a>(requested: &'a str, client: &'a str) -> Result<&'a str, Error> {
    super::client(client)?;
    match requested {
        "native" => Ok(client),
        "claude" | "codex" if requested == client => Ok(client),
        "claude" | "codex" => Err("native provider must match the captured client".into()),
        _ => Err("supported native providers: native, claude, codex".into()),
    }
}

fn validate_model(model: &str) -> Result<(), Error> {
    if model.is_empty()
        || model.len() > 128
        || !model.as_bytes()[0].is_ascii_alphanumeric()
        || !model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-:/[]".contains(&b))
        || super::super::feedback::looks_secret(model)
    {
        return Err("native model identifier is invalid".into());
    }
    Ok(())
}

pub(super) fn hook_model(value: &Value, kind: &str) -> Result<Option<String>, Error> {
    let key = if kind == "PostModelSwitch" {
        "to_model"
    } else {
        "model"
    };
    match value.get(key).filter(|value| !value.is_null()) {
        None => Ok(None),
        Some(value) => {
            let model = value.as_str().ok_or("native model must be a string")?;
            validate_model(model)?;
            Ok(Some(model.into()))
        }
    }
}

/// Claude versions that omit `model` in hooks still record the actual model in
/// the assistant response. Bind it to this Stop, session and project instead of
/// guessing from config or a previous response.
pub(super) fn transcript_model(
    value: &Value,
    root: &Path,
) -> Result<Option<(String, Value)>, Error> {
    use crate::bounded_fs::{self, ReadControl};
    use serde_json::json;
    use std::time::{Duration, Instant};

    let Some(session) = value["session_id"]
        .as_str()
        .and_then(super::super::feedback::valid_session_id)
    else {
        return Ok(None);
    };
    let Some(last) = value["last_assistant_message"]
        .as_str()
        .filter(|text| !text.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(selected) = value["transcript_path"]
        .as_str()
        .map(Path::new)
        .filter(|path| path.is_absolute())
    else {
        return Ok(None);
    };
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".claude")))
        .ok_or("Claude account directory is unavailable")?;
    if !config.is_absolute() {
        return Ok(None);
    }
    let Some(cwd) = value["cwd"]
        .as_str()
        .map(Path::new)
        .filter(|cwd| cwd.is_absolute())
    else {
        return Ok(None);
    };
    let canonical_cwd = cwd.canonicalize()?;
    if !canonical_cwd.starts_with(root) {
        return Ok(None);
    }
    let dir = config
        .join("projects")
        .join(super::super::feedback::claude_project_slug(cwd));
    let path = dir.join(format!("{session}.jsonl"));
    if selected.file_name() != path.file_name()
        || selected
            .parent()
            .ok_or("transcript parent missing")?
            .canonicalize()?
            != dir.canonicalize()?
    {
        return Ok(None);
    }
    const TAIL: u64 = 512 * 1024;
    let control = ReadControl {
        deadline: Some(Instant::now() + Duration::from_millis(250)),
        interrupted: None,
    };
    let file = bounded_fs::read_regular_file_tail(&dir, &path, 256 * 1024 * 1024, TAIL, control)?;
    let bytes = if file.declared_len > TAIL {
        let Some(newline) = file.bytes.iter().position(|byte| *byte == b'\n') else {
            return Ok(None);
        };
        &file.bytes[newline + 1..]
    } else {
        &file.bytes
    };
    let text = std::str::from_utf8(bytes)?;
    for line in text.lines().rev() {
        control.check()?;
        if line.trim().is_empty() {
            continue;
        }
        if line.len() > 128 * 1024 {
            return Ok(None);
        }
        let Ok(record) = crate::setup::parse_json_unique(line.as_bytes()) else {
            return Ok(None);
        };
        if record["type"] != "assistant" {
            continue;
        }
        // The latest assistant record must match. Never search older answers
        // after a conflicting latest record, even if their text happens to match.
        if record["sessionId"] != session
            || record["isSidechain"] == true
            || record["message"]["role"] != "assistant"
            || record["cwd"]
                .as_str()
                .and_then(|cwd| Path::new(cwd).canonicalize().ok())
                .as_deref()
                != Some(canonical_cwd.as_path())
        {
            return Ok(None);
        }
        let Some(blocks) = record["message"]["content"].as_array() else {
            return Ok(None);
        };
        let response = blocks
            .iter()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if response.trim() != last.trim() {
            return Ok(None);
        }
        let Some(model) = record["message"]["model"].as_str() else {
            return Ok(None);
        };
        validate_model(model)?;
        return Ok(Some((
            model.into(),
            json!({"source":"native_session_transcript",
            "record_digest":super::hash(&record),"session_id":session}),
        )));
    }
    Ok(None)
}

pub(super) fn analyze(
    input: &semantic::EpisodeInput,
    requested: &str,
    timeout: u64,
) -> Result<Vec<semantic::SemanticDraft>, Error> {
    let request = semantic::request(input)?;
    let output = run(
        request,
        semantic::INSTRUCTIONS,
        Path::new(&input.project_root),
        requested,
        &input.client,
        input.model.as_deref(),
        timeout,
    )?;
    semantic::parse_response(input, &output)
}

pub(super) fn run(
    request: Vec<u8>,
    instructions: &str,
    root: &Path,
    requested: &str,
    client: &str,
    model: Option<&str>,
    timeout: u64,
) -> Result<Vec<u8>, Error> {
    let selected = provider(requested, client)?;
    let model = model.ok_or("native model was not captured; start a new native session")?;
    validate_model(model)?;
    let executable = crate::setup::resolve_native_cli(selected, root)
        .map_err(|_| "native mining client is unavailable")?;
    run_with_executable(&executable, selected, model, request, instructions, timeout)
}

fn run_with_executable(
    executable: &Path,
    client: &str,
    model: &str,
    request: Vec<u8>,
    instructions: &str,
    timeout: u64,
) -> Result<Vec<u8>, Error> {
    if !(1..=300).contains(&timeout) || request.len() > 512 * 1024 {
        return Err("native mining request or timeout exceeds its bound".into());
    }
    let isolated = tempfile::Builder::new()
        .prefix("mastermind-native-miner-")
        .tempdir()?;
    let mut environment = vec![];
    if client == "codex" {
        let account = std::env::var_os("CODEX_HOME")
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::home_dir().map(|home| home.join(".codex")))
            .ok_or("native Codex account directory is unavailable")?;
        if !account.is_absolute() {
            return Err("native Codex account directory must be absolute".into());
        }
        // Native auth keeps its existing account directory. A private HOME
        // prevents discovery of unrelated ~/.agents skills and instructions.
        environment.push(("CODEX_HOME".into(), account.into_os_string()));
        environment.push((
            "HOME".into(),
            isolated.path().canonicalize()?.into_os_string(),
        ));
    }
    let args = if client == "claude" {
        claude_args(model, instructions)
    } else {
        // The account still uses CODEX_HOME. User configuration, hooks,
        // project instructions, skills, plugins and memory are excluded.
        let instructions_file = isolated.path().join("instructions.md");
        std::fs::write(&instructions_file, instructions)?;
        codex_args(model, &instructions_file)?
    };
    let output = semantic::run_processor_with_env(
        executable,
        &args,
        request,
        timeout,
        Some(isolated.path()),
        &environment,
    )?;
    if client == "claude" {
        claude_result(&output, model)
    } else {
        codex_result(&output)
    }
}

fn claude_args(model: &str, instructions: &str) -> Vec<String> {
    [
        "-p",
        "--safe-mode",
        "--model",
        model,
        "--input-format",
        "text",
        "--output-format",
        "json",
        "--max-turns",
        "1",
        "--tools",
        "",
        "--strict-mcp-config",
        "--mcp-config",
        "{\"mcpServers\":{}}",
        "--no-session-persistence",
        "--disable-slash-commands",
        "--no-chrome",
        "--permission-mode",
        "dontAsk",
        "--system-prompt",
        instructions,
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn codex_args(model: &str, instructions: &Path) -> Result<Vec<String>, Error> {
    let instructions = instructions
        .to_str()
        .ok_or("native instructions path is not UTF-8")?;
    let file = format!(
        "model_instructions_file={}",
        serde_json::to_string(instructions)?
    );
    Ok([
        "exec", "--ephemeral", "--ignore-user-config", "--ignore-rules",
        "--skip-git-repo-check", "--sandbox", "read-only", "--model", model,
        "--json", "--color", "never", "-c", &file,
        "-c", "project_doc_max_bytes=0", "-c", "web_search=\"disabled\"",
        "-c", "approval_policy=\"never\"", "-c", "tools.update_plan.enabled=false",
        "-c", "features={hooks=false,plugins=false,apps=false,multi_agent=false,shell_tool=false,unified_exec=false,memories=false,view_image=false,goals=false,sleep_tool=false,browser_use=false,computer_use=false,image_generation=false,workspace_dependencies=false,code_mode_host=false,skill_mcp_dependency_install=false,skip_host_skill_discovery=true,shell_snapshot=false}",
        "-",
    ].into_iter().map(String::from).collect())
}

fn claude_result(output: &[u8], model: &str) -> Result<Vec<u8>, Error> {
    let value = crate::setup::parse_json_unique(output)
        .map_err(|_| "native Claude returned an invalid result envelope")?;
    if value["type"] != "result" || value["is_error"] != false {
        return Err("native Claude did not complete inference".into());
    }
    // A fallback must not certify that the task's model performed the mining.
    let usage = value["modelUsage"]
        .as_object()
        .ok_or("native Claude model usage is absent")?;
    if usage.len() != 1 || !usage.contains_key(model) {
        return Err("native Claude used a different model".into());
    }
    value["result"]
        .as_str()
        .map(|result| result.as_bytes().to_vec())
        .ok_or_else(|| "native Claude text result is absent".into())
}

fn codex_result(output: &[u8]) -> Result<Vec<u8>, Error> {
    let mut result = None;
    let mut completed = false;
    for line in output
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
    {
        let event = crate::setup::parse_json_unique(line)
            .map_err(|_| "native Codex returned invalid JSONL")?;
        match event["type"].as_str() {
            Some("thread.started" | "turn.started") if !completed => {}
            Some("item.started" | "item.updated" | "item.completed") if !completed => {
                match event["item"]["type"].as_str() {
                    // Catalog warnings are nonfatal diagnostics, not tool
                    // executions. turn.failed/error still fail the request.
                    Some("reasoning" | "error") => {}
                    Some("agent_message") => {
                        if event["type"] == "item.completed" {
                            result = Some(
                                event["item"]["text"]
                                    .as_str()
                                    .ok_or("native Codex message text is absent")?
                                    .as_bytes()
                                    .to_vec(),
                            );
                        }
                    }
                    _ => return Err("native Codex attempted a tool during inference".into()),
                }
            }
            Some("turn.completed") if !completed => completed = true,
            _ => return Err("native Codex did not complete isolated inference".into()),
        }
    }
    if !completed {
        return Err("native Codex inference is incomplete".into());
    }
    result.ok_or_else(|| "native Codex final text is absent".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn routing_requires_the_source_client_and_model() {
        for client in ["codex", "claude"] {
            assert_eq!(provider("native", client).unwrap(), client);
            assert_eq!(provider(client, client).unwrap(), client);
        }
        assert!(provider("claude", "codex").is_err());
        assert!(provider("codex", "claude").is_err());
        assert!(hook_model(&json!({"model":"--help"}), "Stop").is_err());
        assert_eq!(
            hook_model(&json!({"to_model":"claude-opus-5-5"}), "PostModelSwitch")
                .unwrap()
                .as_deref(),
            Some("claude-opus-5-5")
        );
    }

    #[test]
    fn native_results_reject_fallbacks_tools_and_partial_turns() {
        let result = json!({"type":"result","is_error":false,"result":"{}","modelUsage":{"claude-opus-5-5":{}}});
        assert_eq!(
            claude_result(result.to_string().as_bytes(), "claude-opus-5-5").unwrap(),
            b"{}"
        );
        assert!(claude_result(result.to_string().as_bytes(), "claude-sonnet-5").is_err());
        let message = json!({"type":"item.completed","item":{"type":"agent_message","text":"{}"}});
        let done = json!({"type":"turn.completed"});
        assert_eq!(
            codex_result(format!("{message}\n{done}\n").as_bytes()).unwrap(),
            b"{}"
        );
        assert!(codex_result(message.to_string().as_bytes()).is_err());
        let tool = json!({"type":"item.completed","item":{"type":"command_execution"}});
        assert!(codex_result(format!("{tool}\n{message}\n{done}\n").as_bytes()).is_err());
    }
}
