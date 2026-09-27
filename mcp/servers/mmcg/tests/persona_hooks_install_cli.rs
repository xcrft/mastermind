//! Native hook setup is exercised only in isolated projects and homes.
#![cfg(unix)]

use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project's $literal `name`");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&project).unwrap();
        Self {
            _temp: temp,
            home,
            project: project.canonicalize().unwrap(),
        }
    }

    fn run(&self, client: &str, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mmcg"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CODEX_HOME", self.home.join("custom-codex"))
            .args([
                "miner",
                "hooks",
                "setup",
                "--client",
                client,
                "--project-root",
                self.project.to_str().unwrap(),
            ])
            .args(extra)
            .output()
            .unwrap()
    }

    fn success(&self, client: &str, extra: &[&str]) -> Value {
        let output = self.run(client, extra);
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn config(&self, client: &str) -> PathBuf {
        self.project.join(match client {
            "claude" => ".claude/settings.local.json",
            "codex" => ".codex/hooks.json",
            _ => unreachable!(),
        })
    }
}

#[test]
fn dry_run_and_missing_removal_do_not_create_configuration() {
    let fixture = Fixture::new();
    for client in ["claude", "codex"] {
        let result = fixture.success(client, &[]);
        assert_eq!(result["written"], false);
        assert_eq!(result["client"], client);
        assert!(result["events"].as_array().unwrap().len() >= 9);
        assert!(!fixture.config(client).exists());
        fixture.success(client, &["--remove", "--write"]);
        assert!(!fixture.config(client).exists());
    }
    assert!(!fixture.home.join(".claude").exists());
    assert!(!fixture.home.join("custom-codex").exists());
}

#[test]
fn installation_preserves_other_hooks_is_idempotent_and_removes_only_owned_entries() {
    let fixture = Fixture::new();
    for client in ["claude", "codex"] {
        let path = fixture.config(client);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = json!({
            "description": "Keep this workspace setting",
            "hooks": {
                "PreToolUse": [{"matcher":"Bash", "hooks":[
                    {"type":"command", "command":"printf unrelated", "timeout":9}
                ]}],
                "CustomEvent": [{"hooks":[{"type":"command", "command":"custom"}]}]
            }
        });
        fs::write(&path, serde_json::to_vec_pretty(&original).unwrap()).unwrap();
        let result = fixture.success(client, &["--write"]);
        assert_eq!(result["written"], true);
        assert_eq!(result["scope"], "project");
        let first = fs::read(&path).unwrap();
        let installed: Value = serde_json::from_slice(&first).unwrap();
        assert_eq!(installed["description"], original["description"]);
        assert_eq!(
            installed["hooks"]["PreToolUse"][0],
            original["hooks"]["PreToolUse"][0]
        );
        assert_eq!(
            installed["hooks"]["CustomEvent"],
            original["hooks"]["CustomEvent"]
        );
        assert!(installed["hooks"]["UserPromptSubmit"].is_array());
        if client == "codex" {
            assert!(installed["hooks"]["Interrupt"].is_array());
            assert!(installed["hooks"]["PostToolUseFailure"].is_null());
            assert_eq!(result["requires_client_trust"], true);
        } else {
            assert!(installed["hooks"]["PostToolUseFailure"].is_array());
            assert!(installed["hooks"]["Interrupt"].is_null());
        }
        let again = fixture.success(client, &["--write"]);
        assert_eq!(again["written"], false);
        assert_eq!(fs::read(&path).unwrap(), first);
        fixture.success(client, &["--remove", "--write"]);
        let removed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(removed, original);
        assert_eq!(
            fixture.success(client, &["--remove", "--write"])["written"],
            false
        );
    }
}

#[test]
fn duplicate_or_malformed_configuration_is_rejected_without_replacement() {
    let fixture = Fixture::new();
    let path = fixture.config("claude");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    for bytes in [
        r#"{"hooks":{},"hooks":{"Stop":[]}}"#,
        r#"{"hooks":{"Stop":{"hooks":[]}}}"#,
        r#"{"hooks":{"Stop":[{"hooks":"broken"}]}}"#,
        r#"[]"#,
    ] {
        fs::write(&path, bytes).unwrap();
        let output = fixture.run("claude", &["--write"]);
        assert!(!output.status.success(), "{output:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
    }
}

#[test]
fn setup_rejects_symlinked_configuration_without_touching_target() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let target = fixture.home.join("external.json");
    fs::write(&target, "{}").unwrap();
    let path = fixture.config("codex");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    symlink(&target, &path).unwrap();
    let output = fixture.run("codex", &["--write"]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "{}");
    assert!(fs::symlink_metadata(&path)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn generated_shell_command_keeps_paths_and_metacharacters_as_literal_arguments() {
    let fixture = Fixture::new();
    let result = fixture.success("codex", &[]);
    let command = result["hook_config"]["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    // Exercise the native POSIX shell's argument parsing without starting a
    // client or collecting a real conversation.
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("set -- {command}\nprintf '%s\\n' \"$@\""))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let args = stdout.lines().collect::<Vec<_>>();
    let executable = PathBuf::from(env!("CARGO_BIN_EXE_mmcg"))
        .canonicalize()
        .unwrap();
    assert_eq!(
        args,
        vec![
            executable.to_str().unwrap(),
            "miner",
            "hooks",
            "receive",
            "--client",
            "codex",
            "--project-root",
            fixture.project.to_str().unwrap(),
        ]
    );
}

#[test]
fn no_op_removal_preserves_existing_empty_settings_and_event_arrays() {
    let fixture = Fixture::new();
    let path = fixture.config("codex");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    for bytes in [r#"{"hooks":{}}"#, r#"{"hooks":{"Stop":[]}}"#] {
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            fixture.success("codex", &["--remove", "--write"])["written"],
            false
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
    }
}

#[test]
fn replacement_preserves_file_permissions_and_configuration_size_is_bounded() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let path = fixture.config("claude");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "{}").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    fixture.success("claude", &["--write"]);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let oversized = vec![b' '; 1024 * 1024 + 1];
    fs::write(&path, &oversized).unwrap();
    let output = fixture.run("claude", &["--write"]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(fs::read(&path).unwrap(), oversized);
}
