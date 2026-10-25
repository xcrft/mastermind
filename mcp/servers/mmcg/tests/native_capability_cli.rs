#![cfg(not(unix))]

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TASK: &str = ".mastermind/tasks/001-native-capability";
const SPEC: &str = ".mastermind/tasks/001-native-capability/spec.md";
const STATE: &str = ".mastermind/tasks/001-native-capability/state.json";

struct Fixture {
    directory: tempfile::TempDir,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        fs::create_dir(&home).unwrap();
        let fixture = Self { directory, home };
        for args in [
            vec!["init", "-q", "--initial-branch=main"],
            vec!["config", "user.name", "Native Capability Fixture"],
            vec!["config", "user.email", "capability@example.invalid"],
            vec!["config", "commit.gpgsign", "false"],
            vec!["config", "core.autocrlf", "false"],
            vec!["config", "core.hooksPath", ""],
        ] {
            assert_success(&fixture.command("git").args(args).output().unwrap());
        }
        fixture.write(".gitignore", ".mastermind/\nhome/\n");
        fixture.write("README.md", "# Manual task\nExisting instructions.\n");
        assert_success(&fixture.command("git").args(["add", "."]).output().unwrap());
        assert_success(
            &fixture
                .command("git")
                .args(["commit", "-qm", "Synthetic capability baseline"])
                .output()
                .unwrap(),
        );
        fixture.write(
            SPEC,
            "---\nmode: lite\ntouches:\n  - file: README.md\n    scope: file\n    reason: Update the manual instructions.\n---\n# Manual task\n\n## Goals\nUpdate the existing instructions.\n",
        );
        assert_success(&fixture.run(&["run-task", SPEC, "--pre-only", "--allow-no-index"]));
        assert_eq!(fixture.state()["invocation_required"], false);
        assert!(fixture.state()["preflight_approval_sha256"].is_string());
        fixture
    }

    fn write(&self, path: &str, body: &str) {
        let path = self.directory.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(self.directory.path())
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CODEX_HOME", self.home.join("codex"))
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("GIT_CONFIG_GLOBAL", self.home.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_COUNT", "0")
            .env("GIT_TERMINAL_PROMPT", "0");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_mmcg"))
            .args(args)
            .output()
            .unwrap()
    }

    fn state(&self) -> Value {
        serde_json::from_slice(&fs::read(self.directory.path().join(STATE)).unwrap()).unwrap()
    }

    fn task_files(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn collect(path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    collect(&path, files);
                } else {
                    files.insert(path.clone(), fs::read(path).unwrap());
                }
            }
        }
        let mut files = BTreeMap::new();
        collect(&self.directory.path().join(TASK), &mut files);
        files
    }
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{output:?}");
}

fn assert_unsupported(output: &Output) {
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("invocation_runtime_unsupported_platform"),
        "{output:?}"
    );
}

#[test]
fn unsupported_executor_preserves_manual_approval_and_handoff() {
    let fixture = Fixture::new();
    let before = fixture.task_files();
    let next = fixture.run(&["next"]);
    assert_success(&next);
    assert!(!String::from_utf8_lossy(&next.stdout).contains(" --exec"));
    for options in [
        vec!["--exec"],
        vec!["--exec", "--guarded-exec"],
        vec!["--exec", "--auto-review"],
        vec!["--exec", "--auto-repair"],
    ] {
        let mut args = vec!["run-task", SPEC];
        args.extend(options);
        assert_unsupported(&fixture.run(&args));
        assert_eq!(fixture.task_files(), before, "{args:?}");
        let current_next = fixture.run(&["next"]);
        assert_success(&current_next);
        assert_eq!(current_next.stdout, next.stdout, "{args:?}");
    }
    assert_success(&fixture.run(&["run-task", SPEC, "--exec", "--pre-only", "--allow-no-index"]));
    assert_eq!(fixture.state()["invocation_required"], false);
    assert_eq!(fixture.state()["iteration"], 2);
    assert!(!fixture
        .directory
        .path()
        .join(TASK)
        .join("invocation.json")
        .exists());
}

#[test]
fn unsupported_reviewer_preserves_active_review_and_controller_pins() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    state["status"] = json!("history_review_required");
    state["next_step"] = json!("review_history");
    state["semantic_review_required"] = json!(true);
    state["semantic_review_sha256"] = json!("a".repeat(64));
    state["held_snapshot_sha256"] = json!("b".repeat(64));
    state["history_snapshot_sha256"] = json!("c".repeat(64));
    fixture.write(STATE, &serde_json::to_string_pretty(&state).unwrap());
    fixture.write(
        ".mastermind/tasks/001-native-capability/semantic-review.json",
        "Historical review bytes retained by the task.\n",
    );
    let before = fixture.task_files();
    for args in [
        vec!["review-task", "run", SPEC, "--json"],
        vec!["run-task", SPEC, "--auto-review"],
    ] {
        assert_unsupported(&fixture.run(&args));
        assert_eq!(fixture.task_files(), before, "{args:?}");
    }
}
