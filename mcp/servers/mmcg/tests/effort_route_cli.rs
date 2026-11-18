//! Public effort routing, bounded JSON inputs and model-free native launches.

#[cfg(unix)]
#[path = "../src/test_executable.rs"]
mod test_executable;

use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Output, Stdio};

fn context() -> Value {
    json!({
        "agent": "researcher", "task_family": "task_phase_trace",
        "read_only": true, "bounded_scope": true, "stable_inputs": true,
        "lexical_repo_map": true, "risk": "low"
    })
}

fn input(command: &mut Command, bytes: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    child.wait_with_output().unwrap()
}

fn command(root: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mmcg"));
    command.current_dir(root).arg("effort-route");
    command
}

#[test]
fn stdin_and_file_select_the_same_route_without_creating_an_index() {
    let root = tempfile::tempdir().unwrap();
    let mut bytes = serde_json::to_vec(&context()).unwrap();
    bytes.resize(16 * 1024, b' ');
    let from_stdin = input(&mut command(root.path()), &bytes);
    assert!(from_stdin.status.success());
    let path = root.path().join("route.json");
    std::fs::write(&path, bytes).unwrap();
    let from_file = command(root.path())
        .arg("--request")
        .arg(path)
        .output()
        .unwrap();
    assert!(from_file.status.success());
    let decision: Value = serde_json::from_slice(&from_stdin.stdout).unwrap();
    assert_eq!(decision["requested_effort"], "low");
    assert_eq!(
        decision["codex_spawn_args"],
        json!({"fork_turns": "none", "reasoning_effort": "low"})
    );
    assert_eq!(from_file.stdout, from_stdin.stdout);
    assert!(!root.path().join(".mastermind").exists());
}

#[test]
fn malformed_oversized_and_multibyte_name_inputs_fail_without_a_route() {
    let root = tempfile::tempdir().unwrap();
    let mut unicode = context();
    unicode["agent"] = json!("я".repeat(41));
    for bytes in [
        b"{bad json}".to_vec(),
        serde_json::to_vec(&unicode).unwrap(),
        vec![b' '; 16 * 1024 + 1],
    ] {
        let output = input(&mut command(root.path()), &bytes);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    let missing_file = command(root.path())
        .args(["--exec", "codex"])
        .output()
        .unwrap();
    assert!(!missing_file.status.success());
    assert!(missing_file.stdout.is_empty());
}

#[cfg(unix)]
fn fake_client(root: &std::path::Path, client: &str) {
    test_executable::write_executable(
        root.join(client),
        "#!/bin/sh\n/usr/bin/touch ./spawned\nprintf '%s\\n' \"$@\"\n/bin/cat\nexit \"${FAKE_EXIT_CODE:-0}\"\n",
    );
}

#[cfg(unix)]
#[test]
fn native_launch_submits_effort_and_preserves_args_stdin_and_failure_status() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("route.json");
    let mut request = context();
    request["user_effort"] = json!("max");
    std::fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    for client in ["codex", "claude"] {
        fake_client(root.path(), client);
        let mut command = command(root.path());
        command
            .arg("--request")
            .arg(&path)
            .args(["--exec", client, "--"])
            .args([
                "--model",
                "chosen-model",
                "--",
                "--effort",
                "$(touch injected)",
            ])
            .env("PATH", root.path())
            .env_remove("CLAUDE_CODE_EFFORT_LEVEL")
            .env("FAKE_EXIT_CODE", "23");
        let output = input(&mut command, b"prompt from stdin\n");
        assert_eq!(output.status.code(), Some(23));
        let prefix = match client {
            "codex" => "-c\nmodel_reasoning_effort=\"max\"\n",
            _ => "--effort\nmax\n",
        };
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("{prefix}--model\nchosen-model\n--\n--effort\n$(touch injected)\nprompt from stdin\n")
        );
        let record: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(record["override_submitted"], true);
        assert_eq!(record["effort_route"]["reason"], "explicit_user_effort");
        assert_eq!(record["effort_route"]["applied"], false);
        assert!(record["served_effort"].is_null());
        assert!(!root.path().join("injected").exists());
    }
}

#[cfg(unix)]
#[test]
fn conflicting_native_overrides_are_rejected_before_spawn() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("route.json");
    std::fs::write(&path, serde_json::to_vec(&context()).unwrap()).unwrap();
    for (client, args) in [
        ("codex", vec!["-c", "model_reasoning_effort='high'"]),
        ("codex", vec!["--config", "'model_reasoning_effort'='high'"]),
        ("codex", vec!["-cmodel_reasoning_effort='high'"]),
        ("codex", vec!["-c=model_reasoning_effort='high'"]),
        ("codex", vec!["--config=model_reasoning_effort='high'"]),
        ("claude", vec!["--effort", "high"]),
        ("claude", vec!["--effort=high"]),
        ("claude", vec![]),
    ] {
        fake_client(root.path(), client);
        let output = command(root.path())
            .arg("--request")
            .arg(&path)
            .args(["--exec", client, "--"])
            .args(args)
            .env("PATH", root.path())
            .env("CLAUDE_CODE_EFFORT_LEVEL", "high")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!root.path().join("spawned").exists());
    }
}
