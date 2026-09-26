//! Synthetic local histories and an isolated home: never read or publish the
//! developer's real profile when exercising transcript ingestion through CLI.

use mmcg::miner::{
    feedback::claude_project_slug,
    store::{Habit, ProfileStore},
};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "persona_transcripts/source_sync.rs"]
mod source_sync;

#[path = "persona_transcripts/search.rs"]
mod search;

#[cfg(unix)]
#[path = "persona_transcripts/composition.rs"]
mod composition;

const QUOTE: &str = "Check the contract before changing code.";
const BEHAVIOR: &str = "Checks the contract before changing code";

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    codex: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let codex = temp.path().join("custom-codex-home");
        let project = temp.path().join("project-a");
        for dir in [&home, &codex, &project] {
            fs::create_dir(dir).unwrap();
        }
        Self {
            _temp: temp,
            home,
            codex,
            project,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mmcg"))
            .current_dir(&self.project)
            // These child-only settings implement the fixture's home/history
            // locations; the test process and real user environment are untouched.
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CODEX_HOME", &self.codex)
            .args(args)
            .output()
            .unwrap()
    }

    fn success(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert!(out.status.success(), "{out:?}");
        out
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.codex.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    fn records(&self, id: &str) -> Vec<Value> {
        vec![
            json!({"type":"session_meta", "payload":{
                "id":id, "session_id":id, "cwd":self.project,
                "source":"vscode", "thread_source":"user"
            }}),
            json!({"type":"turn_context", "payload":{"turn_id":"turn-1", "cwd":self.project}}),
            json!({"type":"response_item", "timestamp":"2026-09-26T10:00:00Z", "payload":{
                "type":"message", "role":"user", "content":[{"type":"input_text", "text":QUOTE}],
                "internal_chat_message_metadata_passthrough":{
                    "turn_id":"turn-1", "content_item_kinds":["user.text"]
                }
            }}),
        ]
    }

    fn feedback(&self, path: &Path) -> Output {
        self.run(&[
            "miner",
            "feedback",
            "add",
            "--transcript",
            path.to_str().unwrap(),
            "--quote",
            QUOTE,
            "--statement",
            QUOTE,
        ])
    }

    fn propose(&self, path: &Path, project: &Path) -> Output {
        self.run(&[
            "miner",
            "habit",
            "propose",
            "--project-root",
            project.to_str().unwrap(),
            "--transcript",
            path.to_str().unwrap(),
            "--quote",
            QUOTE,
            "--episode",
            "issue-one",
            "--when",
            "When changing a public contract",
            "--behavior",
            BEHAVIOR,
            "--outcome",
            "Keeps the expected behavior explicit",
        ])
    }

    fn db_path(&self) -> PathBuf {
        self.home.join(".mastermind/style.db")
    }

    fn collect(&self, paths: &[&Path], dry_run: bool) -> Output {
        let mut args = vec!["miner", "collect"];
        for path in paths {
            args.extend(["--transcript", path.to_str().unwrap()]);
        }
        if dry_run {
            args.push("--dry-run");
        }
        self.run(&args)
    }

    fn inbox(&self) -> Value {
        let out = self.success(&["miner", "candidates", "list", "--status", "all"]);
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn preferences(&self, id: &str, text: &[&str]) -> String {
        let mut records = self.records(id);
        let template = records.pop().unwrap();
        for text in text {
            let mut msg = template.clone();
            msg["payload"]["content"][0]["text"] = json!(text);
            records.push(msg);
        }
        jsonl(&records)
    }

    fn style(&self) -> String {
        fs::read_to_string(self.home.join(".mastermind/style.md")).unwrap()
    }

    fn propose_collected(&self, candidate: &Value, episode: &str) -> Output {
        self.propose_collected_to(candidate, episode, None)
    }

    fn propose_collected_to(&self, candidate: &Value, episode: &str, habit: Option<i64>) -> Output {
        let mut args = vec![
            "miner",
            "candidates",
            "propose-habit",
            candidate["id"].as_str().unwrap(),
            "--revision",
            candidate["revision"].as_str().unwrap(),
            "--episode",
            episode,
            "--when",
            "When changing a public contract",
            "--behavior",
            BEHAVIOR,
            "--outcome",
            "Keeps the expected behavior explicit",
        ];
        let habit = habit.map(|id| id.to_string());
        if let Some(id) = &habit {
            args.extend(["--habit", id]);
        }
        self.run(&args)
    }

    fn propose_preference(
        &self,
        candidate: &Value,
        statement: &str,
        scope: Option<&str>,
    ) -> Output {
        let mut args = vec![
            "miner",
            "candidates",
            "propose-preference",
            candidate["id"].as_str().unwrap(),
            "--revision",
            candidate["revision"].as_str().unwrap(),
            "--statement",
            statement,
            "--category",
            "communication",
        ];
        if let Some(scope) = scope {
            args.extend(["--scope", scope]);
        }
        self.run(&args)
    }

    fn profile_from_mcp(&self) -> Value {
        use std::io::Write;
        use std::process::Stdio;
        self.success(&["miner", "access", "grant", "--client", "fixture"]);
        let mut child = Command::new(env!("CARGO_BIN_EXE_mmcg"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CODEX_HOME", &self.codex)
            .env("MMCG_PROFILE_CLIENT", "fixture")
            .env_remove("MMCG_INDEX_PATH")
            .args(["serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let request = json!({"jsonrpc":"2.0", "id":1, "method":"tools/call",
            "params":{"name":"mmcg_profile", "arguments":{"paths":["src/main.rs"]}}});
        let init = json!({"jsonrpc":"2.0", "id":0, "method":"initialize", "params":{
            "protocolVersion":"2025-11-25", "capabilities":{}, "clientInfo":{"name":"fixture", "version":"1"}}});
        let ready = json!({"jsonrpc":"2.0", "method":"notifications/initialized"});
        writeln!(child.stdin.take().unwrap(), "{init}\n{ready}\n{request}").unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(result.status.success(), "{result:?}");
        let response = String::from_utf8_lossy(&result.stdout)
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|response| response["id"] == 1)
            .unwrap();
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("missing profile response: {response}"));
        serde_json::from_str(text).unwrap()
    }

    #[cfg(unix)]
    fn strict_preference(
        &self,
        id: &str,
        quote: &str,
        statement: &str,
    ) -> (PathBuf, Value, mmcg::miner::store::Feedback) {
        let path = self.write(
            &format!("sessions/2026/09/26/rollout-{id}.jsonl"),
            &self.preferences(id, &[quote]),
        );
        assert!(self.collect(&[&path], false).status.success());
        let candidate = self.inbox()["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["candidate"]["quote"] == quote)
            .unwrap()["candidate"]
            .clone();
        let proposed = self.propose_preference(&candidate, statement, Some("global"));
        assert!(proposed.status.success(), "{proposed:?}");
        let entry = ProfileStore::open_read_only(&self.db_path())
            .unwrap()
            .feedback()
            .unwrap()
            .into_iter()
            .find(|entry| entry.statement == statement)
            .unwrap();
        (path, candidate, entry)
    }

    #[cfg(unix)]
    fn supersede(
        &self,
        old: &mmcg::miner::store::Feedback,
        new: &mmcg::miner::store::Feedback,
    ) -> Output {
        self.in_fixture_terminal(&[
            "miner",
            "feedback",
            "supersede",
            &old.key,
            "--with",
            &new.key,
            "--old-revision",
            &old.review_revision(),
            "--new-revision",
            &new.review_revision(),
        ])
    }

    #[cfg(unix)]
    fn reviewable_habit(&self) -> (PathBuf, PathBuf) {
        assert!(Command::new("git")
            .args(["init", "-q"])
            .current_dir(&self.project)
            .status()
            .unwrap()
            .success());
        let first = self.write(
            "sessions/2026/09/26/rollout-habit-a.jsonl",
            &jsonl(&self.records("habit-a")),
        );
        let second = self.write(
            "sessions/2026/09/26/rollout-habit-b.jsonl",
            &jsonl(&self.records("habit-b")),
        );
        let proposed = self.propose(&first, &self.project);
        assert!(proposed.status.success(), "{proposed:?}");
        self.success(&[
            "miner",
            "habit",
            "cite",
            "1",
            "--transcript",
            second.to_str().unwrap(),
            "--quote",
            QUOTE,
            "--episode",
            "issue-two",
        ]);
        (first, second)
    }

    #[cfg(unix)]
    fn successor_habit(&self, tag: &str, behavior: &str) -> (Habit, PathBuf, PathBuf) {
        let first = self.write(
            &format!("sessions/2026/09/26/rollout-{tag}-a.jsonl"),
            &jsonl(&self.records(&format!("{tag}-a"))),
        );
        let second = self.write(
            &format!("sessions/2026/09/26/rollout-{tag}-b.jsonl"),
            &jsonl(&self.records(&format!("{tag}-b"))),
        );
        self.success(&[
            "miner",
            "habit",
            "propose",
            "--project-root",
            self.project.to_str().unwrap(),
            "--transcript",
            first.to_str().unwrap(),
            "--quote",
            QUOTE,
            "--episode",
            "issue-one",
            "--when",
            "When changing a public contract",
            "--behavior",
            behavior,
            "--outcome",
            "Keeps the expected behavior explicit",
        ]);
        let current = || {
            ProfileStore::open_read_only(&self.db_path())
                .unwrap()
                .habits()
                .unwrap()
                .into_iter()
                .find(|habit| habit.behavior == behavior)
                .unwrap()
        };
        self.success(&[
            "miner",
            "habit",
            "cite",
            &current().id.to_string(),
            "--transcript",
            second.to_str().unwrap(),
            "--quote",
            QUOTE,
            "--episode",
            "issue-two",
        ]);
        (current(), first, second)
    }

    #[cfg(unix)]
    fn supersede_habit(&self, old: &Habit, new: &Habit) -> Output {
        self.in_fixture_terminal(&[
            "miner",
            "habit",
            "supersede",
            &old.id.to_string(),
            "--with",
            &new.id.to_string(),
            "--old-revision",
            &old.review_revision(),
            "--new-revision",
            &new.review_revision(),
        ])
    }

    #[cfg(unix)]
    fn renew_habit(&self, parent: &Habit) -> Output {
        self.in_fixture_terminal(&[
            "miner",
            "habit",
            "renew",
            &parent.id.to_string(),
            "--revision",
            &parent.review_revision(),
        ])
    }

    #[cfg(unix)]
    fn observe_in_fixture_terminal(&self, id: &str) -> Output {
        let revision = ProfileStore::open_read_only(&self.db_path())
            .unwrap()
            .habit(id.parse().unwrap())
            .unwrap()
            .unwrap()
            .review_revision();
        self.in_fixture_terminal(&["miner", "habit", "observe", id, "--revision", &revision])
    }

    #[cfg(unix)]
    fn in_fixture_terminal(&self, args: &[&str]) -> Output {
        use std::os::fd::FromRawFd;
        use std::process::Stdio;
        let (mut master, mut slave) = (-1, -1);
        // Exercise the actual interactive gate in this isolated synthetic HOME.
        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, 0);
        let _master = unsafe { fs::File::from_raw_fd(master) };
        let slave = unsafe { fs::File::from_raw_fd(slave) };
        Command::new(env!("CARGO_BIN_EXE_mmcg"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CODEX_HOME", &self.codex)
            .stdin(Stdio::from(slave))
            .args(args)
            .output()
            .unwrap()
    }
}

fn jsonl(records: &[Value]) -> String {
    records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[cfg(unix)]
#[test]
fn habit_renewal_requires_review_and_returns_to_identical_description_through_new_evidence() {
    let f = Fixture::new();
    let args = [
        "miner",
        "habit",
        "renew",
        "1",
        "--revision",
        &"a".repeat(64),
    ];
    assert!(!f.run(&args).status.success());
    assert!(!f.in_fixture_terminal(&args).status.success());
    assert!(!f.db_path().exists());
    let (old_path, _) = f.reviewable_habit();
    let db = ProfileStore::open_read_only(&f.db_path()).unwrap();
    let old = db.habit(1).unwrap().unwrap();
    assert!(!f.renew_habit(&old).status.success());
    let (b, _, _) = f.successor_habit("next", "Checks changed contracts only");
    assert!(f.supersede_habit(&old, &b).status.success());
    assert!(!f
        .run(&[
            "miner",
            "habit",
            "renew",
            "1",
            "--revision",
            &old.review_revision()
        ])
        .status
        .success());
    assert!(!f
        .in_fixture_terminal(&[
            "miner",
            "habit",
            "renew",
            "1",
            "--revision",
            &"f".repeat(64)
        ])
        .status
        .success());
    fs::remove_file(old_path).unwrap();
    let result = f.renew_habit(&old);
    assert!(result.status.success(), "{result:?}");
    let receipt: Value = serde_json::from_slice(&result.stdout).unwrap();
    let child = receipt["renewal"]["child_id"].as_i64().unwrap();
    assert_eq!(receipt["renewal"]["generation"], 2);
    assert_eq!(receipt["renewal"]["child_status"], "candidate");
    assert!(db.habit_evidence(child).unwrap().is_empty());
    assert!(db
        .habit(child)
        .unwrap()
        .unwrap()
        .observed_revision
        .is_none());
    assert_eq!(f.profile_from_mcp()["habits"][0]["id"], b.id);
    assert!(!f.style().contains(BEHAVIOR));
    let shown = f.success(&["miner", "habit", "show", &child.to_string()]);
    assert!(String::from_utf8_lossy(&shown.stdout).contains("Generation: 2"));
    assert!(String::from_utf8_lossy(&shown.stdout).contains(&old.review_revision()));
    for (tag, episode) in [
        ("return-a", "return-issue-a"),
        ("return-b", "return-issue-b"),
    ] {
        assert!(!f
            .observe_in_fixture_terminal(&child.to_string())
            .status
            .success());
        let path = f.write(
            &format!("sessions/2026/09/26/rollout-{tag}.jsonl"),
            &jsonl(&f.records(tag)),
        );
        f.success(&[
            "miner",
            "habit",
            "cite",
            &child.to_string(),
            "--transcript",
            path.to_str().unwrap(),
            "--quote",
            QUOTE,
            "--episode",
            episode,
        ]);
    }
    assert!(!f
        .in_fixture_terminal(&[
            "miner",
            "habit",
            "observe",
            &child.to_string(),
            "--revision",
            &old.review_revision()
        ])
        .status
        .success());
    let current = db.habit(child).unwrap().unwrap();
    assert!(f.supersede_habit(&b, &current).status.success());
    let profile = f.profile_from_mcp();
    assert_eq!(profile["habits"].as_array().unwrap().len(), 1);
    assert_eq!(profile["habits"][0]["id"], child);
    assert_eq!(profile["habits"][0]["behavior"], BEHAVIOR);
    let before = db.aggregate().unwrap().profile_revision();
    let repeat = f.renew_habit(&old);
    assert!(repeat.status.success(), "{repeat:?}");
    let repeat: Value = serde_json::from_slice(&repeat.stdout).unwrap();
    assert_eq!(repeat["renewal"]["repeated"], true);
    assert_eq!(repeat["renewal"]["child_id"], child);
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
}

#[cfg(unix)]
#[test]
fn habit_renewal_preserves_legacy_candidate_requests_and_never_redirects_receipts() {
    use sha2::{Digest, Sha256};
    let f = Fixture::new();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(&f.project)
        .status()
        .unwrap()
        .success());
    let first = f.write(
        "sessions/2026/09/26/rollout-legacy.jsonl",
        &f.preferences(
            "legacy",
            &["I usually review the contract before changing code."],
        ),
    );
    assert!(f.collect(&[&first], false).status.success());
    let candidate = f.inbox()["candidates"][0]["candidate"].clone();
    assert!(f
        .propose_collected(&candidate, "issue-one")
        .status
        .success());
    let raw = Connection::open(f.db_path()).unwrap();
    let hash: String = raw
        .query_row(
            "SELECT request_digest FROM persona_candidate_habit",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let repository: String = raw
        .query_row(
            "SELECT repository FROM persona_collection_source",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let legacy_request = json!(["candidate-habit-v1",{
        "episode":"issue-one","when":"When changing a public contract","behavior":BEHAVIOR,
        "outcome":"Keeps the expected behavior explicit","exception":"","global":false,"role":"","workflow":""
    },format!("project:{}",candidate["project"].as_str().unwrap()),candidate["project"],candidate["project_root"],repository]);
    let expected: String = Sha256::digest(serde_json::to_vec(&legacy_request).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(hash, expected);
    raw.execute_batch("DROP TABLE persona_habit_generation; DROP INDEX persona_claim_unique;
        ALTER TABLE persona_claim DROP COLUMN generation;
        CREATE UNIQUE INDEX persona_claim_unique ON persona_claim(kind,when_text,behavior,outcome,exception_text,scope,role,workflow);").unwrap();
    let retry = f.propose_collected(&candidate, "issue-one");
    assert!(retry.status.success(), "{retry:?}");
    assert_eq!(
        raw.query_row(
            "SELECT request_digest FROM persona_candidate_habit",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        hash
    );
    let moved = f.codex.join("archived_sessions/rollout-legacy.jsonl");
    fs::create_dir_all(moved.parent().unwrap()).unwrap();
    fs::rename(first, &moved).unwrap();
    assert!(f.collect(&[&moved], false).status.success());
    let moved_candidate = f.inbox()["candidates"][0]["candidate"].clone();
    assert!(f
        .propose_collected(&moved_candidate, "issue-one")
        .status
        .success());
    let db = ProfileStore::open_read_only(&f.db_path()).unwrap();
    assert_eq!(db.habits().unwrap().len(), 1);
    let parent = db.habit(1).unwrap().unwrap();
    f.success(&["miner", "habit", "reject", "1"]);
    let renewal = f.renew_habit(&parent);
    assert!(renewal.status.success(), "{renewal:?}");
    let child = serde_json::from_slice::<Value>(&renewal.stdout).unwrap()["renewal"]["child_id"]
        .as_i64()
        .unwrap();
    assert!(!f
        .propose_collected_to(&moved_candidate, "issue-one", Some(child))
        .status
        .success());
    assert!(!f
        .propose_collected(&moved_candidate, "issue-one")
        .status
        .success());
    let second = f.write(
        "sessions/2026/09/26/rollout-return.jsonl",
        &f.preferences(
            "return",
            &["I usually review the contract before changing code."],
        ),
    );
    assert!(f.collect(&[&second], false).status.success());
    let second = f.inbox()["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["candidate"]["id"] != candidate["id"])
        .unwrap()["candidate"]
        .clone();
    assert!(!f.propose_collected(&second, "issue-two").status.success());
    let proposal = f.propose_collected_to(&second, "issue-two", Some(child));
    assert!(proposal.status.success(), "{proposal:?}");
    assert_eq!(
        serde_json::from_slice::<Value>(&proposal.stdout).unwrap()["proposal"]["habit_id"],
        child
    );
    assert_eq!(db.habit_evidence(child).unwrap().len(), 1);
    assert_eq!(db.habit_evidence(parent.id).unwrap().len(), 1);
    let before = db.aggregate().unwrap().profile_revision();
    assert!(!f
        .propose_collected_to(&second, "issue-two", Some(parent.id))
        .status
        .success());
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
}

#[cfg(unix)]
#[test]
fn habit_renewal_targeted_source_rebind_stales_child_and_retry_never_revives_parent_or_child() {
    let f = Fixture::new();
    f.reviewable_habit();
    let db = ProfileStore::open_read_only(&f.db_path()).unwrap();
    let parent = db.habit(1).unwrap().unwrap();
    f.success(&["miner", "habit", "reject", "1"]);
    let result = f.renew_habit(&parent);
    assert!(result.status.success(), "{result:?}");
    let child = serde_json::from_slice::<Value>(&result.stdout).unwrap()["renewal"]["child_id"]
        .as_i64()
        .unwrap();
    let mut sources = Vec::new();
    for tag in ["source-a", "source-b"] {
        let path = f.write(
            &format!("sessions/2026/09/26/rollout-{tag}.jsonl"),
            &f.preferences(
                tag,
                &["I usually review the contract before changing code."],
            ),
        );
        assert!(f.collect(&[&path], false).status.success());
        let candidate = f.inbox()["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["candidate"]["source"] == format!("session:codex:{tag}"))
            .unwrap()["candidate"]
            .clone();
        let proposed = f.propose_collected_to(&candidate, tag, Some(child));
        assert!(proposed.status.success(), "{proposed:?}");
        sources.push((path, candidate));
    }
    assert!(f
        .observe_in_fixture_terminal(&child.to_string())
        .status
        .success());
    let raw = Connection::open(f.db_path()).unwrap();
    raw.execute("UPDATE persona_claim SET status='observed' WHERE id=1", [])
        .unwrap();
    raw.execute(
        "INSERT INTO persona_habit_observation VALUES(1,?1)",
        [parent.review_revision()],
    )
    .unwrap();
    let packet = f.profile_from_mcp();
    assert_eq!(packet["habits"].as_array().unwrap().len(), 1);
    assert_eq!(packet["habits"][0]["id"], child);
    assert!(!f.observe_in_fixture_terminal("1").status.success());
    assert!(!f
        .propose_collected_to(&sources[0].1, "source-a", Some(1))
        .status
        .success());
    let moved = f.codex.join("archived_sessions/rollout-source-b.jsonl");
    fs::create_dir_all(moved.parent().unwrap()).unwrap();
    fs::rename(&sources[1].0, &moved).unwrap();
    assert!(f.collect(&[&moved], false).status.success());
    let revised = f.inbox()["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["candidate"]["id"] == sources[1].1["id"])
        .unwrap()["candidate"]
        .clone();
    let relocated = f.propose_collected_to(&revised, "source-b", Some(child));
    assert!(relocated.status.success(), "{relocated:?}");
    assert_eq!(db.habit(child).unwrap().unwrap().status, "stale");
    assert_eq!(db.habit_evidence(child).unwrap().len(), 2);
    assert!(db
        .habit(child)
        .unwrap()
        .unwrap()
        .observed_revision
        .is_none());
    let before = db.aggregate().unwrap().profile_revision();
    assert!(f.renew_habit(&parent).status.success());
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    assert!(f.profile_from_mcp()["habits"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!f.style().contains(BEHAVIOR));
    assert!(f
        .observe_in_fixture_terminal(&child.to_string())
        .status
        .success());
    f.success(&["miner", "habit", "reject", &child.to_string()]);
    let repeat = f.renew_habit(&parent);
    assert!(repeat.status.success(), "{repeat:?}");
    assert_eq!(
        serde_json::from_slice::<Value>(&repeat.stdout).unwrap()["renewal"]["child_status"],
        "rejected"
    );
    assert!(f.profile_from_mcp()["habits"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[cfg(unix)]
#[test]
fn habit_supersession_cli_gates_and_missing_old_source_do_not_weaken_successor_verification() {
    let f = Fixture::new();
    let args = [
        "miner",
        "habit",
        "supersede",
        "1",
        "--with",
        "2",
        "--old-revision",
        &"a".repeat(64),
        "--new-revision",
        &"b".repeat(64),
    ];
    assert!(!f.run(&args).status.success());
    assert!(!f.in_fixture_terminal(&args).status.success());
    assert!(!f.db_path().exists());
    let (old_source, _) = f.reviewable_habit();
    let old = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .habit(1)
        .unwrap()
        .unwrap();
    let (new, source, _) = f.successor_habit("next", "Checks changed public contracts");
    let db = ProfileStore::open_read_only(&f.db_path()).unwrap();
    let before = db.aggregate().unwrap().profile_revision();
    assert!(!f.supersede_habit(&old, &old).status.success());
    assert!(!f
        .run(&[
            "miner",
            "habit",
            "supersede",
            "1",
            "--with",
            "2",
            "--old-revision",
            &old.review_revision(),
            "--new-revision",
            &new.review_revision()
        ])
        .status
        .success());
    for revision in ["short".to_string(), "f".repeat(64)] {
        assert!(!f
            .in_fixture_terminal(&[
                "miner",
                "habit",
                "supersede",
                "1",
                "--with",
                "2",
                "--old-revision",
                &revision,
                "--new-revision",
                &new.review_revision()
            ])
            .status
            .success());
    }
    let bytes = fs::read(&source).unwrap();
    fs::write(
        &source,
        String::from_utf8(bytes.clone())
            .unwrap()
            .replace("user.text", "user.pasted"),
    )
    .unwrap();
    assert!(!f.supersede_habit(&old, &new).status.success());
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    fs::write(&source, bytes).unwrap();
    fs::remove_file(old_source).unwrap();
    let replacement = f.supersede_habit(&old, &new);
    assert!(replacement.status.success(), "{replacement:?}");
    assert_eq!(f.profile_from_mcp()["habits"][0]["behavior"], new.behavior);
    assert!(f.style().contains(&new.behavior));
    assert!(!f.style().contains(&old.behavior));
}

#[cfg(unix)]
#[test]
fn habit_supersession_chain_and_source_loss_expose_only_current_habits_through_mcp() {
    let f = Fixture::new();
    f.reviewable_habit();
    let db = ProfileStore::open_read_only(&f.db_path()).unwrap();
    let old = db.habit(1).unwrap().unwrap();
    assert!(f.observe_in_fixture_terminal("1").status.success());
    let (new, source, _) = f.successor_habit("next", "Checks changed public contracts");
    let result = f.supersede_habit(&old, &new);
    assert!(result.status.success(), "{result:?}");
    let receipt: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(receipt["receipt"]["repeated"], false);
    assert_eq!(f.profile_from_mcp()["habits"].as_array().unwrap().len(), 1);
    assert_eq!(f.profile_from_mcp()["habits"][0]["behavior"], new.behavior);
    let shown = f.success(&["miner", "habit", "show", "1"]);
    let shown = String::from_utf8_lossy(&shown.stdout);
    assert!(shown.contains("Relations:"));
    assert!(shown.contains(&old.review_revision()));
    assert!(shown.contains(&new.review_revision()));
    let raw = Connection::open(f.db_path()).unwrap();
    // Simulate an older writer that resets a status and restores a pin. The
    // immutable outgoing relation still prevents retrieval or another review.
    raw.execute("UPDATE persona_claim SET status='observed' WHERE id=1", [])
        .unwrap();
    raw.execute(
        "INSERT INTO persona_habit_observation VALUES(1,?1)",
        [&old.review_revision()],
    )
    .unwrap();
    let packet = f.profile_from_mcp();
    assert_eq!(packet["habits"].as_array().unwrap().len(), 1);
    assert_eq!(packet["habits"][0]["behavior"], new.behavior);
    assert!(!f.observe_in_fixture_terminal("1").status.success());
    assert!(!f.run(&["miner", "habit", "reject", "1"]).status.success());
    f.success(&["miner", "habit", "refresh"]);
    assert!(!f.style().contains(&old.behavior));

    let bytes = fs::read(&source).unwrap();
    fs::remove_file(&source).unwrap();
    let before = db.aggregate().unwrap().profile_revision();
    let result = f.supersede_habit(&old, &new);
    assert!(result.status.success(), "{result:?}");
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    let packet = f.profile_from_mcp();
    assert_eq!(packet["source_verification"], "incomplete");
    assert!(packet["habits"].as_array().unwrap().is_empty());
    assert!(!f.style().contains(&new.behavior));
    fs::write(&source, bytes).unwrap();
    let (third, _, _) = f.successor_habit("third", "Checks contract compatibility at delivery");
    assert!(f.supersede_habit(&new, &third).status.success());
    let result = f.supersede_habit(&old, &new);
    assert!(result.status.success(), "{result:?}");
    let receipt: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(receipt["receipt"]["repeated"], true);
    assert_eq!(receipt["receipt"]["new_status"], "superseded");
    let packet = f.profile_from_mcp();
    assert_eq!(packet["habits"].as_array().unwrap().len(), 1);
    assert_eq!(packet["habits"][0]["behavior"], third.behavior);
    assert!(!f.style().contains(&old.behavior));
    assert!(!f.style().contains(&new.behavior));
    f.success(&["miner", "habit", "reject", &third.id.to_string()]);
    assert!(f.supersede_habit(&new, &third).status.success());
    assert!(f.profile_from_mcp()["habits"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!f.style().contains(&third.behavior));
}

#[cfg(unix)]
#[test]
fn habit_supersession_retry_does_not_observe_rebound_or_dismissed_evidence() {
    let f = Fixture::new();
    f.reviewable_habit();
    let db = ProfileStore::open_read_only(&f.db_path()).unwrap();
    let old = db.habit(1).unwrap().unwrap();
    let (new, _, second) = f.successor_habit("next", "Checks changed public contracts");
    assert!(f.supersede_habit(&old, &new).status.success());
    let archived = f.codex.join("archived_sessions/rollout-next-b.jsonl");
    fs::create_dir_all(archived.parent().unwrap()).unwrap();
    fs::rename(second, &archived).unwrap();
    f.success(&[
        "miner",
        "habit",
        "cite",
        &new.id.to_string(),
        "--transcript",
        archived.to_str().unwrap(),
        "--quote",
        QUOTE,
        "--episode",
        "issue-two",
    ]);
    assert_eq!(db.habit(new.id).unwrap().unwrap().status, "stale");
    let before = db.aggregate().unwrap().profile_revision();
    assert!(f.supersede_habit(&old, &new).status.success());
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    assert!(!f.style().contains(&new.behavior));
    assert!(f.profile_from_mcp()["habits"]
        .as_array()
        .unwrap()
        .is_empty());
    // A separate current review can observe the rebound sources. Repeating the
    // original replacement after withdrawal still cannot supply that review.
    assert!(f
        .observe_in_fixture_terminal(&new.id.to_string())
        .status
        .success());
    let evidence = db.habit_evidence(new.id).unwrap()[0].id;
    let dismissed = f.in_fixture_terminal(&[
        "miner",
        "habit",
        "dismiss",
        &new.id.to_string(),
        &evidence.to_string(),
    ]);
    assert!(dismissed.status.success(), "{dismissed:?}");
    let before = db.aggregate().unwrap().profile_revision();
    assert!(f.supersede_habit(&old, &new).status.success());
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    assert!(db
        .habit(new.id)
        .unwrap()
        .unwrap()
        .observed_revision
        .is_none());
    assert!(f.profile_from_mcp()["habits"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!f.style().contains(&new.behavior));
}

#[cfg(unix)]
#[test]
fn habit_supersession_blocks_old_candidate_receipts_and_new_proposals_for_retired_definition() {
    let f = Fixture::new();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(&f.project)
        .status()
        .unwrap()
        .success());
    let path = f.write(
        "sessions/2026/09/26/rollout-original.jsonl",
        &f.preferences(
            "original",
            &["I usually review the contract before changing code."],
        ),
    );
    assert!(f.collect(&[&path], false).status.success());
    let candidate = f.inbox()["candidates"][0]["candidate"].clone();
    assert!(f
        .propose_collected(&candidate, "issue-one")
        .status
        .success());
    let db = ProfileStore::open_read_only(&f.db_path()).unwrap();
    let old = db.habit(1).unwrap().unwrap();
    let (new, _, _) = f.successor_habit("next", "Checks changed public contracts");
    assert!(f.supersede_habit(&old, &new).status.success());
    let raw = Connection::open(f.db_path()).unwrap();
    raw.execute("UPDATE persona_claim SET status='candidate' WHERE id=1", [])
        .unwrap();
    let before = db.aggregate().unwrap().profile_revision();
    assert!(!f
        .propose_collected(&candidate, "issue-one")
        .status
        .success());
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    let path = f.write(
        "sessions/2026/09/26/rollout-again.jsonl",
        &f.preferences(
            "again",
            &["I usually review the contract before changing code."],
        ),
    );
    assert!(f.collect(&[&path], false).status.success());
    let second = f.inbox()["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["candidate"]["id"] != candidate["id"])
        .unwrap()["candidate"]
        .clone();
    assert!(!f.propose_collected(&second, "issue-two").status.success());
    assert_eq!(db.habits().unwrap().len(), 2);
    assert_eq!(f.profile_from_mcp()["habits"][0]["behavior"], new.behavior);
}

#[cfg(unix)]
#[test]
fn habit_observation_binds_reviewed_definition_and_evidence_through_cli_and_mcp() {
    let f = Fixture::new();
    let (_, second) = f.reviewable_habit();
    let current = || {
        ProfileStore::open_read_only(&f.db_path())
            .unwrap()
            .habit(1)
            .unwrap()
            .unwrap()
    };
    let initial = current();
    let revision = initial.review_revision();
    let shown = f.success(&["miner", "habit", "show", "1"]);
    let shown = String::from_utf8_lossy(&shown.stdout);
    assert!(shown.contains(&revision));
    assert!(shown.contains("Observed revision: none"));
    assert!(shown.contains("Source verification: current"));
    assert!(!f.run(&["miner", "habit", "observe", "1"]).status.success());
    assert!(!f
        .run(&["miner", "habit", "observe", "1", "--revision", &revision])
        .status
        .success());
    assert!(!f
        .in_fixture_terminal(&[
            "miner",
            "habit",
            "observe",
            "1",
            "--revision",
            &"f".repeat(64)
        ])
        .status
        .success());
    assert!(!f.style().contains(BEHAVIOR));
    assert!(f.profile_from_mcp()["habits"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(f.observe_in_fixture_terminal("1").status.success());
    assert_eq!(
        current().observed_revision.as_deref(),
        Some(revision.as_str())
    );
    assert_eq!(f.profile_from_mcp()["habits"][0]["behavior"], BEHAVIOR);
    assert!(f.style().contains(BEHAVIOR));
    let conn = Connection::open(f.db_path()).unwrap();
    let events = || {
        conn.query_row("SELECT count(*) FROM persona_review_event", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    };
    let count = events();
    assert!(f.observe_in_fixture_terminal("1").status.success());
    assert_eq!(events(), count);

    // An older writer can change wording without touching the pin. Retrieval
    // still refuses it, and the original show digest cannot approve the edit.
    conn.execute(
        "UPDATE persona_claim SET outcome='Reports changed evidence' WHERE id=1",
        [],
    )
    .unwrap();
    assert!(f.profile_from_mcp()["habits"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!f
        .in_fixture_terminal(&["miner", "habit", "observe", "1", "--revision", &revision])
        .status
        .success());
    f.success(&["miner", "habit", "refresh"]);
    assert!(!f.style().contains(BEHAVIOR));
    assert!(f.observe_in_fixture_terminal("1").status.success());
    let definition_review = current();
    let third = f.write(
        "sessions/2026/09/26/rollout-habit-c.jsonl",
        &jsonl(&f.records("habit-c")),
    );
    f.success(&[
        "miner",
        "habit",
        "cite",
        "1",
        "--transcript",
        third.to_str().unwrap(),
        "--quote",
        QUOTE,
        "--episode",
        "issue-three",
    ]);
    assert_eq!(current().status, "stale");
    assert_eq!(current().observed_revision, None);
    assert!(!f.style().contains(BEHAVIOR));
    assert!(!f
        .in_fixture_terminal(&[
            "miner",
            "habit",
            "observe",
            "1",
            "--revision",
            &definition_review.review_revision()
        ])
        .status
        .success());
    assert!(f.observe_in_fixture_terminal("1").status.success());
    let count = events();
    let bytes = fs::read(&second).unwrap();
    fs::write(
        &second,
        String::from_utf8(bytes.clone())
            .unwrap()
            .replace("user.text", "user.pasted"),
    )
    .unwrap();
    assert!(!f.observe_in_fixture_terminal("1").status.success());
    assert_eq!(events(), count);
    let packet = f.profile_from_mcp();
    assert_eq!(packet["source_verification"], "incomplete");
    assert!(packet["habits"].as_array().unwrap().is_empty());
    f.success(&["miner", "habit", "refresh"]);
    assert!(!f.style().contains(BEHAVIOR));
    fs::write(&second, bytes).unwrap();
    assert!(f.observe_in_fixture_terminal("1").status.success());
    assert_eq!(events(), count);
    assert!(f.style().contains(BEHAVIOR));
    let shown = f.success(&["miner", "habit", "show", "1"]);
    let shown = String::from_utf8_lossy(&shown.stdout);
    assert!(shown.contains("Review history:"));
    assert!(shown.contains(&revision));
}

#[cfg(unix)]
#[test]
fn habit_observation_requires_new_review_for_legacy_records_and_never_revives_rejection() {
    let f = Fixture::new();
    f.reviewable_habit();
    let conn = Connection::open(f.db_path()).unwrap();
    conn.execute_batch("UPDATE persona_claim SET status='observed'; INSERT INTO persona_review_event(claim_id,status,at_epoch) VALUES(1,'observed',1);").unwrap();
    let packet = f.profile_from_mcp();
    assert_eq!(packet["source_verification"], "incomplete");
    assert!(packet["habits"].as_array().unwrap().is_empty());
    f.success(&["miner", "habit", "refresh"]);
    assert!(!f.style().contains(BEHAVIOR));
    let show = f.success(&["miner", "habit", "show", "1"]);
    assert!(String::from_utf8_lossy(&show.stdout).contains("\"reviewed_revision\":null"));
    assert!(f.observe_in_fixture_terminal("1").status.success());
    assert_eq!(f.profile_from_mcp()["habits"][0]["behavior"], BEHAVIOR);
    f.success(&["miner", "habit", "reject", "1"]);
    assert!(!f.observe_in_fixture_terminal("1").status.success());
    assert!(f.profile_from_mcp()["habits"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!f.style().contains(BEHAVIOR));
    let current = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .habit(1)
        .unwrap()
        .unwrap();
    assert_eq!(current.status, "rejected");
    assert_eq!(current.observed_revision, None);
}

#[cfg(unix)]
#[test]
fn habit_observation_enforces_the_shared_source_budget_before_recording_review() {
    let f = Fixture::new();
    f.reviewable_habit();
    for i in 2..17 {
        let id = format!("habit-{i}");
        let path = f.write(
            &format!("sessions/2026/09/26/rollout-{id}.jsonl"),
            &jsonl(&f.records(&id)),
        );
        f.success(&[
            "miner",
            "habit",
            "cite",
            "1",
            "--transcript",
            path.to_str().unwrap(),
            "--quote",
            QUOTE,
            "--episode",
            &format!("extra-issue-{i}"),
        ]);
        if i == 15 {
            assert!(f.observe_in_fixture_terminal("1").status.success());
        }
    }
    assert!(!f.observe_in_fixture_terminal("1").status.success());
    let db = ProfileStore::open_read_only(&f.db_path()).unwrap();
    assert_eq!(db.habit(1).unwrap().unwrap().observed_revision, None);
    assert!(!f.style().contains(BEHAVIOR));
    let packet = f.profile_from_mcp();
    assert!(packet["habits"].as_array().unwrap().is_empty());
}

#[cfg(unix)]
const PREFERENCE_QUOTE: &str = "I prefer short replies with the test results.";
#[cfg(unix)]
const PREFERENCE_STATEMENT: &str = "Keep replies short and include test results";

#[cfg(unix)]
#[test]
fn supersession_chain_and_missing_sources_never_restore_retired_preferences_through_mcp() {
    let f = Fixture::new();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(&f.project)
        .status()
        .unwrap()
        .success());
    let (a_path, a_candidate, a) = f.strict_preference(
        "old-pref",
        "I prefer brief replies.",
        "Prefer brief replies",
    );
    let (b_path, _, b) = f.strict_preference(
        "new-pref",
        "I prefer detailed replies.",
        "Prefer detailed replies",
    );
    assert!(f
        .in_fixture_terminal(&[
            "miner",
            "feedback",
            "accept",
            &a.key,
            "--revision",
            &a.review_revision()
        ])
        .status
        .success());
    assert!(f.style().contains(&a.statement));
    let a_bytes = fs::read(&a_path).unwrap();
    fs::remove_file(&a_path).unwrap();
    let replaced = f.supersede(&a, &b);
    assert!(replaced.status.success(), "{replaced:?}");
    let receipt: Value = serde_json::from_slice(&replaced.stdout).unwrap();
    assert_eq!(receipt["receipt"]["repeated"], false);
    assert!(!f.style().contains(&a.statement));
    assert!(f.style().contains(&b.statement));
    let packet = f.profile_from_mcp();
    assert_eq!(packet["feedback"].as_array().unwrap().len(), 1);
    assert_eq!(packet["feedback"][0]["statement"], b.statement);
    assert!(!packet.to_string().contains(&a.statement));
    assert!(!packet.to_string().contains(b_path.to_str().unwrap()));
    let show = f.success(&["miner", "feedback", "show", &a.key]);
    assert!(String::from_utf8_lossy(&show.stdout).contains("Relations:"));
    assert!(String::from_utf8_lossy(&show.stdout).contains(&b.key));

    let b_bytes = fs::read(&b_path).unwrap();
    fs::remove_file(&b_path).unwrap();
    fs::write(&a_path, &a_bytes).unwrap();
    assert!(f.profile_from_mcp()["feedback"]
        .as_array()
        .unwrap()
        .is_empty());
    let before = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .aggregate()
        .unwrap()
        .profile_revision();
    let retry = f.supersede(&a, &b);
    assert!(
        retry.status.success(),
        "a committed retry precedes source freshness gates: {retry:?}"
    );
    assert_eq!(
        before,
        ProfileStore::open_read_only(&f.db_path())
            .unwrap()
            .aggregate()
            .unwrap()
            .profile_revision()
    );
    assert!(!f.style().contains(&a.statement));
    assert!(!f.style().contains(&b.statement));
    assert!(!f
        .in_fixture_terminal(&[
            "miner",
            "feedback",
            "accept",
            &a.key,
            "--revision",
            &a.review_revision()
        ])
        .status
        .success());
    assert!(!f
        .propose_preference(&a_candidate, &a.statement, Some("global"))
        .status
        .success());
    fs::write(&b_path, b_bytes).unwrap();
    let (_, _, c) = f.strict_preference(
        "latest-pref",
        "I prefer replies with a summary.",
        "Prefer replies with a summary",
    );
    assert!(f.supersede(&b, &c).status.success());
    fs::remove_file(&b_path).unwrap();
    let before = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .aggregate()
        .unwrap()
        .profile_revision();
    let conn = Connection::open(f.db_path()).unwrap();
    let count = || {
        conn.query_row("SELECT count(*) FROM feedback_review_event", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    };
    let events = count();
    let retry = f.supersede(&a, &b);
    assert!(retry.status.success(), "{retry:?}");
    let receipt: Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert_eq!(receipt["receipt"]["repeated"], true);
    assert_eq!(receipt["receipt"]["new_status"], "superseded");
    assert_eq!(
        before,
        ProfileStore::open_read_only(&f.db_path())
            .unwrap()
            .aggregate()
            .unwrap()
            .profile_revision()
    );
    assert_eq!(events, count());
    assert_eq!(
        f.profile_from_mcp()["feedback"][0]["statement"],
        c.statement
    );
    assert!(!f.style().contains(&a.statement));
    assert!(!f.style().contains(&b.statement));
    f.success(&["miner", "feedback", "reject", &c.key]);
    assert!(f.supersede(&b, &c).status.success());
    assert!(f.profile_from_mcp()["feedback"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[cfg(unix)]
#[test]
fn supersession_retry_does_not_accept_rebound_or_dismissed_successor_evidence() {
    let f = Fixture::new();
    let (_, _, a) = f.strict_preference(
        "before-rebind",
        "I prefer brief replies.",
        "Prefer brief replies",
    );
    let (path, candidate, b) = f.strict_preference(
        "after-rebind",
        "I prefer detailed replies.",
        "Prefer detailed replies",
    );
    assert!(f.supersede(&a, &b).status.success());
    let mut records: Vec<Value> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    records.insert(
        2,
        json!({"type":"event_msg", "payload":{"type":"agent_message", "message":"fixture"}}),
    );
    fs::write(&path, jsonl(&records)).unwrap();
    let collected = f.collect(&[&path], false);
    assert!(collected.status.success(), "{collected:?}");
    let rebound = f.inbox()["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["candidate"]["id"] == candidate["id"])
        .unwrap()["candidate"]
        .clone();
    assert_ne!(rebound["revision"], candidate["revision"]);
    assert!(f
        .propose_preference(&rebound, &b.statement, Some("global"))
        .status
        .success());
    let current = || {
        ProfileStore::open_read_only(&f.db_path())
            .unwrap()
            .feedback()
            .unwrap()
            .into_iter()
            .find(|entry| entry.key == b.key)
            .unwrap()
    };
    assert_eq!(current().status, "candidate");
    assert_ne!(current().review_revision(), b.review_revision());
    assert!(f.supersede(&a, &b).status.success());
    assert_eq!(current().status, "candidate");
    assert_eq!(current().accepted_revision, None);
    assert!(!f.style().contains(&b.statement));
    let dismissed = f.in_fixture_terminal(&[
        "miner",
        "feedback",
        "dismiss-source",
        &b.key,
        rebound["id"].as_str().unwrap(),
        "--revision",
        &current().review_revision(),
    ]);
    assert!(dismissed.status.success(), "{dismissed:?}");
    fs::remove_file(path).unwrap();
    assert!(f.supersede(&a, &b).status.success());
    assert_eq!(current().status, "candidate");
    assert_eq!(current().accepted_revision, None);
    assert!(!f.style().contains(&a.statement));
    assert!(!f.style().contains(&b.statement));
}

#[cfg(unix)]
#[test]
fn supersession_cli_rejects_noninteractive_ambiguous_self_stale_and_unavailable_requests() {
    let f = Fixture::new();
    let empty = f.in_fixture_terminal(&[
        "miner",
        "feedback",
        "supersede",
        "missing",
        "--with",
        "other",
        "--old-revision",
        &"0".repeat(64),
        "--new-revision",
        &"0".repeat(64),
    ]);
    assert!(!empty.status.success());
    assert!(!f.db_path().exists());
    let (_, _, a) = f.strict_preference(
        "before-gates",
        "I prefer brief replies.",
        "Prefer brief replies",
    );
    let (path, _, b) = f.strict_preference(
        "after-gates",
        "I prefer detailed replies.",
        "Prefer detailed replies",
    );
    let args = [
        "miner",
        "feedback",
        "supersede",
        &a.key,
        "--with",
        &b.key,
        "--old-revision",
        &a.review_revision(),
        "--new-revision",
        &b.review_revision(),
    ];
    let before = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .aggregate()
        .unwrap()
        .profile_revision();
    assert!(!f.run(&args).status.success());
    for (old, new) in [("prefer", b.key.as_str()), (a.key.as_str(), "prefer-brief")] {
        let denied = f.in_fixture_terminal(&[
            "miner",
            "feedback",
            "supersede",
            old,
            "--with",
            new,
            "--old-revision",
            &a.review_revision(),
            "--new-revision",
            &b.review_revision(),
        ]);
        assert!(!denied.status.success(), "{denied:?}");
    }
    assert!(!f
        .in_fixture_terminal(&[
            "miner",
            "feedback",
            "supersede",
            &a.key,
            "--with",
            &b.key,
            "--old-revision",
            &a.review_revision(),
            "--new-revision",
            &"f".repeat(64)
        ])
        .status
        .success());
    fs::remove_file(path).unwrap();
    assert!(!f.supersede(&a, &b).status.success());
    assert_eq!(
        before,
        ProfileStore::open_read_only(&f.db_path())
            .unwrap()
            .aggregate()
            .unwrap()
            .profile_revision()
    );
    assert!(!f.style().contains(&a.statement));
    assert!(!f.style().contains(&b.statement));
}

#[cfg(unix)]
#[test]
fn preference_admission_checks_whole_source_budget_and_dismissal_keeps_history() {
    let f = Fixture::new();
    let paths: Vec<_> = (0..17)
        .map(|i| {
            f.write(
                &format!("sessions/2026/09/26/rollout-budget-{i}.jsonl"),
                &f.preferences(&format!("budget-{i}"), &[PREFERENCE_QUOTE]),
            )
        })
        .collect();
    assert!(f
        .collect(
            &paths[..16].iter().map(PathBuf::as_path).collect::<Vec<_>>(),
            false
        )
        .status
        .success());
    assert!(f.collect(&[&paths[16]], false).status.success());
    let inbox = f.inbox();
    let candidates: Vec<_> = inbox["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| &row["candidate"])
        .collect();
    assert_eq!(candidates.len(), 17);
    for (i, candidate) in candidates.iter().enumerate() {
        let proposed = f.propose_preference(candidate, PREFERENCE_STATEMENT, Some("global"));
        if i < 16 {
            assert!(proposed.status.success(), "{proposed:?}");
        } else {
            assert!(!proposed.status.success());
            assert!(String::from_utf8_lossy(&proposed.stderr).contains("verification budget"));
        }
    }
    let conn = Connection::open(f.db_path()).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM persona_candidate_feedback", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        16
    );
    let entry = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .feedback()
        .unwrap()
        .remove(0);
    let shown = f.success(&["miner", "feedback", "show", &entry.key]);
    assert!(String::from_utf8_lossy(&shown.stdout).contains("Source verification: current"));
    let candidate = candidates[0]["id"].as_str().unwrap();
    assert!(!f
        .in_fixture_terminal(&[
            "miner",
            "feedback",
            "dismiss-source",
            &entry.key,
            candidate,
            "--revision",
            &"0".repeat(64)
        ])
        .status
        .success());
    let dismiss_args = [
        "miner",
        "feedback",
        "dismiss-source",
        &entry.key,
        candidate,
        "--revision",
        &entry.review_revision(),
    ];
    let dismissed = f.in_fixture_terminal(&dismiss_args);
    assert!(dismissed.status.success(), "{dismissed:?}");
    assert!(
        f.in_fixture_terminal(&dismiss_args).status.success(),
        "publication retry is idempotent"
    );
    assert!(
        !f.propose_preference(candidates[0], PREFERENCE_STATEMENT, Some("global"))
            .status
            .success(),
        "dismissed support cannot be restored by replay"
    );
    assert!(
        f.propose_preference(candidates[16], PREFERENCE_STATEMENT, Some("global"))
            .status
            .success(),
        "withdrawal frees verification capacity"
    );
    let entry = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .feedback()
        .unwrap()
        .remove(0);
    assert!(f
        .in_fixture_terminal(&[
            "miner",
            "feedback",
            "accept",
            &entry.key,
            "--revision",
            &entry.review_revision()
        ])
        .status
        .success());
    assert_eq!(f.profile_from_mcp()["feedback"][0]["sources"], 16);
    let show = f.success(&["miner", "feedback", "show", &entry.key]);
    let show = String::from_utf8_lossy(&show.stdout);
    assert!(show.contains("\"source_dismissed\":true"));
    assert!(show.contains("source_dismissed:"));
    assert_eq!(
        conn.query_row("SELECT count(*) FROM feedback_evidence", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        17
    );
}

#[cfg(unix)]
#[test]
fn preference_acceptance_binds_review_and_live_sources_through_mcp_and_refresh() {
    let f = Fixture::new();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(&f.project)
        .status()
        .unwrap()
        .success());
    let mut records = f.records("pref-source");
    records[2]["payload"]["content"][0]["text"] = json!(PREFERENCE_QUOTE);
    // The same text in a preceding assistant message is never the citation.
    let mut assistant = records[2].clone();
    assistant["payload"]["role"] = json!("assistant");
    records.insert(2, assistant);
    let path = f.write(
        "sessions/2026/09/26/rollout-pref-source.jsonl",
        &jsonl(&records),
    );
    assert!(f.collect(&[&path], false).status.success());
    let candidate = f.inbox()["candidates"][0]["candidate"].clone();
    assert_eq!(candidate["line_no"], 4);
    let proposed = f.propose_preference(&candidate, PREFERENCE_STATEMENT, None);
    assert!(proposed.status.success(), "{proposed:?}");
    let receipt: Value = serde_json::from_slice(&proposed.stdout).unwrap();
    let key = receipt["proposal"]["feedback_key"].as_str().unwrap();
    let entry = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .feedback()
        .unwrap()
        .remove(0);
    assert!(entry.scope.starts_with("project:"));
    let revision = entry.review_revision();
    let show = f.success(&["miner", "feedback", "show", key]);
    let show = String::from_utf8_lossy(&show.stdout);
    assert!(show.contains(&revision));
    assert!(show.contains("Source verification: current"));
    assert!(show.contains("\"line_no\":4"));
    assert!(!f.style().contains(PREFERENCE_STATEMENT));
    assert!(f.profile_from_mcp()["feedback"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(
        !f.run(&["miner", "feedback", "accept", key, "--revision", &revision])
            .status
            .success(),
        "noninteractive accept remains forbidden"
    );
    assert!(!f
        .in_fixture_terminal(&[
            "miner",
            "feedback",
            "accept",
            key,
            "--revision",
            &"0".repeat(64)
        ])
        .status
        .success());
    let accepted =
        f.in_fixture_terminal(&["miner", "feedback", "accept", key, "--revision", &revision]);
    assert!(accepted.status.success(), "{accepted:?}");
    assert!(f.style().contains(PREFERENCE_STATEMENT));
    let packet = f.profile_from_mcp();
    assert_eq!(packet["feedback"][0]["statement"], PREFERENCE_STATEMENT);
    assert_eq!(packet["feedback"][0]["sources"], 1);
    assert_eq!(packet["feedback"][0]["key"], key);
    assert_eq!(packet["feedback"][0]["review_revision"], revision);
    assert!(f.style().contains(&format!("key {key}; review {revision}")));
    assert!(f.style().contains(&format!(
        "<!-- mastermind-style:store-revision:{} -->",
        packet["store_revision"].as_str().unwrap()
    )));
    assert!(!packet.to_string().contains(path.to_str().unwrap()));
    assert!(!packet.to_string().contains(PREFERENCE_QUOTE));
    let retry = f.propose_preference(&candidate, PREFERENCE_STATEMENT, None);
    assert!(retry.status.success(), "{retry:?}");
    let retry: Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert_eq!(retry["proposal"]["repeated"], true);
    assert_eq!(retry["proposal"]["status"], "active");

    // Source changes before a later accept or retrieval must fail even though
    // the SQL review digest and quoted words are unchanged.
    records[3]["payload"]["internal_chat_message_metadata_passthrough"]["content_item_kinds"] =
        json!(["user.pasted"]);
    fs::write(&path, jsonl(&records)).unwrap();
    assert!(f.profile_from_mcp()["feedback"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!f
        .in_fixture_terminal(&["miner", "feedback", "accept", key, "--revision", &revision])
        .status
        .success());
    f.success(&["miner", "feedback", "refresh"]);
    assert!(!f.style().contains(PREFERENCE_STATEMENT));

    records[3]["payload"]["internal_chat_message_metadata_passthrough"]["content_item_kinds"] =
        json!(["user.text"]);
    records.insert(2,json!({"type":"event_msg", "payload":{"type":"agent_message", "message":"An appended record moved the human turn"}}));
    fs::write(&path, jsonl(&records)).unwrap();
    assert!(f.collect(&[&path], false).status.success());
    let rebound = f.inbox()["candidates"][0]["candidate"].clone();
    assert_ne!(rebound["revision"], candidate["revision"]);
    assert!(!f
        .propose_preference(&candidate, PREFERENCE_STATEMENT, None)
        .status
        .success());
    assert!(f
        .propose_preference(&rebound, PREFERENCE_STATEMENT, None)
        .status
        .success());
    assert!(!f
        .in_fixture_terminal(&["miner", "feedback", "accept", key, "--revision", &revision])
        .status
        .success());
    let entry = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .feedback()
        .unwrap()
        .remove(0);
    assert_eq!(entry.status, "candidate");
    assert_eq!(entry.accepted_revision, None);
    assert!(f
        .in_fixture_terminal(&[
            "miner",
            "feedback",
            "accept",
            key,
            "--revision",
            &entry.review_revision()
        ])
        .status
        .success());
    assert_eq!(
        f.profile_from_mcp()["feedback"][0]["statement"],
        PREFERENCE_STATEMENT
    );
    assert!(!f
        .run(&[
            "miner",
            "candidates",
            "dismiss",
            rebound["id"].as_str().unwrap(),
            "--revision",
            rebound["revision"].as_str().unwrap()
        ])
        .status
        .success());
    f.success(&["miner", "feedback", "reject", key]);
    assert!(!f
        .propose_preference(&rebound, PREFERENCE_STATEMENT, None)
        .status
        .success());
    assert!(!f
        .in_fixture_terminal(&[
            "miner",
            "feedback",
            "accept",
            key,
            "--revision",
            &entry.review_revision()
        ])
        .status
        .success());
    assert!(f.profile_from_mcp()["feedback"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[cfg(unix)]
#[test]
fn legacy_feedback_survives_migration_but_only_strict_quotes_and_counts_are_published() {
    let f = Fixture::new();
    let legacy = f.write(
        "sessions/2026/09/26/rollout-legacy.jsonl",
        &jsonl(&f.records("legacy")),
    );
    let add = |path: &Path| {
        f.run(&[
            "miner",
            "feedback",
            "add",
            "--transcript",
            path.to_str().unwrap(),
            "--quote",
            QUOTE,
            "--statement",
            PREFERENCE_STATEMENT,
            "--category",
            "communication",
        ])
    };
    assert!(add(&legacy).status.success());
    let conn = Connection::open(f.db_path()).unwrap();
    conn.execute("UPDATE feedback SET status='active'", [])
        .unwrap();
    f.success(&["miner", "feedback", "refresh"]);
    assert!(!f.style().contains(PREFERENCE_STATEMENT));
    let old = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .feedback()
        .unwrap()
        .remove(0);
    assert_eq!(old.status, "active", "legacy review status is retained");
    assert!(
        String::from_utf8_lossy(&f.success(&["miner", "feedback", "show", &old.key]).stdout)
            .contains("legacy-unverifiable")
    );
    assert!(!f
        .in_fixture_terminal(&[
            "miner",
            "feedback",
            "accept",
            &old.key,
            "--revision",
            &old.review_revision()
        ])
        .status
        .success());
    assert!(f.profile_from_mcp()["feedback"]
        .as_array()
        .unwrap()
        .is_empty());
    let path = f.write(
        "sessions/2026/09/26/rollout-strict.jsonl",
        &f.preferences("strict", &[PREFERENCE_QUOTE]),
    );
    assert!(f.collect(&[&path], false).status.success());
    let candidate = f.inbox()["candidates"][0]["candidate"].clone();
    assert!(f
        .propose_preference(&candidate, PREFERENCE_STATEMENT, Some("global"))
        .status
        .success());
    let entry = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .feedback()
        .unwrap()
        .remove(0);
    assert_eq!(entry.key, old.key);
    assert_eq!(entry.status, "candidate");
    assert_eq!(entry.sources, 2, "legacy evidence remains stored");
    let accepted = f.in_fixture_terminal(&[
        "miner",
        "feedback",
        "accept",
        &entry.key,
        "--revision",
        &entry.review_revision(),
    ]);
    assert!(accepted.status.success(), "{accepted:?}");
    assert!(f.style().contains(PREFERENCE_QUOTE));
    assert!(
        !f.style().contains(QUOTE),
        "legacy quote is never projected as verified support"
    );
    assert_eq!(f.profile_from_mcp()["feedback"][0]["sources"], 1);
    let new_legacy = f.write(
        "sessions/2026/09/26/rollout-extra-legacy.jsonl",
        &jsonl(&f.records("extra-legacy")),
    );
    assert!(add(&new_legacy).status.success());
    assert!(
        !f.style().contains(PREFERENCE_STATEMENT),
        "new raw evidence invalidates the acceptance pin"
    );
    assert!(f.profile_from_mcp()["feedback"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!f
        .in_fixture_terminal(&[
            "miner",
            "feedback",
            "accept",
            &entry.key,
            "--revision",
            &entry.review_revision()
        ])
        .status
        .success());
    let changed = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .feedback()
        .unwrap()
        .remove(0);
    assert!(f
        .in_fixture_terminal(&[
            "miner",
            "feedback",
            "accept",
            &changed.key,
            "--revision",
            &changed.review_revision()
        ])
        .status
        .success());
    assert_eq!(f.profile_from_mcp()["feedback"][0]["sources"], 1);
}

#[test]
fn cli_accepts_custom_codex_home_and_archive_without_duplicate_sources() {
    let f = Fixture::new();
    let text = jsonl(&f.records("session-a"));
    let path = f.write("sessions/2026/09/26/rollout-test.jsonl", &text);
    let scan = f.success(&[
        "miner",
        "feedback",
        "scan",
        "--transcript",
        path.to_str().unwrap(),
    ]);
    let scan: Value = serde_json::from_slice(&scan.stdout).unwrap();
    assert_eq!(scan["client"], "codex");
    assert_eq!(scan["turns"][0]["line_no"], 3);
    assert_eq!(scan["turns"][0]["text"], QUOTE);
    assert!(f.feedback(&path).status.success());
    let archive = f.write("archived_sessions/rollout-copy.jsonl", &text);
    assert!(f.feedback(&archive).status.success());

    // The legacy Claude identifier remains unchanged, and does not collide
    // with the same identifier in Codex. Default scan still finds Claude.
    let claude = f
        .home
        .join(".claude/projects")
        .join(claude_project_slug(&f.project.canonicalize().unwrap()))
        .join("session.jsonl");
    fs::create_dir_all(claude.parent().unwrap()).unwrap();
    fs::write(
        &claude,
        json!({"type":"user", "origin":{"kind":"human"}, "sessionId":"session-a",
        "cwd":f.project, "message":{"content":QUOTE}})
        .to_string(),
    )
    .unwrap();
    assert!(f.feedback(&claude).status.success());
    let scan = f.success(&["miner", "feedback", "scan"]);
    let scan: Value = serde_json::from_slice(&scan.stdout).unwrap();
    assert_eq!(scan["client"], "claude");
    let db = Connection::open(f.db_path()).unwrap();
    let mut stmt = db
        .prepare("SELECT source FROM feedback_source ORDER BY source")
        .unwrap();
    let sources: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(sources, ["session:codex:session-a", "session:session-a"]);
    let status: String = db
        .query_row("SELECT status FROM feedback", [], |row| row.get(0))
        .unwrap();
    assert_eq!(status, "candidate");
    assert!(
        !f.style().contains(QUOTE),
        "unreviewed legacy feedback stays out of the published profile"
    );
}

#[test]
fn habits_revalidate_codex_metadata_context_and_message_from_the_same_snapshot() {
    let f = Fixture::new();
    let records = f.records("session-a");
    let original = jsonl(&records);
    let path = f.write("sessions/2026/09/26/rollout-first.jsonl", &original);
    let result = f.propose(&path, &f.project);
    assert!(result.status.success(), "{result:?}");
    assert!(!f.style().contains(BEHAVIOR));
    let second = f.write(
        "sessions/2026/09/26/rollout-second.jsonl",
        &jsonl(&f.records("session-b")),
    );
    f.success(&[
        "miner",
        "habit",
        "cite",
        "1",
        "--transcript",
        second.to_str().unwrap(),
        "--quote",
        QUOTE,
        "--episode",
        "issue-two",
    ]);
    // Set up a reviewed fixture through the store's real evidence gates.
    let mut db = ProfileStore::open(&f.db_path()).unwrap();
    let revision = db.habit(1).unwrap().unwrap().review_revision();
    db.review_habit(1, "observed", Some(&revision))
        .unwrap()
        .unwrap();
    drop(db);
    f.success(&["miner", "habit", "refresh"]);
    assert!(f.style().contains(BEHAVIOR));
    fs::write(
        &path,
        format!(
            "{original}{}\n",
            json!({"type":"event_msg", "payload":{"type":"task_complete"}})
        ),
    )
    .unwrap();
    f.success(&["miner", "habit", "refresh"]);
    assert!(
        f.style().contains(BEHAVIOR),
        "append must preserve a verified quote"
    );

    for mutation in 0..4 {
        let mut changed = records.clone();
        match mutation {
            0 => changed[0]["payload"]["source"] = json!("cli"),
            1 => changed[1]["payload"]["root_turn_id"] = json!("changed-root"),
            2 => changed[2]["payload"]["role"] = json!("assistant"),
            _ => {
                changed[2]["payload"]["internal_chat_message_metadata_passthrough"]
                    ["content_item_kinds"] = json!(["plugins.recommendations"])
            }
        }
        fs::write(&path, jsonl(&changed)).unwrap();
        f.success(&["miner", "habit", "refresh"]);
        assert!(
            !f.style().contains(BEHAVIOR),
            "changed provenance must withhold a habit: {mutation}"
        );
    }
    fs::write(&path, [original.as_bytes(), &[0xff]].concat()).unwrap();
    f.success(&["miner", "habit", "refresh"]);
    assert!(!f.style().contains(BEHAVIOR));
    fs::write(&path, &original).unwrap();
    f.success(&["miner", "habit", "refresh"]);
    assert!(f.style().contains(BEHAVIOR));

    // Archiving needs a new locator, but does not become independent support.
    let archive = f.codex.join("archived_sessions/rollout-first.jsonl");
    fs::create_dir_all(archive.parent().unwrap()).unwrap();
    fs::rename(&path, &archive).unwrap();
    f.success(&["miner", "habit", "refresh"]);
    assert!(!f.style().contains(BEHAVIOR));
    f.success(&[
        "miner",
        "habit",
        "cite",
        "1",
        "--transcript",
        archive.to_str().unwrap(),
        "--quote",
        QUOTE,
        "--episode",
        "issue-one",
    ]);
    let db = Connection::open(f.db_path()).unwrap();
    let (sources, status): (i64, String) = db
        .query_row(
            "SELECT (SELECT count(*) FROM persona_claim_evidence), status FROM persona_claim",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(sources, 2);
    assert_eq!(status, "stale");
}

#[test]
fn cli_rejects_unbound_quotes_and_colliding_project_slugs() {
    let f = Fixture::new();
    let path = f.write(
        "sessions/2026/09/26/rollout-test.jsonl",
        &jsonl(&f.records("session-a")),
    );
    let other = f.project.with_file_name("project_a");
    fs::create_dir(&other).unwrap();
    assert_eq!(claude_project_slug(&f.project), claude_project_slug(&other));
    assert!(!f.propose(&path, &other).status.success());
    for mutation in 0..3 {
        let mut records = f.records("session-a");
        match mutation {
            0 => records[0]["payload"]["thread_source"] = json!("automation"),
            1 => {
                records[2]["payload"]["internal_chat_message_metadata_passthrough"]["turn_id"] =
                    json!("unbound-turn")
            }
            _ => records[1]["payload"]["cwd"] = json!(other),
        }
        fs::write(&path, jsonl(&records)).unwrap();
        assert!(!f.feedback(&path).status.success());
        assert!(!f.propose(&path, &f.project).status.success());
    }
    let mut records = f.records("session-a");
    records[2]["payload"]["content"][0]["text"] = json!(format!("{QUOTE} X"));
    let text = jsonl(&records);
    let invalid_at = text.find(QUOTE).unwrap() + QUOTE.len() + 1;
    let mut invalid_utf8 = text.into_bytes();
    invalid_utf8[invalid_at] = 0xff;
    fs::write(&path, invalid_utf8).unwrap();
    assert!(!f
        .run(&[
            "miner",
            "feedback",
            "scan",
            "--transcript",
            path.to_str().unwrap()
        ])
        .status
        .success());
    assert!(!f.feedback(&path).status.success());
    assert!(!f.propose(&path, &f.project).status.success());
    assert!(
        !f.db_path().exists(),
        "invalid evidence cannot create a profile"
    );
}

#[test]
fn codex_projects_do_not_inherit_the_claude_directory_slug_limit() {
    let mut f = Fixture::new();
    f.project = f.project.join("nested-project-directory-".repeat(6));
    fs::create_dir_all(&f.project).unwrap();
    assert!(f.project.to_str().unwrap().len() > 128);
    let path = f.write(
        "sessions/2026/09/26/rollout-test.jsonl",
        &jsonl(&f.records("session-a")),
    );
    let feedback = f.feedback(&path);
    assert!(feedback.status.success(), "{feedback:?}");
    let habit = f.propose(&path, &f.project);
    assert!(habit.status.success(), "{habit:?}");
}

#[test]
fn cli_only_admits_the_configured_local_history_layout() {
    let f = Fixture::new();
    let text = jsonl(&f.records("session-a"));
    for relative in [
        "rollout-test.jsonl",
        "sessions/rollout-test.jsonl",
        "sessions/2026/09/26/subagents/rollout-test.jsonl",
        "sessions/2026/99/26/rollout-test.jsonl",
        "sessions/2026/09/26/not-rollout.jsonl",
        "archived_sessions/nested/rollout-test.jsonl",
    ] {
        let path = f.write(relative, &text);
        assert!(!f.feedback(&path).status.success(), "{relative}");
    }
    #[cfg(unix)]
    {
        let outside = f.project.join("rollout-external.jsonl");
        fs::write(&outside, &text).unwrap();
        let link = f.codex.join("sessions/2026/09/26/rollout-link.jsonl");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(!f.feedback(&link).status.success());
    }
    assert!(!f.db_path().exists());
}

#[test]
fn collection_preview_is_read_only_and_collection_does_not_publish_candidates() {
    let f = Fixture::new();
    let path = f.write(
        "sessions/2026/09/26/rollout-collect.jsonl",
        &f.preferences(
            "source-a",
            &[
                "Я предпочитаю короткие ответы без лишних деталей.",
                "Не коммить.",
                "I usually review the contract before changing code.",
            ],
        ),
    );
    let preview = f.collect(&[&path], true);
    assert!(preview.status.success(), "{preview:?}");
    let preview: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(preview["candidates"].as_array().unwrap().len(), 2);
    assert_eq!(preview["scope"], "unresolved");
    assert!(!f.home.join(".mastermind").exists());

    // Preserve a pre-existing style snapshot and its aggregate revision.
    let feedback_path = f.write(
        "sessions/2026/09/26/rollout-feedback.jsonl",
        &jsonl(&f.records("feedback-a")),
    );
    assert!(f.feedback(&feedback_path).status.success());
    let style = f.style();
    let revision = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .aggregate()
        .unwrap()
        .profile_revision();
    let collected = f.collect(&[&path], false);
    assert!(collected.status.success(), "{collected:?}");
    assert_eq!(f.style(), style);
    let aggregate = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .aggregate()
        .unwrap();
    assert_eq!(aggregate.profile_revision(), revision);
    assert_eq!(aggregate.feedback.len(), 1);
    assert!(aggregate.habits.is_empty());
    let inbox = f.inbox();
    assert_eq!(inbox["candidates"].as_array().unwrap().len(), 2);
    for c in inbox["candidates"].as_array().unwrap() {
        assert_eq!(c["freshness"], "current");
        assert_eq!(c["candidate"]["status"], "pending");
        assert_eq!(c["scope"], "unresolved");
        assert!(c["episode"].is_null());
    }
    let repeated = f.collect(&[&path], false);
    let repeated: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(repeated["collection"]["sources_unchanged"], 1);
    assert_eq!(f.inbox()["candidates"].as_array().unwrap().len(), 2);

    let first = &inbox["candidates"][0]["candidate"];
    let rejected = f.run(&[
        "miner",
        "candidates",
        "dismiss",
        first["id"].as_str().unwrap(),
        "--revision",
        first["revision"].as_str().unwrap(),
    ]);
    assert!(
        !rejected.status.success(),
        "non-interactive collection cannot review itself"
    );

    // Seed a prior review, then preview a new snapshot without reviving it.
    let db = Connection::open(f.db_path()).unwrap();
    db.execute(
        "UPDATE persona_candidate SET status = 'dismissed' WHERE id = ?1",
        [first["id"].as_str().unwrap()],
    )
    .unwrap();
    let text = fs::read_to_string(&path).unwrap() + "\n";
    fs::write(&path, text).unwrap();
    let preview = f.collect(&[&path], true);
    assert!(preview.status.success(), "{preview:?}");
    let preview: Value = serde_json::from_slice(&preview.stdout).unwrap();
    let reviewed = preview["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["id"] == first["id"])
        .unwrap();
    assert_eq!(reviewed["status"], "dismissed");
    assert_eq!(f.style(), style);
}

#[test]
fn collection_handles_late_context_archive_and_same_size_rewrites() {
    let f = Fixture::new();
    let mut records = f.records("source-a");
    records[2]["payload"]["content"][0]["text"] = json!("I prefer short replies.");
    let context = records.remove(1);
    let path = f.write(
        "sessions/2026/09/26/rollout-collect.jsonl",
        &jsonl(&records),
    );
    assert!(f.collect(&[&path], false).status.success());
    assert!(f.inbox()["candidates"].as_array().unwrap().is_empty());
    records.push(context);
    fs::write(&path, jsonl(&records)).unwrap();
    assert!(f.collect(&[&path], false).status.success());
    let initial = f.inbox();
    let id = initial["candidates"][0]["candidate"]["id"]
        .as_str()
        .unwrap();
    let revision = initial["candidates"][0]["candidate"]["revision"].clone();

    let archive = f.write("archived_sessions/rollout-copy.jsonl", &jsonl(&records));
    fs::remove_file(&path).unwrap();
    assert_eq!(f.inbox()["candidates"][0]["freshness"], "unavailable");
    assert!(f.collect(&[&archive], false).status.success());
    let archived = f.inbox();
    assert_eq!(archived["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(archived["candidates"][0]["candidate"]["id"], id);
    assert_ne!(archived["candidates"][0]["candidate"]["revision"], revision);
    let shown = f.success(&["miner", "candidates", "show", id]);
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["history"]["revisions"].as_array().unwrap().len(), 2);

    let original = jsonl(&records);
    records[1]["payload"]["content"][0]["text"] = json!("I prefer brief replies.");
    let rewrite = jsonl(&records);
    assert_eq!(original.len(), rewrite.len());
    fs::write(&archive, rewrite).unwrap();
    assert_eq!(f.inbox()["candidates"][0]["freshness"], "changed");
    assert!(f.collect(&[&archive], false).status.success());
    let after = f.inbox();
    let candidates = after["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    assert!(candidates
        .iter()
        .any(|c| c["freshness"] == "removed" && c["candidate"]["id"] == id));
    assert!(candidates.iter().any(
        |c| c["freshness"] == "current" && c["candidate"]["quote"] == "I prefer brief replies."
    ));
}

#[test]
fn collection_aborts_a_failed_batch_without_advancing_successful_checkpoints() {
    let f = Fixture::new();
    let path = f.write(
        "sessions/2026/09/26/rollout-first.jsonl",
        &f.preferences("source-a", &["I prefer short replies."]),
    );
    assert!(f.collect(&[&path], false).status.success());
    let db = Connection::open(f.db_path()).unwrap();
    let old: String = db
        .query_row(
            "SELECT snapshot_digest FROM persona_collection_source",
            [],
            |row| row.get(0),
        )
        .unwrap();
    fs::write(
        &path,
        f.preferences(
            "source-a",
            &[
                "I prefer short replies.",
                "Я обычно проверяю тесты перед ревью.",
            ],
        ),
    )
    .unwrap();
    let broken = f.write("sessions/2026/09/26/rollout-broken.jsonl", "{invalid json");
    let failed = f.collect(&[&path, &broken], false);
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("incomplete"));
    let current: String = db
        .query_row(
            "SELECT snapshot_digest FROM persona_collection_source",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let rows: i64 = db
        .query_row("SELECT count(*) FROM persona_candidate", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(current, old);
    assert_eq!(rows, 1);
    let conflict = f.write(
        "archived_sessions/rollout-conflict.jsonl",
        &f.preferences("source-a", &["I prefer brief replies."]),
    );
    assert!(!f.collect(&[&path, &conflict], false).status.success());
    assert!(f.collect(&[&path], false).status.success());
    assert_eq!(f.inbox()["candidates"].as_array().unwrap().len(), 2);
}

#[test]
fn claude_collection_requires_explicit_origin_and_keeps_content_blocks_separate() {
    let f = Fixture::new();
    let path = f
        .home
        .join(".claude/projects")
        .join(claude_project_slug(&f.project.canonicalize().unwrap()))
        .join("session.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let legacy = json!({"type":"user", "sessionId":"source-a", "cwd":f.project,
        "message":{"content":"I prefer short replies."}});
    fs::write(&path, jsonl(std::slice::from_ref(&legacy))).unwrap();
    assert!(f.collect(&[&path], false).status.success());
    assert!(f.inbox()["candidates"].as_array().unwrap().is_empty());
    let mut marked = legacy;
    marked["origin"] = json!({"kind":"human"});
    let mut mixed = marked.clone();
    mixed["message"]["content"] = json!([
        {"type":"text", "text":"I prefer"}, {"type":"tool_result", "content":"service"},
        {"type":"text", "text":"short code reviews"}
    ]);
    let mut pasted = marked.clone();
    pasted["message"]["content"] = json!("<pasted_content><pasted_content>inner</pasted_content>I prefer lengthy code reviews.</pasted_content>");
    fs::write(&path, jsonl(&[mixed, pasted, marked])).unwrap();
    assert!(f.collect(&[&path], false).status.success());
    let inbox = f.inbox();
    assert_eq!(inbox["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(
        inbox["candidates"][0]["candidate"]["quote"],
        "I prefer short replies."
    );
    assert_eq!(inbox["candidates"][0]["freshness"], "current");
}

#[test]
fn collection_limits_do_not_commit_a_truncated_candidate_set() {
    let f = Fixture::new();
    let messages: Vec<String> = (0..513)
        .map(|i| format!("I prefer reviewing code example {i} first."))
        .collect();
    let refs: Vec<&str> = messages.iter().map(String::as_str).collect();
    let path = f.write(
        "sessions/2026/09/26/rollout-many.jsonl",
        &f.preferences("source-a", &refs),
    );
    let rejected = f.collect(&[&path], false);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("512 candidates"));
    assert!(!f.db_path().exists());
    assert!(f.inbox()["candidates"].as_array().unwrap().is_empty());
}

#[test]
fn failed_first_collection_does_not_create_a_broken_profile() {
    let f = Fixture::new();
    let path = f
        .home
        .join(".claude/projects")
        .join(claude_project_slug(&f.project.canonicalize().unwrap()))
        .join("session.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut record = json!({"type":"user", "sessionId":"source-a", "cwd":".",
        "origin":{"kind":"human"}, "message":{"content":"I prefer short replies."}});
    fs::write(&path, jsonl(std::slice::from_ref(&record))).unwrap();
    assert!(!f.collect(&[&path], false).status.success());
    assert!(!f.db_path().exists());
    let doctor = f.run(&["doctor", "--json"]);
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let style = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "style profile")
        .unwrap();
    assert_eq!(style["status"], "ok", "{style}");

    record["cwd"] = json!(f.project);
    fs::write(&path, jsonl(&[record])).unwrap();
    assert!(f.collect(&[&path], false).status.success());
    assert_eq!(f.inbox()["candidates"].as_array().unwrap().len(), 1);
    assert!(!f.home.join(".mastermind/style.md").exists());
}

#[test]
fn collector_rejects_oversized_metadata_relative_cwd_and_hidden_provenance_changes() {
    let f = Fixture::new();
    let path = f
        .home
        .join(".claude/projects")
        .join(claude_project_slug(&f.project.canonicalize().unwrap()))
        .join("session.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let good = json!({"type":"user", "sessionId":"source-a", "cwd":f.project,
        "origin":{"kind":"human"}, "message":{"content":"I prefer short replies."}});
    fs::write(&path, jsonl(std::slice::from_ref(&good))).unwrap();
    assert!(f.collect(&[&path], false).status.success());
    let old = f.inbox()["candidates"][0]["candidate"].clone();
    for mutation in 0..5 {
        let mut record = good.clone();
        match mutation {
            0 => record["timestamp"] = json!("x".repeat(64000)),
            1 => record["cwd"] = json!("."),
            _ => {}
        }
        let records = if mutation >= 2 {
            let mut assistant = json!({"type":"assistant", "sessionId":"other-session",
                "cwd":f.project, "message":{"content":"x".repeat(70000)}});
            if mutation >= 3 {
                assistant["cwd"] = json!(f.home);
                if mutation == 3 {
                    assistant.as_object_mut().unwrap().remove("sessionId");
                } else {
                    assistant["sessionId"] = Value::Null;
                }
            }
            vec![good.clone(), assistant]
        } else {
            vec![record]
        };
        fs::write(&path, jsonl(&records)).unwrap();
        assert!(
            !f.collect(&[&path], false).status.success(),
            "mutation {mutation}"
        );
        assert_eq!(f.inbox()["candidates"][0]["candidate"], old);
        assert!(ProfileStore::open_read_only(&f.db_path()).is_ok());
    }
}

#[test]
fn candidate_proposal_uses_exact_locator_and_retries_without_new_evidence() {
    let f = Fixture::new();
    let path = f.write(
        "sessions/2026/09/26/rollout-curation.jsonl",
        &f.preferences(
            "source-a",
            &[
                "Anton said: I usually review the contract before changing code.",
                "I usually review the contract before changing code.",
            ],
        ),
    );
    assert!(f.collect(&[&path], false).status.success());
    let candidate = f.inbox()["candidates"][0]["candidate"].clone();
    assert_eq!(candidate["line_no"], 4);
    let result = f.propose_collected(&candidate, "issue-one");
    assert!(result.status.success(), "{result:?}");
    let proposal: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(proposal["proposal"]["status"], "candidate");
    assert_eq!(proposal["proposal"]["repeated"], false);
    let db = ProfileStore::open_read_only(&f.db_path()).unwrap();
    let evidence = db.habit_evidence(1).unwrap();
    assert_eq!(evidence.len(), 1);
    assert_eq!(
        evidence[0].line_no, 4,
        "must not re-find the earlier third-party quote"
    );
    assert_eq!(
        evidence[0].record_digest,
        candidate["record_digest"].as_str().unwrap()
    );
    assert!(!f.style().contains(BEHAVIOR));
    let style = f.style();
    let first_revision = db.aggregate().unwrap().profile_revision();
    drop(db);
    let repeated = f.propose_collected(&candidate, "issue-one");
    assert!(repeated.status.success(), "{repeated:?}");
    let repeated: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(repeated["proposal"]["repeated"], true);
    assert!(!f
        .propose_collected(&candidate, "another-issue")
        .status
        .success());
    assert_eq!(f.style(), style);
    let db = ProfileStore::open_read_only(&f.db_path()).unwrap();
    assert_eq!(db.aggregate().unwrap().profile_revision(), first_revision);
    let raw = Connection::open(f.db_path()).unwrap();
    for table in [
        "persona_claim",
        "persona_claim_evidence",
        "persona_candidate_habit",
        "persona_review_event",
    ] {
        let count: i64 = raw
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1, "{table}");
    }
    let shown = f.success(&[
        "miner",
        "candidates",
        "show",
        candidate["id"].as_str().unwrap(),
    ]);
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(
        shown["history"]["habit_proposals"]["items"][0]["habit_id"],
        1
    );

    // A reviewed or dismissed target cannot be restored by replaying a receipt.
    raw.execute("UPDATE persona_claim SET status='rejected'", [])
        .unwrap();
    assert!(!f
        .propose_collected(&candidate, "issue-one")
        .status
        .success());
    assert_eq!(db.habit(1).unwrap().unwrap().status, "rejected");
    raw.execute("UPDATE persona_claim SET status='candidate'", [])
        .unwrap();
    raw.execute("UPDATE persona_claim_evidence SET status='dismissed'", [])
        .unwrap();
    assert!(!f
        .propose_collected(&candidate, "issue-one")
        .status
        .success());
    assert_eq!(db.habit_evidence(1).unwrap()[0].status, "dismissed");
}

#[test]
fn candidate_proposal_rejects_stale_revisions_and_rolls_back_failed_receipts() {
    let f = Fixture::new();
    let original = f.preferences(
        "source-a",
        &["I usually review the contract before changing code."],
    );
    let path = f.write("sessions/2026/09/26/rollout-curation.jsonl", &original);
    assert!(f.collect(&[&path], false).status.success());
    let candidate = f.inbox()["candidates"][0]["candidate"].clone();
    let mut wrong_revision = candidate.clone();
    wrong_revision["revision"] = json!("f".repeat(64));
    assert!(!f
        .propose_collected(&wrong_revision, "issue-one")
        .status
        .success());
    fs::write(&path, original.replace("before changing", "after changing")).unwrap();
    assert!(!f
        .propose_collected(&candidate, "issue-one")
        .status
        .success());
    fs::write(&path, &original).unwrap();
    let raw = Connection::open(f.db_path()).unwrap();
    raw.execute_batch(
        "CREATE TRIGGER reject_receipt BEFORE INSERT ON persona_candidate_habit
        BEGIN SELECT RAISE(ABORT, 'receipt unavailable'); END;",
    )
    .unwrap();
    assert!(!f
        .propose_collected(&candidate, "issue-one")
        .status
        .success());
    assert!(ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .habits()
        .unwrap()
        .is_empty());
    assert!(!f.home.join(".mastermind/style.md").exists());
    raw.execute_batch(
        "DROP TRIGGER reject_receipt;
        CREATE TABLE padding (data BLOB); INSERT INTO padding VALUES (zeroblob(60000000));
        CREATE TRIGGER large_receipt AFTER INSERT ON persona_candidate_habit
        BEGIN INSERT INTO padding VALUES (zeroblob(8000000)); END;",
    )
    .unwrap();
    let too_large = f.propose_collected(&candidate, "issue-one");
    assert!(!too_large.status.success());
    assert!(
        String::from_utf8_lossy(&too_large.stderr).contains("64 MiB"),
        "{too_large:?}"
    );
    let db = ProfileStore::open_read_only(&f.db_path()).unwrap();
    assert!(db.habits().unwrap().is_empty());
    raw.execute_batch("DROP TRIGGER large_receipt;").unwrap();
    let result = f.propose_collected(&candidate, "issue-one");
    assert!(result.status.success(), "{result:?}");
}

#[test]
#[cfg(unix)]
fn transferred_evidence_keeps_strict_claude_checks_through_review_and_refresh() {
    let f = Fixture::new();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(&f.project)
        .status()
        .unwrap()
        .success());
    let folder = f
        .home
        .join(".claude/projects")
        .join(claude_project_slug(&f.project.canonicalize().unwrap()));
    fs::create_dir_all(&folder).unwrap();
    let mut candidates = Vec::new();
    let mut originals = Vec::new();
    for i in 0..2 {
        let path = folder.join(format!("session-{i}.jsonl"));
        let text = jsonl(&[
            json!({"type":"user", "sessionId":format!("source-{i}"), "cwd":f.project,
            "origin":{"kind":"human"}, "message":{"content":"I usually review the contract before changing code."}}),
        ]);
        fs::write(&path, &text).unwrap();
        assert!(f.collect(&[&path], false).status.success());
        let inbox = f.inbox();
        let candidate = inbox["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["candidate"]["source"] == format!("session:source-{i}"))
            .unwrap()["candidate"]
            .clone();
        let result = f.propose_collected(&candidate, &format!("issue-{i}"));
        assert!(result.status.success(), "{result:?}");
        candidates.push(candidate);
        originals.push((path, text));
    }
    let observed = f.observe_in_fixture_terminal("1");
    assert!(observed.status.success(), "{observed:?}");
    assert!(f.style().contains(BEHAVIOR));
    assert!(f.profile_from_mcp().to_string().contains(BEHAVIOR));
    let third_path = folder.join("third-session.jsonl");
    fs::write(&third_path, originals[0].1.replace("source-0", "source-2")).unwrap();
    assert!(f.collect(&[&third_path], false).status.success());
    let third = f.inbox()["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["candidate"]["source"] == "session:source-2")
        .unwrap()["candidate"]
        .clone();
    let unexpected_support = f.propose_collected(&third, "issue-2");
    assert!(!unexpected_support.status.success());
    assert!(String::from_utf8_lossy(&unexpected_support.stderr).contains("already reviewed"));
    assert_eq!(
        ProfileStore::open_read_only(&f.db_path())
            .unwrap()
            .habit_evidence(1)
            .unwrap()
            .len(),
        2
    );
    let retry = f.propose_collected(&candidates[0], "issue-0");
    assert!(retry.status.success(), "{retry:?}");
    let retry: Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert_eq!(retry["proposal"]["status"], "observed");
    assert_eq!(retry["proposal"]["repeated"], true);

    let (path, original) = &originals[0];
    let changed = format!(
        "{original}{}\n",
        json!({"type":"assistant", "cwd":f.home,
        "message":{"content":"x".repeat(70000)}})
    );
    fs::write(path, changed).unwrap();
    let rejected = f.observe_in_fixture_terminal("1");
    assert!(!rejected.status.success(), "{rejected:?}");
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("habit evidence"));
    assert!(!f.profile_from_mcp().to_string().contains(BEHAVIOR));
    f.success(&["miner", "habit", "refresh"]);
    assert!(!f.style().contains(BEHAVIOR));

    // A later candidate revision can rebind only its own citation and requires
    // observation again. A first transfer cannot augment an observed claim.
    fs::write(path, format!("\n{original}")).unwrap();
    assert!(f.collect(&[path], false).status.success());
    let updated = f.inbox()["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["candidate"]["id"] == candidates[0]["id"])
        .unwrap()["candidate"]
        .clone();
    assert!(!f
        .propose_collected(&candidates[0], "issue-0")
        .status
        .success());
    let rebound = f.propose_collected(&updated, "issue-0");
    assert!(rebound.status.success(), "{rebound:?}");
    let rebound: Value = serde_json::from_slice(&rebound.stdout).unwrap();
    assert_eq!(rebound["proposal"]["status"], "stale");
    assert!(!f.style().contains(BEHAVIOR));
    assert!(f.observe_in_fixture_terminal("1").status.success());
    assert!(f.style().contains(BEHAVIOR));
}
