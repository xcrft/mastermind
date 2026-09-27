//! Generated controller input must not enter native hook evidence or cursors.
//! Every home, repository, event and grant is an isolated synthetic fixture.
#![cfg(unix)]

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Seek, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::SystemTime;

const USER_PROMPT: &str = "Before changing an API, check its callers and preserve the contract.";

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&project).unwrap();
        let fixture = Self {
            _temp: temp,
            home,
            project,
        };
        let output = fixture
            .isolated_command("/usr/bin/git")
            .args(["init", "--quiet", "--initial-branch=main"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        fixture
    }

    fn isolated_command(&self, executable: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(executable);
        command
            .current_dir(&self.project)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        command
    }

    fn cli(&self) -> Command {
        self.isolated_command(env!("CARGO_BIN_EXE_mmcg"))
    }

    fn command(&self, args: &[&str]) -> Value {
        parse(self.cli().args(args).output().unwrap())
    }

    fn receive(
        &self,
        client: &str,
        event: &Value,
        controller: bool,
        project_root: &Path,
    ) -> Output {
        // A regular stdin file makes the full native payload available even
        // when admission correctly exits before reading it.
        let mut input = tempfile::tempfile().unwrap();
        input.write_all(event.to_string().as_bytes()).unwrap();
        input.rewind().unwrap();
        let mut command = self.cli();
        command
            .args([
                "miner",
                "hooks",
                "receive",
                "--client",
                client,
                "--project-root",
            ])
            .arg(project_root)
            .stdin(Stdio::from(input));
        if controller {
            command.env("MMCG_INPUT_ORIGIN", "controller");
        }
        command.output().unwrap()
    }

    fn events(&self) -> [Value; 3] {
        let cwd = self.project.canonicalize().unwrap();
        [
            json!({"session_id":"synthetic-session","hook_event_name":"SessionStart","source":"startup","cwd":cwd}),
            json!({"session_id":"synthetic-session","turn_id":"synthetic-turn","hook_event_name":"UserPromptSubmit","prompt":USER_PROMPT,"cwd":cwd}),
            json!({"session_id":"synthetic-session","turn_id":"synthetic-turn","hook_event_name":"Stop","last_assistant_message":"I inspected the callers; the change remains a proposal.","cwd":cwd}),
        ]
    }

    fn snapshot(&self) -> (BTreeMap<PathBuf, Entry>, BTreeMap<PathBuf, Entry>) {
        (snapshot(&self.home), snapshot(&self.project))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    kind: &'static str,
    bytes: Vec<u8>,
    modified: SystemTime,
    mode: u32,
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
    fn walk(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, Entry>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let (kind, bytes) = if metadata.is_dir() {
            ("directory", Vec::new())
        } else if metadata.file_type().is_symlink() {
            use std::os::unix::ffi::OsStrExt;
            (
                "symlink",
                fs::read_link(path).unwrap().as_os_str().as_bytes().to_vec(),
            )
        } else {
            assert!(metadata.is_file(), "unexpected fixture entry: {path:?}");
            ("file", fs::read(path).unwrap())
        };
        output.insert(
            path.strip_prefix(root).unwrap().to_path_buf(),
            Entry {
                kind,
                bytes,
                modified: metadata.modified().unwrap(),
                mode: metadata.permissions().mode(),
            },
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                walk(root, &entry.unwrap().path(), output);
            }
        }
    }
    let mut output = BTreeMap::new();
    walk(root, root, &mut output);
    output
}

fn parse(output: Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error}: {output:?}"))
}

fn assert_controller_skip(output: Output) {
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({})
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap(),
        json!({"status":"skipped","reason":"controller_generated_input"})
    );
}

#[test]
fn controller_origin_skips_native_evidence_without_changing_existing_capture_state() {
    for client in ["claude", "codex"] {
        let fixture = Fixture::new();
        fixture.command(&["miner", "hooks", "setup", "--client", client, "--write"]);
        // Keep a real capture cursor present, so a skip must preserve existing
        // state as well as avoid creating a generated episode or candidate.
        let seed = json!({"session_id":"existing-human-session","hook_event_name":"SessionStart","source":"startup","cwd":fixture.project.canonicalize().unwrap()});
        parse(fixture.receive(client, &seed, false, &fixture.project));
        let before = fixture.snapshot();
        for event in fixture.events() {
            assert_controller_skip(fixture.receive(client, &event, true, &fixture.project));
        }
        assert_eq!(
            fixture.snapshot(),
            before,
            "{client}: admission wrote state"
        );
        let episodes = fixture.command(&["miner", "hooks", "episodes"]);
        assert_eq!(episodes["episodes"], json!([]));
        assert!(!fixture.home.join(".mastermind/style.db").exists());

        // The same bytes, without the provenance marker, retain the normal
        // user-channel capture contract and still require later human review.
        for event in fixture.events() {
            let output = fixture.receive(client, &event, false, &fixture.project);
            assert!(output.stderr.is_empty(), "{output:?}");
            assert_eq!(parse(output), json!({}));
        }
        let episodes = fixture.command(&["miner", "hooks", "episodes"]);
        let episodes = episodes["episodes"].as_array().unwrap();
        assert_eq!(episodes.len(), 1);
        let episode = fixture.command(&[
            "miner",
            "hooks",
            "show",
            episodes[0]["id"].as_str().unwrap(),
        ]);
        assert_eq!(episode["drafts"], json!([]));
        assert!(episode["episode"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| {
                event["text"] == USER_PROMPT && event["origin"] == "user_channel_unverified"
            }));
        assert!(!fixture.home.join(".mastermind/style.db").exists());
    }
}

#[test]
fn controller_skip_precedes_home_journal_and_project_resolution() {
    let fixture = Fixture::new();
    let before = fixture.snapshot();
    let root = fixture.project.join("missing-project-root");
    let event = &fixture.events()[1];
    for client in ["claude", "codex"] {
        assert_controller_skip(fixture.receive(client, event, true, &root));
    }
    assert_eq!(fixture.snapshot(), before);
    assert!(!fixture.home.join(".mastermind").exists());
}
