//! Prompt refinement through the native hook CLI, using only synthetic input,
//! an isolated home and a local protocol fixture. No model or executor is used.
#![cfg(unix)]

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Seek, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const LITERAL_ARG: &str = "--fixture=keep $HOME 'quotes' `literal`";
const PROCESSOR: &str = r#"import json
import os
from pathlib import Path
import sys
import time

harness = Path(sys.argv[1])
assert sys.argv[2] == "--fixture=keep $HOME 'quotes' `literal`"
assert os.environ.get("MASTERMIND_MINER") == "1", "missing recursion guard"
request = json.load(sys.stdin)
assert request["schema"] == 1
source = request["input"]
with (harness / "calls.jsonl").open("a", encoding="utf-8") as stream:
    stream.write(json.dumps({"id": source["id"], "guard": os.environ["MASTERMIND_MINER"]}) + "\n")
(harness / (source["id"] + ".json")).write_text(json.dumps(request, ensure_ascii=False), encoding="utf-8")
mode = (harness / "mode").read_text(encoding="utf-8")
if mode == "blocked":
    (harness / "ready").write_text(source["id"], encoding="utf-8")
    deadline = time.monotonic() + 30
    while not (harness / "release").exists():
        if time.monotonic() >= deadline:
            sys.exit(14)
        time.sleep(0.02)
    mode = "activation"
if mode == "timeout":
    time.sleep(30)
    sys.exit(15)
print("SYNTHETIC_PROCESSOR_DIAGNOSTIC", file=sys.stderr)
if mode == "invalid_json":
    print("{invalid")
    sys.exit(0)
response = {
    "schema": 1,
    "intake_id": source["id"],
    "prompt_digest": source["prompt_digest"],
    "action": "passthrough",
    "workflow_intent": "ordinary",
    "intent_evidence": None,
    "refined_prompt": source["original"],
    "questions": [],
}
if mode == "activation":
    response.update(action="refined", workflow_intent="activate_mastermind",
                    intent_evidence=source["original"],
                    refined_prompt="Inspect the request and draft a plan for approval before implementation.")
elif mode == "continuation":
    assert source["active_task"] is not None
    response.update(workflow_intent="continue_active", intent_evidence=source["original"])
elif mode == "ask":
    response.update(action="ask", workflow_intent="unclear", refined_prompt=None,
                    questions=["Which repository should this request use?"])
elif mode == "wrong_id":
    response["intake_id"] = "0" * 64
elif mode == "wrong_digest":
    response["prompt_digest"] = "0" * 64
elif mode == "unknown_field":
    response["execute_now"] = True
print(json.dumps(response, ensure_ascii=False))
"#;

#[test]
fn profile_uses_only_the_explicit_continuation_scope_and_stays_separate_from_refinement() {
    for client in ["claude", "codex"] {
        let f = Fixture::new("activation");
        f.setup(client, "8");
        assert!(f
            .run(&["miner", "access", "grant", "--client", client])
            .status
            .success());
        let mut store =
            mmcg::miner::store::ProfileStore::open(&f.home.join(".mastermind/style.db")).unwrap();
        let commits = (0..8)
            .map(|index| mmcg::miner::store::CommitEvidence {
                sha: format!("{index:040}"),
                authored_at: "2026-09-01".into(),
                counts: [("decl.const".to_owned(), 3)].into_iter().collect(),
            })
            .collect::<Vec<_>>();
        store
            .upsert_repo(
                "/synthetic",
                &mmcg::miner::store::RepoProvenance {
                    author: "Fixture".into(),
                    commits_total: 8,
                    commits_sampled: 8,
                    added_lines_sampled: 24,
                    latest_sha: None,
                    latest_date: None,
                    mined_at_epoch: 1,
                    extractor: String::new(),
                },
                &[],
                &commits,
                &[],
            )
            .unwrap();
        drop(store);
        f.success(&["miner", "hooks", "setup", "--client", client, "--write"]);
        f.start(client, "task-profile");
        f.native(
            client,
            &f.prompt("task-profile", "one", "Use Mastermind for this task."),
        );
        let first = f.receipts().remove(0);
        let spec = ".mastermind/tasks/001-profile/spec.md";
        fs::create_dir_all(f.project.join(spec).parent().unwrap()).unwrap();
        fs::write(
            f.project.join(spec),
            "---\nmode: verified\ntouches: [{file: src/api.py}]\ncreates: [web/new.ts]\n---\n# Inspect\n",
        )
        .unwrap();
        let binding = f.success(&[
            "miner",
            "hooks",
            "bind-task",
            first["input"]["id"].as_str().unwrap(),
            "--spec",
            spec,
        ]);
        f.native(client, &f.event("task-profile", "one", "Stop"));
        f.mode("continuation");
        let continued = f.native(
            client,
            &f.prompt("task-profile", "two", "Continue the bound task."),
        );
        let context = continued["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.contains("bound_active_task"));
        assert!(context.len() <= 8 * 1024);
        let selected: Value = serde_json::from_str(
            context
                .rsplit("\n\n")
                .next()
                .unwrap()
                .split_once('\n')
                .unwrap()
                .1,
        )
        .unwrap();
        assert_eq!(
            selected["selection"]["paths"],
            json!(["src/api.py", "web/new.ts"])
        );
        assert_eq!(selected["selection"]["role"], "planner");
        assert_eq!(selected["selection"]["workflow"], "verified");
        assert_eq!(selected["task_context"]["source"], "bound_task");
        assert_eq!(
            selected["task_context"]["task_binding_revision"],
            binding["binding"]["revision"]
        );
        f.native(client, &f.event("task-profile", "two", "Stop"));
        f.mode("ordinary");
        let unrelated = f.native(
            client,
            &f.prompt("task-profile", "three", "Inspect src/other.rs."),
        );
        let context = unrelated["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        let selected: Value = serde_json::from_str(
            context
                .rsplit("\n\n")
                .next()
                .unwrap()
                .split_once('\n')
                .unwrap()
                .1,
        )
        .unwrap();
        assert_eq!(selected["selection"]["paths"], json!(["src/other.rs"]));
        assert!(selected["selection"]["workflow"].is_null());
        assert_eq!(
            selected["task_context"]["source"],
            "original_prompt_path_literals"
        );
        assert!(selected["task_context"]["task_binding_revision"].is_null());
        assert_eq!(f.calls().len(), 3);
    }
}

#[test]
fn bound_intake_survives_tool_events_and_routes_only_its_session() {
    let f = Fixture::new("activation");
    f.setup("claude", "8");
    f.start("claude", "bound");
    f.native(
        "claude",
        &f.prompt("bound", "one", "Use Mastermind to inspect this code."),
    );
    let first = f.receipts().remove(0);
    let id = first["input"]["id"].as_str().unwrap();
    let spec = ".mastermind/tasks/001-inspect/spec.md";
    fs::create_dir_all(f.project.join(spec).parent().unwrap()).unwrap();
    fs::write(f.project.join(spec), "# Inspect this code\n").unwrap();
    let mut tool = f.event("bound", "one", "PreToolUse");
    tool["tool_use_id"] = json!("bind-command");
    tool["tool_name"] = json!("Bash");
    tool["tool_input"] = json!({"command":"mastermind miner hooks bind-task"});
    f.native("claude", &tool);
    let args = ["miner", "hooks", "bind-task", id, "--spec", spec];
    let binding = f.success(&args);
    assert_eq!(binding["status"], "bound");
    assert_eq!(binding["binding"]["spec_path"], spec);
    assert_eq!(binding["binding"]["intake_id"], id);
    assert_eq!(
        f.success(&args),
        binding,
        "retry must return the same receipt"
    );
    tool["hook_event_name"] = json!("PostToolUse");
    tool["event_id"] = json!("bound:one:PostToolUse");
    tool["tool_response"] = json!({"exit_code":0});
    f.native("claude", &tool);
    f.native("claude", &f.event("bound", "one", "Stop"));
    f.mode("continuation");
    let response = f.native("claude", &f.prompt("bound", "two", "Continue that task."));
    assert!(response.to_string().contains("bound_active_task"));
    let second = f.receipts().remove(1);
    assert_eq!(second["input"]["active_task"], spec);
    assert_eq!(
        second["task_binding_revision"],
        binding["binding"]["revision"]
    );
    let second_id = second["input"]["id"].as_str().unwrap();
    let continued = f.success(&[
        "miner",
        "hooks",
        "bind-task",
        second_id,
        "--spec",
        spec,
        "--expected-binding",
        binding["binding"]["revision"].as_str().unwrap(),
    ]);
    // Reconstruct a crash after the new marker but before its SQL transaction.
    let mut prepared = continued.clone();
    prepared["status"] = json!("prepared");
    fs::write(
        f.project
            .join(".mastermind/tasks/001-inspect/state.intake.json"),
        prepared.to_string(),
    )
    .unwrap();
    let db = Connection::open(f.home.join(".mastermind/persona-events.db")).unwrap();
    db.execute("DELETE FROM hook_task_binding WHERE intake=?1", [second_id])
        .unwrap();
    let session_id = second["input"]["session_id"].as_str().unwrap();
    let data: String = db
        .query_row(
            "SELECT data FROM hook_session WHERE id=?1",
            [session_id],
            |row| row.get(0),
        )
        .unwrap();
    let mut session: Value = serde_json::from_str(&data).unwrap();
    session["task_epoch"] = second["session_epoch"].clone();
    session["task_binding"] = binding["binding"]["revision"].clone();
    db.execute(
        "UPDATE hook_session SET data=?2 WHERE id=?1",
        rusqlite::params![session_id, session.to_string()],
    )
    .unwrap();
    assert_eq!(
        f.success(&["miner", "hooks", "bind-task", second_id, "--spec", spec]),
        continued
    );
    f.mode("ordinary");
    f.start("claude", "unbound");
    f.native("claude", &f.prompt("unbound", "one", "Continue that task."));
    assert!(f.receipts().remove(2)["input"]["active_task"].is_null());
    let sidecar = fs::read_to_string(
        f.project
            .join(".mastermind/tasks/001-inspect/state.intake.json"),
    )
    .unwrap();
    assert!(
        !sidecar.contains("Use Mastermind"),
        "raw source belongs only in the journal"
    );
    assert!(!f
        .project
        .join(".mastermind/tasks/001-inspect/state.json")
        .exists());
}

#[test]
fn refiner_exposure_is_event_scoped_and_recovery_cannot_erase_it() {
    let f = Fixture::new("ordinary");
    f.setup("codex", "8");
    f.start("codex", "influence");
    let quote = "Before changing an API, check its callers and preserve the contract.";
    let inspect = |turn: &str, expected: &str| {
        let mut prompt = f.prompt("influence", turn, quote);
        prompt["influence"] = json!({"prior_unknown":false,"prior_profile_context":false,"prior_refiner_context":false});
        f.native("codex", &prompt);
        f.native("codex", &f.event("influence", turn, "Stop"));
        let receipt = f.receipts().pop().unwrap();
        let episode = f.success(&[
            "miner",
            "hooks",
            "show",
            receipt["input"]["episode_id"].as_str().unwrap(),
        ])["episode"]
            .clone();
        let event = episode["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["origin"] == "user_channel_unverified")
            .unwrap();
        let response = json!({"schema":1,"episode_id":episode["id"],"episode_revision":episode["revision"],"drafts":[{
            "when":"When changing a public API contract","behavior":"Checks callers before changing an API contract",
            "rationale":null,"outcome":null,"exception":"No exception was observed.","role":null,"workflow":null,
            "evidence_kind":"technical_approach","supports":[{"event_id":event["id"],"quote":quote}],"contradictions":[]}]});
        let output_path = f.harness.join("semantic-response.json");
        fs::write(&output_path, response.to_string()).unwrap();
        let processor = f.harness.join("semantic.py");
        fs::write(&processor, "import sys,json\nfrom pathlib import Path\njson.load(sys.stdin)\nprint(Path(sys.argv[1]).read_text())\n").unwrap();
        let output = f
            .cli()
            .args([
                "miner",
                "hooks",
                "analyze",
                episode["id"].as_str().unwrap(),
                "--revision",
                episode["revision"].as_str().unwrap(),
                "--processor",
            ])
            .arg(&f.python)
            .arg("--processor-arg=-I")
            .arg(format!("--processor-arg={}", processor.display()))
            .arg(format!("--processor-arg={}", output_path.display()))
            .output()
            .unwrap();
        let draft = parse(output)["drafts"][0].clone();
        let inspected = f.success(&["miner", "hooks", "draft", draft["id"].as_str().unwrap()]);
        assert_eq!(inspected["evidence_class"], expected);
        assert_eq!(
            inspected["promotion_eligible"],
            expected == "no_recorded_prior_exposure"
        );
    };
    inspect("first", "no_recorded_prior_exposure");
    inspect("second", "dependent_observation");
    f.success(&["miner", "hooks", "recover", "--client", "codex"]);
    f.start("codex", "influence");
    inspect("recovered", "unknown_influence");
}

#[test]
fn marker_loss_or_a_conflicting_event_blocks_the_first_preflight() {
    let f = Fixture::new("activation");
    f.setup("codex", "8");
    f.start("codex", "source");
    let prompt = f.prompt("source", "one", "Use Mastermind to inspect the code.");
    f.native("codex", &prompt);
    let spec = ".mastermind/tasks/001-marker/spec.md";
    fs::create_dir_all(f.project.join(spec).parent().unwrap()).unwrap();
    fs::write(f.project.join(spec), "# Inspect\n").unwrap();
    let receipt = f.receipts().remove(0);
    let id = receipt["input"]["id"].as_str().unwrap();
    let binding = f.success(&["miner", "hooks", "bind-task", id, "--spec", spec]);
    let marker = f
        .project
        .join(".mastermind/tasks/001-marker/state.intake.json");
    fs::remove_file(&marker).unwrap();
    let run = ["run-task", spec, "--pre-only", "--allow-no-index"];
    let output = f.run(&run);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("marker disappeared"),
        "{output:?}"
    );
    fs::write(marker, binding.to_string()).unwrap();
    let mut conflict = prompt.clone();
    conflict["prompt"] = json!("Use Mastermind for a different scope.");
    f.native("codex", &conflict);
    let output = f.run(&run);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("evidence is incomplete"),
        "{output:?}"
    );
}

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    harness: PathBuf,
    python: PathBuf,
    script: PathBuf,
}

#[test]
fn binding_rejects_ordinary_stale_and_cross_project_sources() {
    let f = Fixture::new("ordinary");
    f.setup("codex", "8");
    f.start("codex", "admission");
    f.native("codex", &f.prompt("admission", "one", "Explain the code."));
    fs::write(f.project.join("spec.md"), "# Inspect\n").unwrap();
    let first = f.receipts().remove(0);
    let id = first["input"]["id"].as_str().unwrap();
    assert!(!f
        .run(&["miner", "hooks", "bind-task", id, "--spec", "spec.md"])
        .status
        .success());
    f.mode("activation");
    f.native(
        "codex",
        &f.prompt("admission", "two", "Use Mastermind for this task."),
    );
    let second = f.receipts().remove(1);
    let id = second["input"]["id"].as_str().unwrap();
    fs::create_dir(f.project.join("nested")).unwrap();
    fs::write(f.project.join("nested/spec.md"), "# Another project\n").unwrap();
    assert!(!f
        .run(&[
            "miner",
            "hooks",
            "bind-task",
            id,
            "--spec",
            "spec.md",
            "--project-root",
            "nested"
        ])
        .status
        .success());
    f.native("codex", &f.event("admission", "two", "Stop"));
    assert!(!f
        .run(&["miner", "hooks", "bind-task", id, "--spec", "spec.md"])
        .status
        .success());
}

#[test]
fn prepared_handoff_blocks_run_and_allows_exact_cas_recovery() {
    let f = Fixture::new("activation");
    f.setup("codex", "8");
    f.start("codex", "before-crash");
    f.native(
        "codex",
        &f.prompt(
            "before-crash",
            "one",
            "Use Mastermind to inspect this code.",
        ),
    );
    let spec = ".mastermind/tasks/001-recover/spec.md";
    fs::create_dir_all(f.project.join(spec).parent().unwrap()).unwrap();
    fs::write(f.project.join(spec), "# Inspect\n").unwrap();
    let first = f.receipts().remove(0);
    let id = first["input"]["id"].as_str().unwrap();
    let bound = f.success(&["miner", "hooks", "bind-task", id, "--spec", spec]);
    let marker = f
        .project
        .join(".mastermind/tasks/001-recover/state.intake.json");
    let mut prepared = bound.clone();
    prepared["status"] = json!("prepared");
    fs::write(&marker, prepared.to_string()).unwrap();
    // Crash after the SQL commit: exact retry completes the local marker.
    f.native("codex", &f.event("before-crash", "", "SessionEnd"));
    assert_eq!(
        f.success(&["miner", "hooks", "bind-task", id, "--spec", spec]),
        bound
    );
    // Crash before SQL publication: durable marker prevents an unbound run.
    fs::write(&marker, prepared.to_string()).unwrap();
    let db = Connection::open(f.home.join(".mastermind/persona-events.db")).unwrap();
    db.execute("DELETE FROM hook_task_binding", []).unwrap();
    let output = f.run(&["run-task", spec, "--pre-only", "--allow-no-index"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("prepared but incomplete"),
        "{output:?}"
    );
    f.start("codex", "after-crash");
    f.native(
        "codex",
        &f.prompt(
            "after-crash",
            "one",
            "Use Mastermind to finish the inspection.",
        ),
    );
    let next = f.receipts().remove(1);
    let next_id = next["input"]["id"].as_str().unwrap();
    assert!(!f
        .run(&["miner", "hooks", "bind-task", next_id, "--spec", spec])
        .status
        .success());
    let recovered = f.success(&[
        "miner",
        "hooks",
        "bind-task",
        next_id,
        "--spec",
        spec,
        "--expected-binding",
        bound["binding"]["revision"].as_str().unwrap(),
    ]);
    assert_ne!(
        recovered["binding"]["revision"],
        bound["binding"]["revision"]
    );
    fs::write(f.project.join("another.md"), "# Another\n").unwrap();
    assert!(!f
        .run(&[
            "miner",
            "hooks",
            "bind-task",
            next_id,
            "--spec",
            "another.md"
        ])
        .status
        .success());
}

#[test]
fn missing_intake_marker_exact_recovery_requires_latest_cas_without_changing_session() {
    let f = Fixture::new("activation");
    f.setup("codex", "8");
    f.start("codex", "missing-marker");
    f.native(
        "codex",
        &f.prompt(
            "missing-marker",
            "one",
            "Use Mastermind to inspect this code.",
        ),
    );
    let intake = f.receipts().remove(0);
    let id = intake["input"]["id"].as_str().unwrap();
    let session = intake["input"]["session_id"].as_str().unwrap();
    let spec = ".mastermind/tasks/001-missing/spec.md";
    let text = "# Inspect this code\n";
    fs::create_dir_all(f.project.join(spec).parent().unwrap()).unwrap();
    fs::write(f.project.join(spec), text).unwrap();
    let bound = f.success(&["miner", "hooks", "bind-task", id, "--spec", spec]);
    let revision = bound["binding"]["revision"].as_str().unwrap();
    let marker = f
        .project
        .join(".mastermind/tasks/001-missing/state.intake.json");
    fs::remove_file(&marker).unwrap();
    let before = f.binding_state(session);
    let missing = f.run(&["miner", "hooks", "bind-task", id, "--spec", spec]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("--expected-binding"));
    assert!(!f
        .run(&[
            "miner",
            "hooks",
            "bind-task",
            id,
            "--spec",
            spec,
            "--expected-binding",
            &"0".repeat(64)
        ])
        .status
        .success());
    assert!(!marker.exists());
    assert_eq!(f.binding_state(session), before);

    fs::write(f.project.join(spec), "# A different task scope\n").unwrap();
    assert!(
        !f.run(&[
            "miner",
            "hooks",
            "bind-task",
            id,
            "--spec",
            spec,
            "--expected-binding",
            revision
        ])
        .status
        .success(),
        "exact recovery must retain the task identity"
    );
    assert!(!marker.exists());
    assert_eq!(f.binding_state(session), before);
    fs::write(f.project.join(spec), text).unwrap();
    let recovered = f.success(&[
        "miner",
        "hooks",
        "bind-task",
        id,
        "--spec",
        spec,
        "--expected-binding",
        revision,
    ]);
    assert_eq!(recovered["status"], "bound");
    assert_eq!(recovered["recovered"], true);
    assert_eq!(recovered["binding"], bound["binding"]);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&marker).unwrap()).unwrap(),
        bound
    );
    assert_eq!(
        f.binding_state(session),
        before,
        "exact recovery only restores the missing local marker"
    );
    assert_eq!(
        f.calls().len(),
        1,
        "recovery must not invoke the refiner again"
    );

    f.native("codex", &f.event("missing-marker", "one", "Stop"));
    f.mode("continuation");
    let continued = f.native(
        "codex",
        &f.prompt("missing-marker", "two", "Continue that task."),
    );
    assert!(continued.to_string().contains("bound_active_task"));
    let intake = f.receipts().remove(1);
    assert_eq!(intake["input"]["active_task"], spec);
    assert_eq!(intake["task_binding_revision"], revision);
    assert_eq!(intake["session_epoch"], before["session"]["task_epoch"]);
}

#[test]
fn missing_intake_marker_replacement_requires_cas_and_rejects_superseded_intakes() {
    let f = Fixture::new("activation");
    f.setup("claude", "8");
    f.start("claude", "replace-marker");
    f.native(
        "claude",
        &f.prompt(
            "replace-marker",
            "one",
            "Use Mastermind to inspect this code.",
        ),
    );
    let first = f.receipts().remove(0);
    let first_id = first["input"]["id"].as_str().unwrap();
    let session = first["input"]["session_id"].as_str().unwrap();
    let spec = ".mastermind/tasks/001-replacement/spec.md";
    fs::create_dir_all(f.project.join(spec).parent().unwrap()).unwrap();
    fs::write(f.project.join(spec), "# Inspect this code\n").unwrap();
    let initial = f.success(&["miner", "hooks", "bind-task", first_id, "--spec", spec]);
    let initial_revision = initial["binding"]["revision"].as_str().unwrap();
    f.native("claude", &f.event("replace-marker", "one", "Stop"));
    f.mode("continuation");
    f.native(
        "claude",
        &f.prompt("replace-marker", "two", "Continue the existing inspection."),
    );
    let next = f.receipts().remove(1);
    let next_id = next["input"]["id"].as_str().unwrap();
    assert_eq!(next["task_binding_revision"], initial_revision);
    let marker = f
        .project
        .join(".mastermind/tasks/001-replacement/state.intake.json");
    fs::remove_file(&marker).unwrap();
    let before = f.binding_state(session);
    for args in [
        vec!["miner", "hooks", "bind-task", next_id, "--spec", spec],
        vec![
            "miner",
            "hooks",
            "bind-task",
            next_id,
            "--spec",
            spec,
            "--expected-binding",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ],
    ] {
        assert!(!f.run(&args).status.success());
        assert!(!marker.exists());
        assert_eq!(f.binding_state(session), before);
    }
    let replacement = f.success(&[
        "miner",
        "hooks",
        "bind-task",
        next_id,
        "--spec",
        spec,
        "--expected-binding",
        initial_revision,
    ]);
    let revision = replacement["binding"]["revision"].as_str().unwrap();
    assert_ne!(revision, initial_revision);
    assert_eq!(
        replacement["binding"]["previous_revision"],
        initial_revision
    );
    assert_eq!(replacement["binding"]["intake_id"], next_id);
    let after = f.binding_state(session);
    assert_eq!(after["bindings"].as_array().unwrap().len(), 2);
    assert_eq!(after["session"]["task_binding"], revision);
    assert_eq!(
        after["session"]["task_epoch"].as_u64().unwrap(),
        before["session"]["task_epoch"].as_u64().unwrap() + 1
    );

    fs::remove_file(&marker).unwrap();
    for expected in [initial_revision, revision] {
        let output = f.run(&[
            "miner",
            "hooks",
            "bind-task",
            first_id,
            "--spec",
            spec,
            "--expected-binding",
            expected,
        ]);
        assert!(
            !output.status.success(),
            "a superseded intake cannot restore an old marker"
        );
        assert!(!marker.exists());
        assert_eq!(f.binding_state(session), after);
    }
    let restored = f.success(&[
        "miner",
        "hooks",
        "bind-task",
        next_id,
        "--spec",
        spec,
        "--expected-binding",
        revision,
    ]);
    assert_eq!(restored["binding"], replacement["binding"]);
    assert_eq!(restored["recovered"], true);
    assert_eq!(f.binding_state(session), after);
    assert_eq!(f.calls().len(), 2);
    f.native("claude", &f.event("replace-marker", "two", "Stop"));
    let native = f.native(
        "claude",
        &f.prompt("replace-marker", "three", "Continue the same task."),
    );
    assert!(native.to_string().contains("bound_active_task"));
    let continuation = f.receipts().remove(2);
    assert_eq!(continuation["input"]["active_task"], spec);
    assert_eq!(continuation["task_binding_revision"], revision);
    assert_eq!(
        continuation["session_epoch"],
        after["session"]["task_epoch"]
    );
    assert!(!f
        .project
        .join(".mastermind/tasks/001-replacement/state.json")
        .exists());
}

#[test]
fn a_changed_active_spec_withholds_an_inflight_continuation() {
    let f = Fixture::new("activation");
    f.setup("claude", "8");
    f.start("claude", "binding");
    f.native(
        "claude",
        &f.prompt("binding", "one", "Use Mastermind to inspect the code."),
    );
    let spec = "spec.md";
    fs::write(f.project.join(spec), "# Inspect\n").unwrap();
    let first = f.receipts().remove(0);
    f.success(&[
        "miner",
        "hooks",
        "bind-task",
        first["input"]["id"].as_str().unwrap(),
        "--spec",
        spec,
    ]);
    f.native("claude", &f.event("binding", "one", "Stop"));
    f.mode("blocked");
    let mut child = f
        .native_command("claude", &f.prompt("binding", "two", "Continue the task."))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f.harness.join("ready").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let pending = f.receipts().remove(1);
    assert_eq!(pending["input"]["active_task"], spec, "{pending}");
    fs::write(f.project.join(spec), "# A different scope\n").unwrap();
    fs::write(f.harness.join("release"), "").unwrap();
    assert!(child.wait().unwrap().success());
    let receipt = f.receipts().remove(1);
    assert_eq!(receipt["status"], "withheld");
    assert_eq!(receipt["reason"], "admission_changed");
}

impl Fixture {
    fn new(mode: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        let harness = temp.path().join("processor-fixture");
        for path in [&home, &project, &harness] {
            fs::create_dir(path).unwrap();
        }
        // Python is already used by the Unix CI workflow. Resolve it once so
        // the tested processor does not depend on shell expansion or PATH.
        let interpreter = Command::new("python3")
            .args(["-I", "-c", "import sys; print(sys.executable)"])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &home)
            .output()
            .expect("the Unix CLI fixture requires Python 3");
        assert!(interpreter.status.success(), "{interpreter:?}");
        let python = PathBuf::from(String::from_utf8(interpreter.stdout).unwrap().trim())
            .canonicalize()
            .unwrap();
        let script = harness.join("processor.py");
        fs::write(&script, PROCESSOR).unwrap();
        fs::write(harness.join("mode"), mode).unwrap();
        let f = Self {
            home: home.canonicalize().unwrap(),
            project: project.canonicalize().unwrap(),
            harness: harness.canonicalize().unwrap(),
            python,
            script,
            _temp: temp,
        };
        let output = f
            .isolated("/usr/bin/git")
            .args(["init", "--quiet", "--initial-branch=main"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        f
    }

    fn isolated(&self, executable: impl AsRef<std::ffi::OsStr>) -> Command {
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
        self.isolated(env!("CARGO_BIN_EXE_mmcg"))
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cli().args(args).output().unwrap()
    }

    fn success(&self, args: &[&str]) -> Value {
        parse(self.run(args))
    }

    fn setup_command(&self, client: &str, timeout: &str) -> Command {
        let mut command = self.cli();
        command
            .args([
                "miner",
                "hooks",
                "setup",
                "--client",
                client,
                "--write",
                "--refiner-timeout",
                timeout,
                "--refiner-processor",
            ])
            .arg(&self.python)
            .arg("--refiner-arg=-I")
            .arg(format!("--refiner-arg={}", self.script.display()))
            .arg(format!("--refiner-arg={}", self.harness.display()))
            .arg(format!("--refiner-arg={LITERAL_ARG}"));
        command
    }

    fn setup(&self, client: &str, timeout: &str) -> Value {
        parse(self.setup_command(client, timeout).output().unwrap())
    }

    fn config(&self, client: &str) -> PathBuf {
        self.project.join(match client {
            "codex" => ".codex/hooks.json",
            "claude" => ".claude/settings.local.json",
            _ => unreachable!(),
        })
    }

    fn mode(&self, mode: &str) {
        fs::write(self.harness.join("mode"), mode).unwrap();
    }

    fn event(&self, session: &str, turn: &str, kind: &str) -> Value {
        let mut event = json!({
            "session_id":session,
            "event_id":format!("{session}:{turn}:{kind}"),
            "hook_event_name":kind,
            "cwd":self.project,
        });
        if !turn.is_empty() {
            event["turn_id"] = json!(turn);
        }
        if kind == "SessionStart" {
            event["source"] = json!("startup");
        }
        event
    }

    fn prompt(&self, session: &str, turn: &str, original: &str) -> Value {
        let mut event = self.event(session, turn, "UserPromptSubmit");
        event["prompt"] = json!(original);
        event
    }

    fn native_command(&self, client: &str, event: &Value) -> Command {
        // A seekable input also permits a guard to exit before reading stdin.
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
            .arg(&self.project)
            .stdin(Stdio::from(input))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn native(&self, client: &str, event: &Value) -> Value {
        parse(self.native_command(client, event).output().unwrap())
    }

    fn start(&self, client: &str, session: &str) {
        assert_eq!(
            self.native(client, &self.event(session, "", "SessionStart")),
            json!({})
        );
    }

    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(self.harness.join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn receipts(&self) -> Vec<Value> {
        let db = Connection::open_with_flags(
            self.home.join(".mastermind/persona-events.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut statement = db
            .prepare("SELECT data FROM hook_intake ORDER BY rowid")
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
            .collect()
    }

    fn binding_state(&self, session: &str) -> Value {
        let db = Connection::open_with_flags(
            self.home.join(".mastermind/persona-events.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut statement = db
            .prepare("SELECT data FROM hook_task_binding ORDER BY rowid")
            .unwrap();
        let bindings = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| serde_json::from_str::<Value>(&row.unwrap()).unwrap())
            .collect::<Vec<_>>();
        let data: String = db
            .query_row(
                "SELECT data FROM hook_session WHERE id=?1",
                [session],
                |row| row.get(0),
            )
            .unwrap();
        json!({"bindings":bindings, "session":serde_json::from_str::<Value>(&data).unwrap()})
    }

    fn intake(&self, id: &str) -> Value {
        self.success(&["miner", "hooks", "intake", id])
    }

    fn request(&self, id: &str) -> Value {
        serde_json::from_slice(&fs::read(self.harness.join(format!("{id}.json"))).unwrap()).unwrap()
    }

    fn assert_no_executor_or_profile_write(&self) {
        assert!(!self.project.join(".mastermind/tasks").exists());
        assert!(!self.home.join(".mastermind/style.db").exists());
        assert!(!self.home.join(".mastermind/style.md").exists());
    }

    fn leave_capture_marker(&self, client: &str) -> PathBuf {
        // Simulate a process that persisted its admission marker, then failed
        // before SQLite begin_capture. Public status verifies the condition.
        let mut digest = Sha256::new();
        digest.update(b"mastermind-persona-capture-grant-v1\0");
        digest.update(self.project.to_str().unwrap().as_bytes());
        let hash: String = digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let directory = self.home.join(".mastermind/.persona-capture");
        fs::create_dir_all(&directory).unwrap();
        let marker = directory.join(format!("{client}-{hash}-{:032x}.pending", 1_u128));
        fs::write(&marker, b"").unwrap();
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
        marker
    }
}

fn parse(output: Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error}: {output:?}"))
}

fn packet(native: &Value) -> Value {
    assert_eq!(native.as_object().unwrap().len(), 1, "{native}");
    let hook = &native["hookSpecificOutput"];
    assert_eq!(hook["hookEventName"], "UserPromptSubmit");
    let context = hook["additionalContext"].as_str().unwrap();
    let (guidance, data) = context.split_once('\n').unwrap();
    assert!(guidance.to_lowercase().contains("advisory"), "{guidance}");
    let result: Value = serde_json::from_str(data).unwrap();
    assert_eq!(result["kind"], "mastermind_prompt_intake");
    assert_eq!(result["payload"]["status"], "inline");
    result
}

fn assert_timeouts(config: &Value, prompt_timeout: u64) {
    let events = config["hooks"].as_object().unwrap();
    assert!(events.len() >= 9);
    for (name, groups) in events {
        let handlers = groups[0]["hooks"].as_array().unwrap();
        assert_eq!(handlers.len(), 1);
        assert_eq!(
            handlers[0]["timeout"],
            if name == "UserPromptSubmit" {
                prompt_timeout
            } else {
                3
            },
            "{name}"
        );
        assert_ne!(handlers[0]["async"], true, "{name}");
    }
}

struct PendingNative {
    child: Option<Child>,
    release: PathBuf,
}

impl PendingNative {
    fn spawn(f: &Fixture, client: &str, event: &Value) -> Self {
        Self {
            child: Some(f.native_command(client, event).spawn().unwrap()),
            release: f.harness.join("release"),
        }
    }

    fn finish(mut self) -> Output {
        fs::write(&self.release, "release").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.child.as_mut().unwrap().try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "native hook did not finish");
            std::thread::sleep(Duration::from_millis(20));
        }
        self.child.take().unwrap().wait_with_output().unwrap()
    }
}

impl Drop for PendingNative {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Unblock the fixture before cancellation, including assertion
            // failures. The product supervises its own processor subprocess.
            let _ = fs::write(&self.release, "release");
            unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
            let deadline = Instant::now() + Duration::from_secs(3);
            while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fs::metadata(path).is_ok_and(|metadata| metadata.len() == 64) {
        assert!(
            Instant::now() < deadline,
            "missing coordination file {path:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn setup_budgets_both_clients_preserves_refiner_and_disable_keeps_capture() {
    for client in ["codex", "claude"] {
        let f = Fixture::new("ordinary");
        for timeout in ["0", "21"] {
            let output = f.setup_command(client, timeout).output().unwrap();
            assert!(!output.status.success(), "{output:?}");
            assert!(!f.config(client).exists());
        }
        let configured = f.setup(client, "4");
        assert_eq!(configured["refiner"]["status"], "configured");
        assert_eq!(configured["refiner"]["timeout_seconds"], 4);
        let before = fs::read(f.config(client)).unwrap();
        assert_timeouts(&serde_json::from_slice(&before).unwrap(), 7);
        let repeated = f.success(&["miner", "hooks", "setup", "--client", client, "--write"]);
        assert_eq!(repeated["refiner"], configured["refiner"]);
        assert_eq!(fs::read(f.config(client)).unwrap(), before);
        f.start(client, "configured");
        let response = f.native(
            client,
            &f.prompt("configured", "one", "Explain this function."),
        );
        assert_eq!(packet(&response)["route"], "native_work");
        assert_eq!(f.calls().len(), 1);

        f.success(&[
            "miner",
            "hooks",
            "setup",
            "--client",
            client,
            "--write",
            "--disable-refiner",
        ]);
        let status = f.success(&["miner", "hooks", "status", "--client", client]);
        assert_eq!(status["grant"]["enabled"], true);
        assert_eq!(status["refiner"]["status"], "not_configured");
        assert_timeouts(
            &serde_json::from_slice(&fs::read(f.config(client)).unwrap()).unwrap(),
            3,
        );
        f.start(client, "capture-only");
        assert_eq!(
            f.native(
                client,
                &f.prompt("capture-only", "two", "Keep collecting this observation.")
            ),
            json!({})
        );
        assert_eq!(f.calls().len(), 1);
        assert_eq!(f.receipts().len(), 1);
        assert_eq!(
            f.success(&["miner", "hooks", "episodes"])["episodes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        // Selecting the built-in adapter in a dry run must not contact it.
        let preview = f.success(&[
            "miner",
            "hooks",
            "setup",
            "--client",
            client,
            "--refiner-provider",
            "claude",
        ]);
        assert_eq!(preview["written"], false);
        assert_eq!(preview["refiner"]["provider"], "claude");
        assert_eq!(preview["refiner"]["timeout_seconds"], 8);
        assert_timeouts(&preview["hook_config"], 11);
        f.assert_no_executor_or_profile_write();
    }
}

#[test]
fn ordinary_unicode_prompt_is_durably_preserved_and_only_advisory_context_is_offered() {
    let original =
        "  Привет, Алина 👩‍💻.\nCompare café and cafe\u{301}.\tKeep $HOME; 'quotes' literal.  ";
    for client in ["codex", "claude"] {
        let f = Fixture::new("ordinary");
        f.setup(client, "8");
        f.start(client, "unicode");
        let native = f.native(client, &f.prompt("unicode", "one", original));
        let context = packet(&native);
        assert_eq!(context["action"], "passthrough");
        assert_eq!(context["workflow_intent"], "ordinary");
        assert_eq!(context["route"], "native_work");
        assert_eq!(
            context["payload"]["refined_prompt"]
                .as_str()
                .unwrap()
                .as_bytes(),
            original.as_bytes()
        );
        let id = context["intake_id"].as_str().unwrap();
        let receipt = f.intake(id);
        assert_eq!(receipt["schema"], 1);
        assert_eq!(receipt["status"], "offered");
        assert_eq!(receipt["input"]["id"], id);
        assert_eq!(receipt["input"]["client"], client);
        assert_eq!(receipt["input"]["project_root"], json!(f.project));
        assert_eq!(receipt["input"]["active_task"], Value::Null);
        assert_eq!(
            receipt["input"]["original"].as_str().unwrap().as_bytes(),
            original.as_bytes()
        );
        let digest: String = Sha256::digest(original.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(receipt["input"]["prompt_digest"], digest);
        assert_eq!(context["prompt_digest"], receipt["input"]["prompt_digest"]);
        assert_eq!(f.receipts(), vec![receipt.clone()]);
        assert_eq!(f.request(id)["input"], receipt["input"]);
        assert_eq!(f.calls(), vec![json!({"id":id,"guard":"1"})]);
        let shown = f.success(&[
            "miner",
            "hooks",
            "show",
            receipt["input"]["episode_id"].as_str().unwrap(),
        ]);
        assert!(shown["episode"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| {
                event["id"] == receipt["input"]["event_id"]
                    && event["text"] == original
                    && event["origin"] == "user_channel_unverified"
            }));
        f.assert_no_executor_or_profile_write();
    }
}

#[test]
fn fixture_classifications_route_activation_to_planning_and_uncertainty_to_questions() {
    // The fake supplies the intent label. These examples test transport and
    // routing, not whether any model understands English or Russian correctly.
    for (original, mode, route) in [
        (
            "Начни с мастермайнда: сначала разберись и составь план.",
            "activation",
            "mastermind-task-planning",
        ),
        (
            "Use Mastermind to plan the change before implementation.",
            "activation",
            "mastermind-task-planning",
        ),
        ("Сделай это.", "ask", "questions"),
    ] {
        let f = Fixture::new(mode);
        f.setup("codex", "8");
        f.start("codex", "intent");
        let context = packet(&f.native("codex", &f.prompt("intent", "one", original)));
        assert_eq!(context["route"], route);
        let receipt = f.intake(context["intake_id"].as_str().unwrap());
        assert_eq!(receipt["input"]["original"], original);
        if mode == "activation" {
            assert_eq!(context["workflow_intent"], "activate_mastermind");
            assert_eq!(context["action"], "refined");
            assert_eq!(context["payload"]["intent_evidence"], original);
            assert_ne!(context["payload"]["refined_prompt"], original);
        } else {
            assert_eq!(context["action"], "ask");
            assert_eq!(context["workflow_intent"], "unclear");
            assert_eq!(context["payload"]["refined_prompt"], Value::Null);
            assert_eq!(
                context["payload"]["questions"],
                json!(["Which repository should this request use?"])
            );
        }
        assert_eq!(f.calls().len(), 1);
        f.assert_no_executor_or_profile_write();
    }
}

#[test]
fn malformed_binding_and_timeout_failures_preserve_original_without_workflow_handoff() {
    let original = "Keep this exact request unchanged if refinement fails: café, пример.";
    for mode in [
        "invalid_json",
        "wrong_id",
        "wrong_digest",
        "unknown_field",
        "timeout",
    ] {
        let f = Fixture::new(mode);
        f.setup("codex", if mode == "timeout" { "1" } else { "8" });
        f.start("codex", "fallback");
        let native = f.native("codex", &f.prompt("fallback", "one", original));
        let receipts = f.receipts();
        assert_eq!(receipts.len(), 1, "{mode}");
        let receipt = &receipts[0];
        assert_eq!(receipt["status"], "degraded", "{mode}: {receipt}");
        assert_eq!(receipt["input"]["original"], original);
        assert_eq!(receipt["response"], Value::Null);
        assert_eq!(receipt["reason"], "processor_failed_or_invalid_response");
        assert_eq!(f.intake(receipt["input"]["id"].as_str().unwrap()), *receipt);
        let context = native["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.contains(receipt["input"]["id"].as_str().unwrap()));
        assert!(context.contains("degraded"));
        assert!(!context.contains("mastermind-task-planning"));
        assert!(!context.contains("SYNTHETIC_PROCESSOR_DIAGNOSTIC"));
        assert_eq!(f.calls().len(), 1);
        f.assert_no_executor_or_profile_write();
    }
}

#[test]
fn replay_closed_turn_and_conflicting_native_identity_never_repeat_the_processor() {
    let f = Fixture::new("activation");
    f.setup("codex", "8");
    f.start("codex", "replay");
    let event = f.prompt("replay", "one", "Use Mastermind to plan this change.");
    let offered = packet(&f.native("codex", &event));
    let original_receipt = f.intake(offered["intake_id"].as_str().unwrap());
    assert_eq!(f.native("codex", &event), json!({}));
    assert_eq!(
        f.native("codex", &f.event("replay", "one", "Stop")),
        json!({})
    );
    assert_eq!(f.native("codex", &event), json!({}));
    let mut conflict = event.clone();
    conflict["prompt"] = json!("A different request under the same native event id.");
    assert_eq!(f.native("codex", &conflict), json!({}));
    assert_eq!(f.calls().len(), 1);
    assert_eq!(f.receipts(), vec![original_receipt.clone()]);
    let shown = f.success(&[
        "miner",
        "hooks",
        "show",
        original_receipt["input"]["episode_id"].as_str().unwrap(),
    ]);
    assert!(shown["episode"]["coverage_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|gap| gap == "event_identity_conflict"));
}

#[test]
fn incomplete_closed_and_automated_sessions_skip_processing_and_generated_guards_skip_capture() {
    for mode in ["missing_start", "ended_session", "automated"] {
        let f = Fixture::new("activation");
        f.setup("codex", "8");
        if mode != "missing_start" {
            f.start("codex", "inadmissible");
        }
        if mode == "ended_session" {
            assert_eq!(
                f.native("codex", &f.event("inadmissible", "", "SessionEnd")),
                json!({})
            );
        }
        let mut prompt = f.prompt("inadmissible", "one", "Use Mastermind to plan this change.");
        if mode == "automated" {
            prompt["is_automated"] = json!(true);
        }
        f.native("codex", &prompt);
        assert!(f.calls().is_empty(), "{mode}");
        let receipts = f.receipts();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0]["status"], "degraded");
        assert_eq!(receipts[0]["reason"], "capture_not_admitted");
        f.assert_no_executor_or_profile_write();
    }
    let f = Fixture::new("activation");
    f.setup("codex", "8");
    f.start("codex", "generated");
    for (name, value) in [
        ("MASTERMIND_MINER", "1"),
        ("MMCG_INPUT_ORIGIN", "controller"),
    ] {
        let event = f.prompt(
            "generated",
            name,
            "Use Mastermind to plan this generated request.",
        );
        let output = f
            .native_command("codex", &event)
            .env(name, value)
            .output()
            .unwrap();
        assert_eq!(parse(output), json!({}));
    }
    assert!(f.receipts().is_empty());
    assert!(f.calls().is_empty());
    assert_eq!(
        f.success(&["miner", "hooks", "episodes"])["episodes"],
        json!([])
    );
}

#[test]
fn revocation_or_a_new_prompt_withholds_a_blocked_processors_old_result() {
    for invalidation in ["revoke", "new_prompt"] {
        let f = Fixture::new("blocked");
        f.setup("codex", "20");
        f.start("codex", "race");
        let original = "Use Mastermind to plan the first request.";
        let running = PendingNative::spawn(&f, "codex", &f.prompt("race", "first", original));
        wait_for(&f.harness.join("ready"));
        let id = fs::read_to_string(f.harness.join("ready")).unwrap();
        assert_eq!(f.intake(&id)["status"], "pending");
        if invalidation == "revoke" {
            f.success(&[
                "miner", "hooks", "setup", "--client", "codex", "--write", "--remove",
            ]);
        } else {
            f.mode("ordinary");
            let next = packet(&f.native(
                "codex",
                &f.prompt("race", "second", "Explain this second request."),
            ));
            assert_eq!(next["route"], "native_work");
            assert_ne!(next["intake_id"], id);
        }
        assert_eq!(parse(running.finish()), json!({}), "{invalidation}");
        let receipt = f.intake(&id);
        assert_eq!(receipt["status"], "withheld", "{invalidation}: {receipt}");
        assert_eq!(receipt["input"]["original"], original);
        assert_eq!(receipt["response"], Value::Null);
        assert_eq!(receipt["reason"], "admission_changed");
        assert_eq!(
            f.calls().len(),
            if invalidation == "revoke" { 1 } else { 2 }
        );
        f.assert_no_executor_or_profile_write();
    }
}

#[test]
fn forgetting_an_episode_removes_its_durable_intake_and_raw_prompt() {
    let f = Fixture::new("ordinary");
    f.setup("codex", "8");
    f.start("codex", "forget");
    let context = packet(&f.native(
        "codex",
        &f.prompt("forget", "one", "Synthetic private observation to forget."),
    ));
    let id = context["intake_id"].as_str().unwrap();
    let receipt = f.intake(id);
    let episode = receipt["input"]["episode_id"].as_str().unwrap();
    f.native("codex", &f.event("forget", "one", "Stop"));
    let current = f.success(&["miner", "hooks", "show", episode]);
    f.success(&[
        "miner",
        "hooks",
        "forget",
        episode,
        "--revision",
        current["episode"]["revision"].as_str().unwrap(),
    ]);
    assert!(!f.run(&["miner", "hooks", "intake", id]).status.success());
    assert!(!f.run(&["miner", "hooks", "show", episode]).status.success());
    assert!(f.receipts().is_empty());
    assert_eq!(
        f.success(&["miner", "hooks", "episodes"])["episodes"],
        json!([])
    );
    assert_eq!(f.calls().len(), 1);
    f.assert_no_executor_or_profile_write();
}

#[test]
fn durable_capture_fences_block_refinement_before_processing_and_before_publication() {
    let f = Fixture::new("ordinary");
    f.setup("codex", "8");
    f.start("codex", "prior-fence");
    let marker = f.leave_capture_marker("codex");
    let status = f.success(&["miner", "hooks", "status", "--client", "codex"]);
    assert_eq!(status["grant"]["pending"], 0);
    assert_eq!(status["capture_pending"], true);
    let native = f.native(
        "codex",
        &f.prompt("prior-fence", "one", "Explain this synthetic function."),
    );
    assert!(
        f.calls().is_empty(),
        "a durable capture gap must prevent model input"
    );
    assert!(f
        .receipts()
        .iter()
        .all(|receipt| receipt["status"] != "offered" && receipt["status"] != "pending"));
    assert!(!native.to_string().contains("mastermind_prompt_intake"));
    assert!(
        marker.exists(),
        "ordinary delivery must not clear a prior marker"
    );

    let f = Fixture::new("blocked");
    f.setup("codex", "20");
    f.start("codex", "publication-fence");
    let running = PendingNative::spawn(
        &f,
        "codex",
        &f.prompt(
            "publication-fence",
            "one",
            "Use Mastermind to plan this change.",
        ),
    );
    wait_for(&f.harness.join("ready"));
    let id = fs::read_to_string(f.harness.join("ready")).unwrap();
    assert_eq!(f.intake(&id)["status"], "pending");
    let marker = f.leave_capture_marker("codex");
    let status = f.success(&["miner", "hooks", "status", "--client", "codex"]);
    assert_eq!(status["grant"]["pending"], 0);
    assert_eq!(status["capture_pending"], true);
    assert_eq!(parse(running.finish()), json!({}));
    let receipt = f.intake(&id);
    assert_eq!(receipt["status"], "withheld");
    assert_eq!(receipt["response"], Value::Null);
    assert_eq!(f.calls().len(), 1);
    assert!(marker.exists());
    f.assert_no_executor_or_profile_write();
}

#[test]
fn disabling_refiner_revokes_processing_even_when_native_configuration_is_malformed() {
    let f = Fixture::new("activation");
    f.setup("codex", "8");
    let broken = b"{invalid native config";
    fs::write(f.config("codex"), broken).unwrap();
    let output = f.run(&[
        "miner",
        "hooks",
        "setup",
        "--client",
        "codex",
        "--write",
        "--disable-refiner",
    ]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(fs::read(f.config("codex")).unwrap(), broken);
    let status = f.success(&["miner", "hooks", "status", "--client", "codex"]);
    assert_eq!(status["refiner"]["status"], "not_configured");
    assert_eq!(status["grant"]["enabled"], true);
    f.start("codex", "capture-remains");
    let event = f.prompt(
        "capture-remains",
        "one",
        "Use Mastermind to plan this change.",
    );
    assert_eq!(f.native("codex", &event), json!({}));
    assert!(f.calls().is_empty());
    assert!(f.receipts().is_empty());
    assert_eq!(
        f.success(&["miner", "hooks", "episodes"])["episodes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    f.assert_no_executor_or_profile_write();
}
