use serde_json::{json, Value};
use std::fs::{self, FileTimes};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, SystemTime};

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository with spaces");
        fs::create_dir(&root).unwrap();
        fs::write(root.join(".gitignore"), ".mastermind/\n").unwrap();
        for args in [
            vec!["init", "-q", "--initial-branch=main"],
            vec!["config", "user.name", "Managed index test"],
            vec!["config", "user.email", "index@example.invalid"],
            vec!["config", "core.autocrlf", "false"],
            vec!["config", "core.hooksPath", ""],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            assert_success(
                &Command::new("git")
                    .current_dir(&root)
                    .args(args)
                    .output()
                    .unwrap(),
            );
        }
        let fixture = Self {
            _directory: directory,
            root,
        };
        fixture.source("original_refresh", 0);
        for args in [vec!["add", "."], vec!["commit", "-q", "-m", "baseline"]] {
            assert_success(
                &Command::new("git")
                    .current_dir(&fixture.root)
                    .args(args)
                    .output()
                    .unwrap(),
            );
        }
        fixture
    }

    fn source(&self, name: &str, revision: u64) {
        let path = self.root.join("probe.py");
        fs::write(&path, format!("def {name}():\n    return 1\n")).unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + revision),
            ))
            .unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mmcg"));
        command
            .current_dir(&self.root)
            .env("MMCG_QUERY_BUDGET_MS", "60000");
        command
    }

    fn index(&self, custom: Option<&Path>) {
        let mut command = self.command();
        if let Some(path) = custom {
            command.arg("--index").arg(path);
        }
        assert_success(&command.args(["index", "."]).output().unwrap());
    }

    fn search(&self, custom: Option<&Path>) -> Value {
        let mut command = self.command();
        if let Some(path) = custom {
            command.arg("--index").arg(path);
        }
        command.arg("serve");
        if custom.is_some() {
            command.arg("--root").arg(&self.root);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        for message in [
            json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
                "protocolVersion":"2025-11-25","capabilities":{},
                "clientInfo":{"name":"managed-index-test","version":"1"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"mmcg_search","arguments":{"name":"updated_refresh"}}}),
        ] {
            writeln!(input, "{message}").unwrap();
        }
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert_success(&output);
        let response = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|response| response["id"] == 1)
            .unwrap();
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn default_mcp_startup_refreshes_the_repository_index() {
    let fixture = Fixture::new();
    fixture.index(None);
    fixture.source("updated_refresh", 1);

    let result = fixture.search(None);

    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(result["structuredContent"]["count"], 1, "{result}");
    assert_eq!(
        result["structuredContent"]["results"][0]["name"],
        "updated_refresh"
    );
}

#[test]
fn bound_explicit_repository_index_refreshes_after_source_change() {
    let fixture = Fixture::new();
    fixture.index(None);
    fixture.source("updated_refresh", 1);

    let result = fixture.search(Some(&fixture.root.join(".mastermind/mmcg.db")));

    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(result["structuredContent"]["count"], 1, "{result}");
}

#[test]
fn default_source_review_commands_refresh_the_repository_index() {
    for args in [
        vec!["concept", "updated refresh", "--format", "json"],
        vec!["concept-review", "prepare", "updated refresh"],
    ] {
        let fixture = Fixture::new();
        fixture.index(None);
        fixture.source("updated_refresh", 1);

        let output = fixture.command().args(&args).output().unwrap();
        assert_success(&output);
        let output = fixture
            .command()
            .args(["query", "search", "updated_refresh"])
            .output()
            .unwrap();
        assert_success(&output);
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["count"], 1, "{args:?}: {response}");
    }
}

#[test]
fn explicit_custom_index_refuses_refresh_without_changing_database_bytes() {
    let fixture = Fixture::new();
    let custom = fixture.root.join(".mastermind/custom.db");
    fixture.index(Some(&custom));
    let before = fs::read(&custom).unwrap();
    fixture.source("updated_refresh", 1);

    let result = fixture.search(Some(&custom));

    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(result["structuredContent"]["code"], "index_stale");
    assert_eq!(result["structuredContent"]["refresh_attempted"], false);
    assert_eq!(fs::read(&custom).unwrap(), before);
}
