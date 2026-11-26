use mmcg::{
    audit_spec::{self, Bundle, Finding, Verdict},
    executor_report,
    indexer::Indexer,
    lessons, run_task, spec,
    store::{Store, WorkBudget},
    verify_spec,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MIB: usize = 1024 * 1024;
const SPEC: &str = ".mastermind/tasks/001-declared-files/spec.md";
const DOC: &str = "docs/guide.md";
const VERIFY: &str = "git --version > .mastermind/should-not-run";

struct Fixture {
    store: Store,
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for args in [
            vec!["init", "-q", "--initial-branch=main"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "commit.gpgsign", "false"],
            vec!["config", "core.autocrlf", "false"],
        ] {
            git(root, &args);
        }
        std::fs::write(root.join(".gitignore"), "graph.db*\n.mastermind/\n").unwrap();
        std::fs::write(root.join("service.py"), "def keep():\n    return 1\n").unwrap();
        std::fs::create_dir(root.join("docs")).unwrap();
        std::fs::write(root.join(DOC), "# Guide\nBefore.\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "baseline"]);
        git(root, &["tag", "baseline"]);
        let mut store = Store::open(root.join("graph.db")).unwrap();
        Indexer::new(root).index_all(&mut store, false).unwrap();
        let fixture = Self { store, directory };
        fixture.write_spec(&[DOC]);
        fixture
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }
    fn spec(&self) -> PathBuf {
        self.root().join(SPEC)
    }
    fn state(&self) -> mmcg::task_record::RunState {
        mmcg::task_record::load_state(&mmcg::task_record::state_file_path(
            self.root(),
            &self.spec(),
        ))
        .unwrap()
        .unwrap()
    }
    fn audit_evidence(&self) -> Value {
        let body = std::fs::read_to_string(self.spec().with_file_name("audit.md")).unwrap();
        serde_json::from_str(
            body.split_once("Structured audit evidence (JSON; null means not evaluated):")
                .unwrap()
                .1,
        )
        .unwrap()
    }
    fn write(&self, path: &str, bytes: &[u8]) {
        let path = self.root().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    fn write_spec(&self, docs: &[&str]) {
        let metadata = json!({
            "mode": "lite", "touches": [{"file": "service.py", "symbols": ["keep"]}],
            "expected_docs": docs, "verify": [{"cmd": VERIFY}],
        });
        self.write(SPEC, format!("---\n{}---\n# Update documentation\n## Goals\nUpdate the service and required documents.\n## Phase 1: update\nDescribe the changed behavior.\n", serde_norway::to_string(&metadata).unwrap()).as_bytes());
    }
    fn write_creation_spec(&self, mode: &str, touches: &[&str], creates: &[&str], docs: &[&str]) {
        let metadata = json!({
            "mode": mode,
            "touches": touches.iter().map(|file| json!({"file":file,"symbols":["keep"]})).collect::<Vec<_>>(),
            "creates": creates, "expected_docs": docs, "verify": [{"cmd":VERIFY}],
        });
        let mut body = format!(
            "---\n{}---\n# Create files\n",
            serde_norway::to_string(&metadata).unwrap()
        );
        for heading in verify_spec::mandatory_sections_for_mode(Some(mode)) {
            body.push_str(&format!("\n## {heading}\nCreate the declared code and documentation and check their behavior.\n"));
        }
        body.push_str("\n## Phase 1: create\nImplement the new files.\n");
        self.write(SPEC, body.as_bytes());
    }
    fn refresh(&mut self) {
        Indexer::new(self.root())
            .index_all(&mut self.store, false)
            .unwrap();
    }
    fn change(&mut self) {
        self.write("service.py", b"def keep():\n    return 2\n");
        self.write(DOC, b"# Guide\nAfter.\n");
        self.refresh();
    }
    fn report(&self, files: &[&str]) -> executor_report::ExecutorReport {
        let value = json!({
            "schema_version": 1, "spec": SPEC, "status": "complete",
            "phases": [{"id": "1", "status": "done"}], "files_modified": files,
            "claims": [], "defects": [], "verifications": [{"cmd": VERIFY, "result": "pass", "observed": {"exit_code": 0}}],
        });
        std::fs::write(
            self.spec().with_file_name("executor-report.md"),
            value.to_string(),
        )
        .unwrap();
        executor_report::parse_canonical_str(&value.to_string()).unwrap()
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mmcg"));
        command
            .current_dir(self.root())
            .arg("--index")
            .arg(self.root().join("graph.db"))
            .env("MMCG_QUERY_BUDGET_MS", "60000");
        command
    }
    fn verify_from(&self, cwd: &Path) -> Output {
        self.command()
            .current_dir(cwd)
            .arg("verify-spec")
            .arg(self.spec())
            .arg("--root")
            .arg(self.root())
            .args(["--require-index", "--json"])
            .output()
            .unwrap()
    }
    fn verify(&self, success: bool) -> Value {
        json_output(self.verify_from(self.root()), success)
    }
    fn audit(&self, success: bool) -> Value {
        json_output(
            self.command()
                .arg("audit-spec")
                .arg(self.spec())
                .args(["--since", "baseline", "--json"])
                .output()
                .unwrap(),
            success,
        )
    }
    fn audit_with_report(&self, success: bool) -> Value {
        json_output(
            self.command()
                .arg("audit-spec")
                .arg(self.spec())
                .args(["--since", "baseline", "--json", "--executor-report"])
                .arg(self.spec().with_file_name("executor-report.md"))
                .output()
                .unwrap(),
            success,
        )
    }
    fn ci(&self) -> Output {
        self.command()
            .args([
                "ci",
                "--since",
                "baseline",
                "--require-executor-report",
                "--bundle-dir",
            ])
            .arg(self.root().join(".mastermind/bundles"))
            .output()
            .unwrap()
    }
    fn controller(&self, pre_only: bool) -> run_task::Outcome {
        self.controller_with_strict(pre_only, false)
    }
    fn controller_with_strict(&self, pre_only: bool, strict: bool) -> run_task::Outcome {
        run_task::run(
            &self.spec(),
            self.root(),
            &self.root().join("graph.db"),
            run_task::RunOpts {
                pre_only,
                post_only: !pre_only,
                strict,
                ..Default::default()
            },
        )
    }
    fn policy(&self, success: bool) -> Value {
        json_output(
            self.command()
                .args([
                    "policy",
                    "check",
                    "--since",
                    "baseline",
                    "--config",
                    ".mastermind/policy.yml",
                    "--format",
                    "sarif",
                ])
                .output()
                .unwrap(),
            success,
        )
    }
    #[cfg(unix)]
    fn write_reviewed_spec(&self) {
        let metadata = json!({
            "mode": "verified", "touches": [{"file": "service.py", "symbols": ["keep"]}],
            "verify": [{"cmd": "git --version", "run": {"id": "unit", "argv": ["git", "--version"], "cwd": ".", "timeout_secs": 60}}],
            "acceptance": [{"id": "service-result", "statement": "The service changed.", "checks": ["unit"]}],
        });
        let mut body = format!(
            "---\n{}---\n# Reviewed change\n",
            serde_norway::to_string(&metadata).unwrap()
        );
        for heading in verify_spec::mandatory_sections_for_mode(Some("verified")) {
            body.push_str(&format!(
                "\n## {heading}\nChange the service and run the declared check.\n"
            ));
        }
        self.write(SPEC, body.as_bytes());
    }
    #[cfg(unix)]
    fn report_reviewed(&self) {
        let value = json!({
            "schema_version": 1, "spec": SPEC, "status": "complete", "phases": [],
            "files_modified": ["service.py"], "claims": [], "defects": [],
            "verifications": [{"cmd": "git --version", "result": "pass", "observed": {"exit_code": 0}}],
        });
        std::fs::write(
            self.spec().with_file_name("executor-report.md"),
            value.to_string(),
        )
        .unwrap();
    }
    #[cfg(unix)]
    fn run_unit_check(&self) {
        let output = self
            .command()
            .args(["verification", "run"])
            .arg(self.spec())
            .args(["--id", "unit", "--json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    fn write_setup(&self, budgets: &[(&str, u64)]) {
        let mut setup =
            serde_json::to_value(mmcg::onboarding::Settings::local(self.root())).unwrap();
        for (key, value) in budgets {
            setup[key] = json!(value);
        }
        self.write(
            ".mastermind/setup.json",
            &serde_json::to_vec(&setup).unwrap(),
        );
    }
    fn change_service(&mut self) {
        self.write("service.py", b"def keep():\n    return 2\n");
        self.refresh();
    }
    #[cfg(unix)]
    fn remove_setup(&self) {
        std::fs::remove_file(self.root().join(".mastermind/setup.json")).unwrap();
    }
    fn write_blobs(&self, count: u8) {
        for index in 0..count {
            self.write(&format!("data/blob-{index}.bin"), &vec![index; 5 * MIB]);
        }
    }
    #[cfg(unix)]
    fn plain_controller(&self) -> Output {
        self.command()
            .arg("run-task")
            .arg(self.spec())
            .arg("--root")
            .arg(self.root())
            .output()
            .unwrap()
    }
    fn state_json(&self) -> Value {
        serde_json::from_slice(
            &std::fs::read(mmcg::task_record::state_file_path(
                self.root(),
                &self.spec(),
            ))
            .unwrap(),
        )
        .unwrap()
    }
    fn active_approval(&self) -> Value {
        let pin = self.state_json()["preflight_approval_sha256"]
            .as_str()
            .unwrap()
            .to_string();
        serde_json::from_slice(
            &std::fs::read(
                self.spec()
                    .with_file_name("preflight-approvals")
                    .join(format!("{pin}.json")),
            )
            .unwrap(),
        )
        .unwrap()
    }
    fn controller_from(&self, cwd: &Path, pre_only: bool) -> Output {
        self.command()
            .current_dir(cwd)
            .arg("run-task")
            .arg(self.spec())
            .arg("--root")
            .arg(self.root())
            .arg(if pre_only {
                "--pre-only"
            } else {
                "--post-only"
            })
            .output()
            .unwrap()
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn json_output(output: Output, success: bool) -> Value {
    assert_eq!(output.status.success(), success, "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn file_error(report: &Value, file: Option<&str>, reason: Option<&str>) {
    assert_eq!(report["verdict"], "broken", "{report}");
    assert!(
        report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| {
                finding["kind"] == "declared_file_unavailable"
                    && finding["file"] == json!(file)
                    && reason.is_none_or(|reason| finding["reason"] == reason)
            }),
        "{report}"
    );
}

#[test]
fn creates_public_workflow_retains_baseline_across_drafts_commits_and_recovery() {
    let mut fixture = Fixture::new();
    let doc = "docs/new.md";
    fixture.write_creation_spec("strict", &[], &["new.py", doc], &["./docs\\new.md"]);
    assert_eq!(fixture.verify(true)["verdict"], "pass");
    assert_eq!(
        fixture.controller_with_strict(true, true),
        run_task::Outcome::PreReady
    );
    let baseline = fixture.state().baseline_ref;
    let initial_worktree = serde_json::to_value(fixture.state().initial_worktree).unwrap();
    assert!(!fixture.root().join("new.py").exists());
    fixture.write("new.py", b"def added():\n    return 2\n");
    fixture.write(doc, b"# New guide\nAdded behavior.\n");
    fixture.refresh();
    fixture.report(&["new.py", doc]);
    assert_eq!(fixture.verify(true)["verdict"], "pass");
    assert_eq!(fixture.audit(true)["verdict"], "held");
    assert_eq!(
        fixture.controller_with_strict(true, true),
        run_task::Outcome::PreReady
    );
    assert_eq!(fixture.state().baseline_ref, baseline);
    assert_eq!(
        serde_json::to_value(fixture.state().initial_worktree).unwrap(),
        initial_worktree
    );
    assert_eq!(fixture.controller(false), run_task::Outcome::PostHeld);
    let held = fixture.state();
    assert!(held.held_snapshot_sha256.is_some() && held.history_snapshot_sha256.is_some());
    git(fixture.root(), &["add", "new.py", doc]);
    assert_eq!(fixture.audit(true)["verdict"], "held");
    git(fixture.root(), &["commit", "-qm", "create files"]);
    assert_eq!(
        fixture.controller_with_strict(true, true),
        run_task::Outcome::PreReady
    );
    assert_eq!(fixture.state().baseline_ref, baseline);
    assert_eq!(
        serde_json::to_value(fixture.state().initial_worktree).unwrap(),
        initial_worktree
    );
    assert_eq!(fixture.controller(false), run_task::Outcome::PostHeld);
    assert_eq!(
        fixture.state().held_snapshot_sha256,
        held.held_snapshot_sha256
    );
    let ci = fixture.ci();
    assert!(ci.status.success(), "{ci:?}");
    let bundle: Value = serde_json::from_slice(
        &std::fs::read(
            fixture
                .root()
                .join(".mastermind/bundles/001-declared-files.bundle.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(bundle["schema_version"], 3);
    assert_eq!(bundle["manifest"]["verdict"], "held");
    assert_eq!(bundle["manifest"]["declared_files"], json!([doc, "new.py"]));
    assert_eq!(bundle["manifest"]["discrepancies"], json!([]));
    std::fs::remove_file(fixture.root().join(doc)).unwrap();
    fixture.refresh();
    file_error(&fixture.audit(false), Some(doc), Some("target_missing"));
    assert_eq!(fixture.controller(false), run_task::Outcome::PostBroken);
    let failed = fixture.state();
    assert!(failed.held_snapshot_sha256.is_none() && failed.history_snapshot_sha256.is_none());
    assert_eq!(failed.baseline_ref, baseline);
    fixture.write(doc, b"# New guide\nAdded behavior.\n");
    fixture.refresh();
    assert_eq!(fixture.controller(false), run_task::Outcome::PostHeld);
    assert_eq!(fixture.state().baseline_ref, baseline);
    assert!(!fixture.root().join(".mastermind/should-not-run").exists());
}

#[test]
fn controller_attributes_only_changes_after_the_initial_worktree() {
    let mut fixture = Fixture::new();
    fixture.write_spec(&[]);
    fixture.write("service.py", b"def keep():\n    return 8\n");
    fixture.write(DOC, b"Prior local documentation.\n");
    fixture.write("local-notes.md", b"Prior local notes.\n");
    fixture.write("unfinished.py", b"def unfinished(\n");
    fixture.write("deleted-before.txt", b"Prior committed file.\n");
    git(fixture.root(), &["add", "deleted-before.txt"]);
    git(fixture.root(), &["commit", "-qm", "prior file"]);
    std::fs::remove_file(fixture.root().join("deleted-before.txt")).unwrap();
    assert_eq!(fixture.controller(true), run_task::Outcome::PreReady);
    fixture.write("service.py", b"def keep():\n    return 2\n");
    fixture.refresh();
    fixture.report(&["service.py"]);
    assert_eq!(fixture.controller(false), run_task::Outcome::PostHeld);
    let audit = fixture.audit_evidence();
    assert_eq!(audit["symbol_diff"]["files_in_diff"], json!(["service.py"]));
    assert_eq!(audit["ownership"]["basis"], "initial_worktree");
    assert_eq!(
        audit["ownership"]["unchanged_prior_files"],
        json!(["deleted-before.txt", DOC, "local-notes.md", "unfinished.py"])
    );
    let release = std::fs::read_to_string(mmcg::task_record::release_file_path(
        fixture.root(),
        &fixture.spec(),
    ))
    .unwrap();
    assert!(release.contains("service.py"));
    for prior in [DOC, "deleted-before.txt", "local-notes.md", "unfinished.py"] {
        assert!(!release.contains(prior), "{release}");
    }
    assert_eq!(
        std::fs::read(fixture.root().join("local-notes.md")).unwrap(),
        b"Prior local notes.\n"
    );
    assert_eq!(
        fixture.audit_with_report(false)["verdict"],
        "broken",
        "standalone audit must retain its whole-change contract"
    );
}

#[test]
fn controller_does_not_exempt_modified_removed_or_restored_prior_work() {
    for action in ["edit", "delete", "restore", "mode", "new-on-retry"] {
        if action == "mode" && !cfg!(unix) {
            continue;
        }
        let mut fixture = Fixture::new();
        fixture.write_spec(&[]);
        fixture.write(DOC, b"Prior local documentation.\n");
        fixture.write("local-notes.md", b"Prior local notes.\n");
        assert_eq!(fixture.controller(true), run_task::Outcome::PreReady);
        let initial = serde_json::to_value(fixture.state().initial_worktree).unwrap();
        fixture.write("service.py", b"def keep():\n    return 2\n");
        let changed = match action {
            "edit" => {
                fixture.write("local-notes.md", b"Changed prior notes.\n");
                "local-notes.md"
            }
            "delete" => {
                std::fs::remove_file(fixture.root().join("local-notes.md")).unwrap();
                "local-notes.md"
            }
            "restore" => {
                fixture.write(DOC, b"# Guide\nBefore.\n");
                DOC
            }
            "mode" => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        fixture.root().join("local-notes.md"),
                        std::fs::Permissions::from_mode(0o755),
                    )
                    .unwrap();
                }
                "local-notes.md"
            }
            "new-on-retry" => {
                fixture.write("new-local.md", b"New undeclared work.\n");
                "new-local.md"
            }
            _ => unreachable!(),
        };
        fixture.refresh();
        fixture.report(&["service.py"]);
        if action == "new-on-retry" {
            assert_eq!(fixture.controller(true), run_task::Outcome::PreReady);
            assert_eq!(
                serde_json::to_value(fixture.state().initial_worktree).unwrap(),
                initial
            );
        }
        assert_eq!(
            fixture.controller(false),
            run_task::Outcome::PostBroken,
            "{action}"
        );
        let audit = fixture.audit_evidence();
        assert!(
            audit["symbol_diff"]["files_in_diff"]
                .as_array()
                .unwrap()
                .contains(&json!(changed)),
            "{action}: {audit}"
        );
        let text = std::fs::read_to_string(fixture.spec().with_file_name("audit.md")).unwrap();
        assert!(
            text.contains(&format!("unexpected_file: `{changed}`")),
            "{action}: {text}"
        );
        assert!(
            text.contains(&format!(
                "executor_report_missing_changed_file: `{changed}`"
            )),
            "{action}: {text}"
        );
    }
}

#[test]
fn controller_checks_added_claims_against_dirty_source_preimages() {
    let mut fixture = Fixture::new();
    fixture.write(
        "draft.py",
        b"def keep():\n    return 1\n\ndef obsolete():\n    return 0\n",
    );
    fixture.write_creation_spec("verified", &["draft.py"], &[], &[]);
    let body = std::fs::read_to_string(fixture.spec()).unwrap().replacen(
        "mode: verified\n",
        "mode: verified\nbreaking_changes:\n  removed_symbols:\n    - name: obsolete\n      file: draft.py\n",
        1,
    );
    fixture.write(SPEC, body.as_bytes());
    assert_eq!(fixture.controller(true), run_task::Outcome::PreReady);
    fixture.write(
        "draft.py",
        b"def keep():\n    return 1\n\ndef fresh():\n    return 2\n",
    );
    fixture.refresh();
    fixture.report(&["draft.py"]);
    let report_path = fixture.spec().with_file_name("executor-report.md");
    let mut report: Value = serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    report["claims"] = json!([
        {"kind":"function_added", "symbol":"keep", "file":"draft.py"},
        {"kind":"function_added", "symbol":"fresh", "file":"draft.py"}
    ]);
    std::fs::write(&report_path, report.to_string()).unwrap();
    assert_eq!(fixture.controller(false), run_task::Outcome::PostBroken);
    let audit = fixture.audit_evidence();
    assert_eq!(
        audit["claim_checks"][0]["finding"]["kind"],
        "claimed_symbol_not_added"
    );
    assert_eq!(audit["claim_checks"][1]["status"], "verified");
    report["claims"] = json!([{"kind":"function_added", "symbol":"fresh", "file":"draft.py"}]);
    std::fs::write(&report_path, report.to_string()).unwrap();
    assert_eq!(fixture.controller(false), run_task::Outcome::PostHeld);
}

#[test]
fn controller_never_infers_legacy_ownership_or_accepts_missing_preimages() {
    for case in ["legacy", "missing-preimage", "changed-preimage"] {
        let mut fixture = Fixture::new();
        fixture.write_spec(&[]);
        fixture.write("local-notes.md", b"Prior local notes.\n");
        assert_eq!(fixture.controller(true), run_task::Outcome::PreReady);
        let baseline = fixture.state().baseline_ref;
        if case == "legacy" {
            let state_path = mmcg::task_record::state_file_path(fixture.root(), &fixture.spec());
            let mut state: Value =
                serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
            state.as_object_mut().unwrap().remove("initial_worktree");
            state
                .as_object_mut()
                .unwrap()
                .remove("preflight_approval_sha256");
            std::fs::write(&state_path, state.to_string()).unwrap();
        } else {
            let initial = fixture.state().initial_worktree.unwrap();
            let hash = match &initial.entries[0].state {
                mmcg::task_worktree::PathState::Regular { sha256, .. } => sha256,
                _ => panic!("expected notes preimage"),
            };
            let preimage = fixture.spec().with_file_name("preflight-inputs").join(hash);
            if case == "missing-preimage" {
                std::fs::remove_file(preimage).unwrap();
            } else {
                std::fs::write(preimage, b"Changed retained input.\n").unwrap();
            }
        }
        fixture.write("service.py", b"def keep():\n    return 2\n");
        fixture.refresh();
        fixture.report(&["service.py"]);
        if case == "legacy" {
            assert_eq!(fixture.controller(true), run_task::Outcome::PreReady);
            assert!(fixture.state().initial_worktree.is_none());
        }
        assert_eq!(
            fixture.controller(false),
            run_task::Outcome::PostBroken,
            "{case}"
        );
        assert_eq!(fixture.state().baseline_ref, baseline);
        assert!(!mmcg::task_record::release_file_path(fixture.root(), &fixture.spec()).exists());
    }
}

#[test]
fn creates_postflight_rejects_baseline_files_even_when_untracked() {
    let mut fixture = Fixture::new();
    fixture.write_creation_spec("lite", &[], &["service.py"], &[]);
    assert_eq!(fixture.verify(true)["verdict"], "pass");
    fixture.write("service.py", b"def keep():\n    return 2\n");
    fixture.refresh();
    fixture.report(&["service.py"]);
    file_error(
        &fixture.audit(false),
        Some("service.py"),
        Some("creation_exists_in_baseline"),
    );
    git(fixture.root(), &["rm", "--cached", "service.py"]);
    fixture.refresh();
    file_error(
        &fixture.audit(false),
        Some("service.py"),
        Some("creation_exists_in_baseline"),
    );
    let ci = fixture.ci();
    assert!(!ci.status.success(), "{ci:?}");
    assert!(
        String::from_utf8_lossy(&ci.stderr).contains("creation_exists_in_baseline"),
        "{ci:?}"
    );
    fixture.write_creation_spec("lite", &[], &["docs"], &[]);
    std::fs::remove_file(fixture.root().join(DOC)).unwrap();
    std::fs::remove_dir(fixture.root().join("docs")).unwrap();
    fixture.write("docs", b"a file replacing a baseline tree");
    fixture.refresh();
    fixture.report(&["service.py", "docs", DOC]);
    file_error(
        &fixture.audit(false),
        Some("docs"),
        Some("creation_exists_in_baseline"),
    );
}

#[test]
fn creates_public_gates_reject_invalid_metadata_and_declaration_conflicts() {
    let fixture = Fixture::new();
    for creates in [
        "new.py",
        "null",
        "[null]",
        "[true]",
        "[7]",
        "[{file: new.py}]",
        "[new.py",
    ] {
        fixture.write_creation_spec("lite", &[], &[], &[]);
        let body = std::fs::read_to_string(fixture.spec())
            .unwrap()
            .replace("creates: []", &format!("creates: {creates}"));
        fixture.write(SPEC, body.as_bytes());
        fixture.report(&[]);
        let checked = fixture.verify(false);
        assert!(
            checked["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["kind"] == "declared_file_unavailable"
                    && e["file"].is_null()
                    && e["reason"] == "frontmatter_invalid"),
            "{checked}"
        );
        file_error(&fixture.audit(false), None, Some("frontmatter_invalid"));
        assert_eq!(fixture.controller(true), run_task::Outcome::PreFailed);
        let ci = fixture.ci();
        assert!(!ci.status.success(), "{ci:?}");
        assert!(
            String::from_utf8_lossy(&ci.stderr).contains("frontmatter_invalid"),
            "{ci:?}"
        );
    }
    for (touches, creates, docs, reason) in [
        (
            vec!["./service.py"],
            vec!["service.py"],
            vec![],
            "creation_touch_conflict",
        ),
        (
            vec![],
            vec!["new", "new/file.py"],
            vec![],
            "declaration_ancestor_conflict",
        ),
        (
            vec![],
            vec!["new"],
            vec!["new/guide.md"],
            "declaration_ancestor_conflict",
        ),
        (vec![], vec!["../new.py"], vec![], "target_path_invalid"),
    ] {
        fixture.write_creation_spec("lite", &touches, &creates, &docs);
        let checked = fixture.verify(false);
        assert!(
            checked["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["kind"] == "declared_file_unavailable" && e["reason"] == reason),
            "{checked}"
        );
        assert_eq!(fixture.audit(false)["verdict"], "broken");
    }
}

#[test]
fn creates_strict_controller_and_policy_bind_created_bytes() {
    for mixed in [false, true] {
        let mut fixture = Fixture::new();
        let touches = if mixed { vec!["service.py"] } else { vec![] };
        fixture.write_creation_spec("strict", &touches, &["./new.py"], &[]);
        fixture.write(
            ".mastermind/policy.yml",
            b"rules:\n  - id: critical\n    critical: new.py\n    require_workflow: strict\n",
        );
        assert_eq!(
            fixture.controller_with_strict(true, true),
            run_task::Outcome::PreReady
        );
        fixture.write("new.py", b"def added():\n    return 2\n");
        let mut files = vec!["new.py"];
        if mixed {
            fixture.write("service.py", b"def keep():\n    return 2\n");
            files.push("service.py");
        }
        fixture.refresh();
        fixture.report(&files);
        assert_eq!(fixture.controller(false), run_task::Outcome::PostHeld);
        let held = fixture.state().held_snapshot_sha256;
        assert!(held.is_some());
        assert_eq!(
            fixture.policy(true)["runs"][0]["properties"]["passed"],
            true
        );
        fixture.write("new.py", b"def added():\n    return 3\n");
        fixture.refresh();
        let failed = fixture.policy(false);
        assert_eq!(failed["runs"][0]["properties"]["passed"], false);
        assert!(
            failed["runs"][0]["results"]
                .as_array()
                .unwrap()
                .iter()
                .any(|result| result["ruleId"] == "critical"),
            "{failed}"
        );
        fixture.report(&files);
        assert_eq!(fixture.controller(false), run_task::Outcome::PostHeld);
        assert_ne!(fixture.state().held_snapshot_sha256, held);
        assert_eq!(
            fixture.policy(true)["runs"][0]["properties"]["passed"],
            true
        );
    }
}

#[test]
fn creates_aliases_resolve_root_from_nested_cwd() {
    let mut fixture = Fixture::new();
    fixture.write_creation_spec("strict", &[], &["./new\\file.py"], &[]);
    let nested = fixture.root().join(".mastermind/nested");
    std::fs::create_dir_all(nested.join("new/file.py")).unwrap();
    assert_eq!(
        json_output(fixture.verify_from(&nested), true)["verdict"],
        "pass"
    );
    let pre = fixture.controller_from(&nested, true);
    assert!(pre.status.success(), "{pre:?}");
    fixture.write("new/file.py", b"def added():\n    return 1\n");
    fixture.refresh();
    fixture.report(&["new/file.py"]);
    assert_eq!(fixture.controller(false), run_task::Outcome::PostHeld);
    let held = fixture.state();
    let post = fixture.controller_from(&nested, false);
    assert!(post.status.success(), "{post:?}");
    assert_eq!(
        fixture.state().history_snapshot_sha256,
        held.history_snapshot_sha256
    );
    assert_eq!(
        fixture.state().held_snapshot_sha256,
        held.held_snapshot_sha256
    );
}

#[test]
fn creates_do_not_waive_find_or_verification_contracts() {
    let mut fixture = Fixture::new();
    fixture.write_creation_spec("lite", &[], &["new.py"], &[]);
    let body = std::fs::read_to_string(fixture.spec()).unwrap();
    fixture.write(
        SPEC,
        format!("{body}\n**File:** `new.py`\nFIND:\n```python\nneedle\n```\n").as_bytes(),
    );
    let checked = fixture.verify(false);
    assert!(
        checked["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "find_block_unavailable" && e["reason"] == "target_missing"),
        "{checked}"
    );
    fixture.write("new.py", b"def added():\n    return 'needle'\n");
    fixture.refresh();
    assert_eq!(fixture.verify(true)["verdict"], "pass");
    fixture.write("new.py", b"def added():\n    return 2\n");
    fixture.refresh();
    fixture.report(&["new.py"]);
    let report_path = fixture.spec().with_file_name("executor-report.md");
    let mut report: Value = serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    report["verifications"] = json!([]);
    std::fs::write(&report_path, report.to_string()).unwrap();
    let broken = fixture.audit_with_report(false);
    assert_eq!(broken["verdict"], "broken");
    assert!(
        broken["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "verification_requirement_unmet"
                && f["cmd"] == VERIFY
                && f["reason"] == "missing_result"),
        "{broken}"
    );
    fixture.report(&["new.py"]);
    assert_eq!(fixture.audit_with_report(true)["verdict"], "held");
}

#[test]
fn declared_files_cli_preserves_binary_assets_and_path_aliases() {
    let fixture = Fixture::new();
    fixture.write_spec(&["./docs\\guide.md"]);
    assert_eq!(fixture.verify(true)["verdict"], "pass");
    fixture.write(".mastermind/asset.bin", &[0xff, 0, 0xfe]);
    std::fs::OpenOptions::new()
        .write(true)
        .open(fixture.root().join(".mastermind/asset.bin"))
        .unwrap()
        .set_len(mmcg::indexer::MAX_INDEXABLE_FILE_SIZE + 1)
        .unwrap();
    fixture.write_spec(&[".mastermind/asset.bin"]);
    assert_eq!(fixture.verify(true)["verdict"], "pass");
    let body = std::fs::read_to_string(fixture.spec()).unwrap();
    fixture.write(
        SPEC,
        format!("{body}\nDo not touch `unrelated-missing.md`.\n").as_bytes(),
    );
    assert_eq!(fixture.verify(true)["verdict"], "pass");
}

#[test]
fn declared_files_cli_rejects_invalid_paths_nonfiles_and_missing_targets() {
    let mut fixture = Fixture::new();
    fixture.write("service.py", b"def keep():\n    return 2\n");
    fixture.refresh();
    fixture.report(&["service.py"]);
    let outside = tempfile::tempdir_in(fixture.root().parent().unwrap()).unwrap();
    std::fs::write(outside.path().join("guide.md"), "# Outside\n").unwrap();
    let absolute = outside
        .path()
        .join("guide.md")
        .to_str()
        .unwrap()
        .to_string();
    let parent = format!(
        "../{}/guide.md",
        outside.path().file_name().unwrap().to_str().unwrap()
    );
    for path in [
        "",
        ".",
        "..",
        "a/../guide.md",
        "C:/guide.md",
        "C:guide.md",
        absolute.as_str(),
        parent.as_str(),
    ] {
        fixture.write_spec(&[path]);
        let verification = fixture.verify(false);
        assert_eq!(
            verification["errors"],
            json!([{
                "kind": "declared_file_unavailable", "file": if path.is_empty() { None } else { Some(path) }, "reason": "target_path_invalid",
            }]),
            "{verification}"
        );
        file_error(
            &fixture.audit(false),
            if path.is_empty() { None } else { Some(path) },
            Some("target_path_invalid"),
        );
    }
    fixture.write_spec(&["docs"]);
    let directory = fixture.verify(false);
    assert_eq!(directory["errors"][0]["kind"], "declared_file_unavailable");
    file_error(&fixture.audit(false), Some("docs"), None);
    fixture.write_spec(&["missing.md"]);
    assert_eq!(
        fixture.verify(false)["errors"],
        json!([{"kind": "missing_file", "file": "missing.md"}])
    );
    file_error(
        &fixture.audit(false),
        Some("missing.md"),
        Some("target_missing"),
    );
}

#[test]
fn declared_docs_controller_rejects_deletion_clears_approval_and_recovers() {
    let mut fixture = Fixture::new();
    assert_eq!(fixture.controller(true), run_task::Outcome::PreReady);
    let baseline = fixture.state().baseline_ref;
    fixture.change();
    fixture.report(&["service.py", DOC]);
    assert_eq!(fixture.controller(false), run_task::Outcome::PostHeld);
    let mut held = fixture.state();
    assert!(held.history_snapshot_sha256.is_some());
    held.held_snapshot_sha256 = Some("old-held".into());
    mmcg::task_record::save_state_in_repository(
        fixture.root(),
        &mmcg::task_record::state_file_path(fixture.root(), &fixture.spec()),
        &held,
    )
    .unwrap();
    std::fs::remove_file(fixture.root().join(DOC)).unwrap();
    fixture.refresh();
    let audited = fixture.audit(false);
    file_error(&audited, Some(DOC), Some("target_missing"));
    assert_eq!(
        audited["findings"].as_array().unwrap().len(),
        1,
        "{audited}"
    );
    assert_eq!(fixture.controller(false), run_task::Outcome::PostBroken);
    let failed = fixture.state();
    assert_eq!(failed.baseline_ref, baseline);
    assert_eq!(failed.next_step.as_deref(), Some("planner_review"));
    assert!(failed.held_snapshot_sha256.is_none() && failed.history_snapshot_sha256.is_none());
    let ci = fixture.ci();
    assert!(!ci.status.success(), "{ci:?}");
    assert!(
        String::from_utf8_lossy(&ci.stderr).contains("missing_file"),
        "{ci:?}"
    );
    assert!(!fixture
        .root()
        .join(".mastermind/bundles/001-declared-files.bundle.json")
        .exists());
    fixture.write(DOC, b"# Guide\nAfter.\n");
    fixture.refresh();
    assert_eq!(fixture.controller(false), run_task::Outcome::PostHeld);
    assert_eq!(fixture.state().baseline_ref, baseline);
    git(fixture.root(), &["add", "service.py", DOC]);
    git(
        fixture.root(),
        &["commit", "-q", "-m", "update service and guide"],
    );
    let ci = fixture.ci();
    assert!(ci.status.success(), "{ci:?}");
    let bundle: Value = serde_json::from_slice(
        &std::fs::read(
            fixture
                .root()
                .join(".mastermind/bundles/001-declared-files.bundle.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(bundle["schema_version"], 3);
    assert_eq!(bundle["manifest"]["verdict"], "held");
    assert_eq!(bundle["manifest"]["discrepancies"], json!([]));
    assert!(!fixture.root().join(".mastermind/should-not-run").exists());
}

#[test]
fn declared_docs_aliases_and_history_receipts_ignore_nested_cwd_shadows() {
    let mut fixture = Fixture::new();
    fixture.write_spec(&["./docs\\guide.md"]);
    fixture.write(
        ".mastermind/nested/docs/guide.md",
        b"# Shadow\nDifferent bytes.\n",
    );
    fixture.write(".mastermind/nested/service.py", b"shadow source\n");
    let nested = fixture.root().join(".mastermind/nested");
    assert_eq!(
        json_output(fixture.verify_from(&nested), true)["verdict"],
        "pass"
    );
    let pre = fixture.controller_from(&nested, true);
    assert!(pre.status.success(), "{pre:?}");
    fixture.change();
    fixture.report(&["service.py", DOC]);
    assert_eq!(fixture.controller(false), run_task::Outcome::PostHeld);
    let first = fixture.state();
    let post = fixture.controller_from(&nested, false);
    assert!(post.status.success(), "{post:?}");
    let repeated = fixture.state();
    assert_eq!(repeated.baseline_ref, first.baseline_ref);
    assert_eq!(
        repeated.history_snapshot_sha256,
        first.history_snapshot_sha256
    );
    std::fs::remove_file(fixture.root().join(DOC)).unwrap();
    fixture.refresh();
    let post = fixture.controller_from(&nested, false);
    assert!(!post.status.success(), "{post:?}");
    assert!(fixture.state().history_snapshot_sha256.is_none());
}

#[test]
fn declared_file_failure_preserves_bundle_severity_and_lesson_evidence() {
    let mut fixture = Fixture::new();
    fixture.write("service.py", b"def keep():\n    return 2\n");
    fixture.refresh();
    git(fixture.root(), &["add", "service.py"]);
    git(fixture.root(), &["commit", "-q", "-m", "update service"]);
    fixture.write_spec(&["../outside.md"]);
    let executor = fixture.report(&["service.py"]);
    let parsed = spec::parse_file(&fixture.spec()).unwrap();
    let report = audit_spec::run_with_report(
        &parsed,
        &fixture.store,
        fixture.root(),
        "baseline",
        Some(&executor),
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Broken);
    assert_eq!(report.findings.len(), 1, "{report:?}");
    assert!(
        matches!(&report.findings[0], Finding::DeclaredFileUnavailable { file, reason } if file.as_deref() == Some("../outside.md") && reason == "target_path_invalid")
    );
    assert!(report.render_text().contains("declared_file_unavailable"));
    let report_path = fixture.spec().with_file_name("executor-report.md");
    let bundle = Bundle::from_report_full(
        &report,
        Some(&executor),
        Some(&parsed),
        report_path.to_str(),
        Some(fixture.root()),
    );
    assert_eq!(bundle.spec_files, ["service.py"]);
    assert_eq!(bundle.verdict, "broken");
    assert!(
        bundle.human_summary.contains("1 errors, 0 warnings"),
        "{}",
        bundle.human_summary
    );
    assert!(bundle.snapshot_drift.is_empty());
    let manifest = bundle.into_manifest(fixture.root()).unwrap();
    assert_eq!(manifest.verdict, "broken");
    assert_eq!(manifest.declared_files, ["service.py"]);
    assert_eq!(
        manifest.discrepancies,
        [
            json!({"kind": "declared_file_unavailable", "file": "../outside.md", "reason": "target_path_invalid"})
        ]
    );
    assert!(lessons::append_audit_candidate(fixture.root(), &fixture.spec(), &report).unwrap());
    let lessons =
        std::fs::read_to_string(fixture.root().join(".mastermind/tasks/_lessons.md")).unwrap();
    assert!(
        lessons.contains("contract broken") && lessons.contains("declared file unavailable"),
        "{lessons}"
    );
}

#[test]
fn declared_files_preflight_reports_limits_and_store_interruption() {
    let fixture = Fixture::new();
    fixture.write_spec(&[DOC; 1025]);
    let capped = fixture.verify(false);
    assert_eq!(
        capped["errors"],
        json!([{"kind": "declared_file_unavailable", "file": null, "reason": "declaration_limit_exceeded"}])
    );
    fixture.write_spec(&[DOC]);
    let mut parsed = spec::parse_file(&fixture.spec()).unwrap();
    parsed.frontmatter.as_mut().unwrap().touches[0]
        .symbols
        .clear();
    assert!(fixture.store.push_work_budget(WorkBudget {
        deadline: None,
        op_ticks: Some(0)
    }));
    let report = verify_spec::run(&parsed, Some(&fixture.store), fixture.root());
    fixture.store.pop_work_budget();
    assert!(report.errors.iter().any(|finding| matches!(finding, verify_spec::Finding::DeclaredFileUnavailable { reason, .. } if reason == "interrupted")), "{report:?}");
    assert!(!verify_spec::run(&parsed, Some(&fixture.store), fixture.root()).has_failures());
}

#[cfg(unix)]
#[test]
fn declared_files_public_gates_reject_links_and_special_files_unix() {
    use std::os::unix::fs::symlink;
    let mut fixture = Fixture::new();
    fixture.write("service.py", b"def keep():\n    return 2\n");
    fixture.refresh();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("guide.md"), "# Outside\n").unwrap();
    symlink(
        outside.path().join("guide.md"),
        fixture.root().join(".mastermind/link.md"),
    )
    .unwrap();
    symlink(outside.path(), fixture.root().join(".mastermind/linked")).unwrap();
    assert!(Command::new("mkfifo")
        .arg(fixture.root().join(".mastermind/pipe"))
        .status()
        .unwrap()
        .success());
    for path in [
        ".mastermind/link.md",
        ".mastermind/linked/guide.md",
        ".mastermind/pipe",
    ] {
        fixture.write_spec(&[path]);
        assert_eq!(
            fixture.verify(false)["errors"][0]["kind"],
            "declared_file_unavailable"
        );
        file_error(&fixture.audit(false), Some(path), None);
        assert_eq!(fixture.controller(true), run_task::Outcome::PreFailed);
    }
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

// Observed verification runs only on unix.
#[cfg(unix)]
#[test]
fn a_larger_worktree_budget_lets_a_35_mib_prior_worktree_through_both_flights() {
    let mut fixture = Fixture::new();
    fixture.write_reviewed_spec();
    fixture.write_setup(&[
        ("worktree_max_total_bytes", 67_108_864),
        ("snapshot_max_total_bytes", 67_108_864),
    ]);
    fixture.write_blobs(7);
    assert_eq!(fixture.controller(true), run_task::Outcome::PreReady);
    let state = fixture.state_json();
    assert_eq!(state["initial_worktree"]["max_total_bytes"], 67_108_864);
    assert_eq!(state["snapshot_max_total_bytes"], 67_108_864);
    assert_eq!(
        fixture.active_approval()["snapshot_max_total_bytes"],
        67_108_864
    );
    fixture.remove_setup();
    fixture.change_service();
    fixture.run_unit_check();
    fixture.report_reviewed();
    assert_eq!(fixture.controller(false), run_task::Outcome::PostHeld);
    let resumed = fixture.plain_controller();
    assert!(resumed.status.success(), "{resumed:?}");
    assert!(
        String::from_utf8_lossy(&resumed.stdout).contains("Mechanical audit is held"),
        "{resumed:?}"
    );
    let state = fixture.state();
    assert_eq!(state.status, "history_review_required");
    assert_eq!(state.blocking_reason, None);
    let prepared = fixture
        .command()
        .args(["review-task", "prepare"])
        .arg(fixture.spec())
        .arg("--json")
        .output()
        .unwrap();
    assert!(prepared.status.success(), "{prepared:?}");
}

#[test]
fn a_strict_snapshot_over_32_mib_binds_under_the_pinned_budget_and_policy_accepts_it() {
    let mut fixture = Fixture::new();
    let names: Vec<String> = (0..7).map(|index| format!("big-{index}.bin")).collect();
    let created: Vec<&str> = names.iter().map(String::as_str).collect();
    fixture.write_creation_spec("strict", &[], &created, &[]);
    fixture.write_setup(&[("snapshot_max_total_bytes", 67_108_864)]);
    fixture.write(
        ".mastermind/policy.yml",
        b"rules:\n  - id: critical\n    critical: big-0.bin\n    require_workflow: strict\n",
    );
    assert_eq!(
        fixture.controller_with_strict(true, true),
        run_task::Outcome::PreReady
    );
    for (index, name) in names.iter().enumerate() {
        fixture.write(name, &vec![index as u8; 5 * MIB]);
    }
    fixture.refresh();
    fixture.report(&created);
    assert_eq!(fixture.controller(false), run_task::Outcome::PostHeld);
    assert!(fixture.state().held_snapshot_sha256.is_some());
    assert_eq!(
        fixture.policy(true)["runs"][0]["properties"]["passed"],
        true
    );
}

#[test]
fn editing_the_pinned_snapshot_budget_in_state_does_not_bypass_the_approval() {
    let mut fixture = Fixture::new();
    fixture.write_spec(&[]);
    fixture.write_setup(&[("snapshot_max_total_bytes", 67_108_864)]);
    assert_eq!(fixture.controller(true), run_task::Outcome::PreReady);
    let mut state = fixture.state_json();
    assert_eq!(state["snapshot_max_total_bytes"], 67_108_864);
    state["snapshot_max_total_bytes"] = json!(50_331_648);
    fixture.write(
        &SPEC.replace("spec.md", "state.json"),
        &serde_json::to_vec(&state).unwrap(),
    );
    fixture.change_service();
    fixture.report(&["service.py"]);
    let output = fixture.controller_from(fixture.root(), false);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        stderr_of(&output).contains("preflight_approval_binding_mismatch"),
        "{output:?}"
    );
}

#[test]
fn the_default_worktree_budget_names_itself_and_its_init_flag() {
    let fixture = Fixture::new();
    fixture.write_blobs(7);
    let output = fixture.controller_from(fixture.root(), true);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        stderr_of(&output).contains(
            "initial_worktree_byte_limit: worktree_max_total_bytes 33554432 is exhausted: \
             31457280 bytes counted, next file `data/blob-6.bin` has 5242880 bytes; raise it \
             with `mastermind init --worktree-max-total-bytes <bytes>` before the task's first \
             pre-flight"
        ),
        "{output:?}"
    );
    assert!(!fixture.spec().with_file_name("state.json").exists());
}

#[test]
fn a_snapshot_budget_is_pinned_by_pre_flight_and_a_change_waits_for_the_next_one() {
    let mut fixture = Fixture::new();
    fixture.write_spec(&[]);
    fixture.write_setup(&[("worktree_max_total_bytes", 67_108_864)]);
    fixture.write_blobs(7);
    assert_eq!(fixture.controller(true), run_task::Outcome::PreReady);
    let initial = fixture.state_json()["initial_worktree"].clone();
    fixture.change_service();
    fixture.report(&["service.py"]);
    let failed = fixture.controller_from(fixture.root(), false);
    assert!(!failed.status.success(), "{failed:?}");
    let message = stderr_of(&failed);
    for expected in [
        "history snapshot `data/blob-6.bin`: snapshot_max_total_bytes 33554432 is exhausted: ",
        " bytes counted, this file has 5242880 bytes; raise it with ",
        "`mastermind init --snapshot-max-total-bytes <bytes>` and repeat ",
        "`mastermind run-task <spec> --pre-only`",
    ] {
        assert!(message.contains(expected), "{expected}: {message}");
    }
    fixture.write_setup(&[
        ("worktree_max_total_bytes", 33_554_432),
        ("snapshot_max_total_bytes", 67_108_864),
    ]);
    let still = fixture.controller_from(fixture.root(), false);
    assert!(
        stderr_of(&still).contains("snapshot_max_total_bytes 33554432"),
        "{still:?}"
    );
    assert_eq!(fixture.controller(true), run_task::Outcome::PreReady);
    let state = fixture.state_json();
    assert_eq!(state["initial_worktree"], initial);
    assert_eq!(state["snapshot_max_total_bytes"], 67_108_864);
    assert_eq!(
        fixture.active_approval()["snapshot_max_total_bytes"],
        67_108_864
    );
    assert_eq!(fixture.controller(false), run_task::Outcome::PostHeld);
}

#[test]
fn a_larger_budget_keeps_the_per_file_limit_and_refuses_links() {
    let fixture = Fixture::new();
    fixture.write_setup(&[("worktree_max_total_bytes", 67_108_864)]);
    fixture.write("data/big.bin", &vec![1; 5 * MIB + 1]);
    let output = fixture.controller_from(fixture.root(), true);
    assert!(
        stderr_of(&output).contains(
            "initial_worktree_file_unavailable: data/big.bin: file has 5242881 bytes, limit is 5242880"
        ),
        "{output:?}"
    );
    #[cfg(unix)]
    {
        std::fs::remove_file(fixture.root().join("data/big.bin")).unwrap();
        std::os::unix::fs::symlink("service.py", fixture.root().join("linked.py")).unwrap();
        let output = fixture.controller_from(fixture.root(), true);
        assert!(
            stderr_of(&output).contains(
                "initial_worktree_file_unavailable: linked.py: path is not a regular no-follow file"
            ),
            "{output:?}"
        );
    }
}
