use mmcg::{
    indexer::Indexer,
    run_task, spec,
    store::{Store, WorkBudget},
    verify_spec,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPEC: &str = ".mastermind/tasks/001-find/spec.md";
const INPUT: &str = ".mastermind/find/input.txt";
const VERIFY: &str = "git --version > .mastermind/should-not-run";

fn source(value: u8) -> String {
    format!("def keep():\n    return {value}\n")
}

fn spec_text(file: Option<&str>, find: &str) -> String {
    let metadata = json!({
        "mode": "lite", "touches": [{"file": "service.py", "symbols": ["keep"]}],
        "verify": [{"cmd": VERIFY}],
    });
    let marker = file
        .map(|file| format!("**File:** `{file}`\n"))
        .unwrap_or_default();
    format!("---\n{}---\n# Replace service text\n## Goals\nApply the literal replacement.\n## Phase 1: replace\n{marker}FIND:\n```text\n{find}\n```\nCHANGE TO:\n```text\nreturn 2\n```\n", serde_norway::to_string(&metadata).unwrap())
}

struct Fixture {
    store: Store,
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new(initial: u8) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for args in [
            vec!["init", "-q", "--initial-branch=main"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            git(root, &args);
        }
        std::fs::write(root.join(".gitignore"), "graph.db*\n.mastermind/\n").unwrap();
        std::fs::write(root.join("service.py"), source(initial)).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "baseline"]);
        git(root, &["tag", "baseline"]);
        let mut store = Store::open(root.join("graph.db")).unwrap();
        Indexer::new(root).index_all(&mut store, false).unwrap();
        let fixture = Self { store, directory };
        fixture.write_spec(Some(INPUT), "needle");
        fixture.write(INPUT, b"prefix needle suffix");
        fixture
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }
    fn spec(&self) -> PathBuf {
        self.root().join(SPEC)
    }

    fn write(&self, path: &str, contents: &[u8]) {
        let path = self.root().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn write_spec(&self, file: Option<&str>, find: &str) {
        self.write(SPEC, spec_text(file, find).as_bytes());
    }

    fn change(&mut self, value: u8) {
        self.write("service.py", source(value).as_bytes());
        Indexer::new(self.root())
            .index_all(&mut self.store, false)
            .unwrap();
    }

    fn write_report(&self) {
        let report = json!({
            "schema_version": 1, "spec": SPEC, "status": "complete",
            "phases": [{"id": "1", "status": "done"}], "files_modified": ["service.py"],
            "claims": [], "defects": [], "verifications": [{"cmd": VERIFY, "result": "pass"}],
        });
        std::fs::write(
            self.spec().with_file_name("executor-report.md"),
            report.to_string(),
        )
        .unwrap();
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

    fn verify_from(&self, cwd: &Path, json: bool) -> Output {
        let mut command = self.command();
        command
            .current_dir(cwd)
            .arg("verify-spec")
            .arg(self.spec())
            .arg("--root")
            .arg(self.root())
            .arg("--require-index");
        if json {
            command.arg("--json");
        }
        command.output().unwrap()
    }

    fn verify(&self, success: bool) -> Value {
        let output = self.verify_from(self.root(), true);
        assert_eq!(output.status.success(), success, "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
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
        run_task::run(
            &self.spec(),
            self.root(),
            &self.root().join("graph.db"),
            run_task::RunOpts {
                pre_only,
                post_only: !pre_only,
                ..Default::default()
            },
        )
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

fn unavailable(report: &Value, file: Option<&str>, reason: &str) {
    assert_eq!(report["verdict"], "fail");
    assert_eq!(
        report["errors"],
        json!([{
            "kind": "find_block_unavailable", "file": file,
            "phase": "Phase 1: replace", "reason": reason,
        }]),
        "{report}"
    );
}

#[test]
fn find_preflight_cli_distinguishes_mismatch_from_unavailable() {
    let fixture = Fixture::new(1);
    assert_eq!(fixture.verify(true)["verdict"], "pass");
    fixture.write(INPUT, b"different");
    let mismatch = fixture.verify(false);
    assert_eq!(
        mismatch["errors"],
        json!([{
            "kind": "find_block_mismatch", "file": INPUT,
            "phase": "Phase 1: replace", "find_text_preview": "needle",
        }])
    );
    fixture.write(INPUT, b"needle\xff");
    unavailable(&fixture.verify(false), Some(INPUT), "target_not_utf8");
    let text = fixture.verify_from(fixture.root(), false);
    assert!(!text.status.success());
    assert!(String::from_utf8_lossy(&text.stdout).contains("find_block_unavailable"));
    assert!(String::from_utf8_lossy(&text.stdout).contains("target_not_utf8"));
    std::fs::remove_file(fixture.root().join(INPUT)).unwrap();
    unavailable(&fixture.verify(false), Some(INPUT), "target_missing");
    std::fs::create_dir(fixture.root().join(INPUT)).unwrap();
    let directory = fixture.verify(false);
    assert_eq!(directory["errors"].as_array().unwrap().len(), 1);
    assert_eq!(directory["errors"][0]["kind"], "find_block_unavailable");
    fixture.write_spec(None, "needle");
    unavailable(&fixture.verify(false), None, "target_unspecified");
}

#[test]
fn find_preflight_cli_confines_targets_and_ignores_cwd_shadows() {
    let fixture = Fixture::new(1);
    fixture.write(&format!(".mastermind/nested/{INPUT}"), b"different");
    let nested = fixture.root().join(".mastermind/nested");
    let output = fixture.verify_from(&nested, true);
    assert!(output.status.success(), "{output:?}");
    fixture.write(INPUT, b"different");
    fixture.write(&format!(".mastermind/nested/{INPUT}"), b"needle");
    let output = fixture.verify_from(&nested, true);
    assert!(!output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["errors"][0]["kind"], "find_block_mismatch");
    let outside = tempfile::tempdir_in(fixture.root().parent().unwrap()).unwrap();
    std::fs::write(outside.path().join("input.txt"), b"needle").unwrap();
    let absolute = outside
        .path()
        .join("input.txt")
        .to_str()
        .unwrap()
        .to_string();
    let parent = format!(
        "../{}/input.txt",
        outside.path().file_name().unwrap().to_str().unwrap()
    );
    for path in [
        absolute.as_str(),
        parent.as_str(),
        "/input.txt",
        "C:/input.txt",
        "C:input.txt",
    ] {
        fixture.write_spec(Some(path), "needle");
        unavailable(&fixture.verify(false), Some(path), "target_path_invalid");
    }
    fixture.write_spec(Some("./.mastermind\\find\\input.txt"), "needle");
    fixture.write(INPUT, b"needle");
    assert_eq!(fixture.verify(true)["verdict"], "pass");
}

#[test]
fn find_preflight_cli_rejects_partial_reads_and_shared_budget_exhaustion() {
    let fixture = Fixture::new(1);
    std::fs::OpenOptions::new()
        .write(true)
        .open(fixture.root().join(INPUT))
        .unwrap()
        .set_len(mmcg::indexer::MAX_INDEXABLE_FILE_SIZE + 1)
        .unwrap();
    unavailable(&fixture.verify(false), Some(INPUT), "target_too_large");
    let base = std::fs::read_to_string(fixture.spec()).unwrap();
    let phase = base[base.find("## Phase 1: replace").unwrap()..].to_string();
    fixture.write(SPEC, format!("{base}{}", phase.repeat(7)).as_bytes());
    let repeated = fixture.verify(false);
    let findings = repeated["errors"].as_array().unwrap();
    assert_eq!(findings.len(), 8, "{repeated}");
    assert!(findings
        .iter()
        .all(|finding| finding["kind"] == "find_block_unavailable"));
    assert!(findings
        .iter()
        .any(|finding| finding["reason"] == "read_budget_exhausted"));
    fixture.write(SPEC, format!("{base}{}", phase.repeat(1024)).as_bytes());
    let capped = fixture.verify(false);
    assert_eq!(
        capped["errors"],
        json!([{
            "kind": "find_block_unavailable", "file": null, "phase": null,
            "reason": "block_limit_exceeded",
        }])
    );
}

#[test]
fn find_preflight_public_verifier_observes_store_interruption() {
    let fixture = Fixture::new(1);
    let mut parsed = spec::parse_str(SPEC, &spec_text(Some(INPUT), "needle"));
    parsed.frontmatter.as_mut().unwrap().touches[0]
        .symbols
        .clear();
    assert!(fixture.store.push_work_budget(WorkBudget {
        deadline: None,
        op_ticks: Some(0)
    }));
    let report = verify_spec::run(&parsed, Some(&fixture.store), fixture.root());
    fixture.store.pop_work_budget();
    assert!(report.has_failures());
    assert!(
        report.errors.iter().any(|finding| matches!(finding,
            verify_spec::Finding::FindBlockUnavailable { reason, .. } if reason == "interrupted"
        )),
        "{report:?}"
    );
    assert!(!verify_spec::run(&parsed, Some(&fixture.store), fixture.root()).has_failures());
}

#[test]
fn find_preflight_controller_revokes_approval_and_recovers() {
    let fixture = Fixture::new(1);
    let state_path = run_task::state_file_path(fixture.root(), &fixture.spec());
    fixture.write(INPUT, &[0xff]);
    assert_eq!(fixture.controller(true), run_task::Outcome::PreFailed);
    assert!(run_task::load_state(&state_path).unwrap().is_none());
    fixture.write(INPUT, b"needle");
    assert_eq!(fixture.controller(true), run_task::Outcome::PreReady);
    let mut approved = run_task::load_state(&state_path).unwrap().unwrap();
    approved.held_snapshot_sha256 = Some("old-held".into());
    approved.history_snapshot_sha256 = Some("old-history".into());
    run_task::save_state_in_repository(fixture.root(), &state_path, &approved).unwrap();
    fixture.write(INPUT, &[0xff]);
    assert_eq!(fixture.controller(true), run_task::Outcome::PreFailed);
    let failed = run_task::load_state(&state_path).unwrap().unwrap();
    assert_eq!(failed.baseline_ref, approved.baseline_ref);
    assert_eq!(failed.next_step.as_deref(), Some("run_preflight"));
    assert!(failed.held_snapshot_sha256.is_none() && failed.history_snapshot_sha256.is_none());
    fixture.write(INPUT, b"needle");
    assert_eq!(fixture.controller(true), run_task::Outcome::PreReady);
    let recovered = run_task::load_state(&state_path).unwrap().unwrap();
    assert_eq!(recovered.baseline_ref, approved.baseline_ref);
    assert!(!fixture.root().join(".mastermind/should-not-run").exists());
}

#[test]
fn find_postflight_accepts_replacement_independently_of_git_baseline() {
    for initial in [0, 1] {
        let mut fixture = Fixture::new(initial);
        fixture.write_spec(Some("service.py"), "return 1");
        fixture.change(1);
        let baseline = git(fixture.root(), &["rev-parse", "baseline"]);
        assert_eq!(fixture.controller(true), run_task::Outcome::PreReady);
        let state_path = run_task::state_file_path(fixture.root(), &fixture.spec());
        assert_eq!(
            run_task::load_state(&state_path)
                .unwrap()
                .unwrap()
                .baseline_ref,
            baseline
        );
        fixture.change(2);
        fixture.write_report();
        assert_eq!(fixture.controller(false), run_task::Outcome::PostHeld);
        let stale = fixture.verify(false);
        assert_eq!(stale["errors"][0]["kind"], "find_block_mismatch");
        git(fixture.root(), &["add", "service.py"]);
        git(fixture.root(), &["commit", "-q", "-m", "replacement"]);
        let ci = fixture.ci();
        assert!(ci.status.success(), "initial={initial}: {ci:?}");
        let bundle: Value = serde_json::from_slice(
            &std::fs::read(
                fixture
                    .root()
                    .join(".mastermind/bundles/001-find.bundle.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(bundle["schema_version"], 3);
        assert_eq!(bundle["manifest"]["verdict"], "held");
        assert_eq!(bundle["manifest"]["discrepancies"], json!([]));
        assert_eq!(
            run_task::load_state(&state_path)
                .unwrap()
                .unwrap()
                .baseline_ref,
            baseline
        );
        assert!(!fixture.root().join(".mastermind/should-not-run").exists());
    }
}

#[test]
fn find_postflight_preserves_file_section_and_verification_gates() {
    let mut fixture = Fixture::new(1);
    fixture.write_spec(Some("service.py"), "return 1");
    fixture.change(2);
    git(fixture.root(), &["add", "service.py"]);
    git(fixture.root(), &["commit", "-q", "-m", "replacement"]);
    fixture.write_report();
    assert!(fixture.ci().status.success());
    let original = std::fs::read_to_string(fixture.spec()).unwrap();
    for (modified, message) in [
        (
            original.replace("touches:", "expected_docs: [missing.md]\ntouches:"),
            "missing_file",
        ),
        (
            original.replace("Apply the literal replacement.", ""),
            "empty_mandatory_section",
        ),
    ] {
        fixture.write(SPEC, modified.as_bytes());
        let ci = fixture.ci();
        assert!(!ci.status.success(), "{ci:?}");
        let text = String::from_utf8_lossy(&ci.stderr);
        assert!(text.contains(message), "{ci:?}");
        assert!(!text.contains("find_block_mismatch"), "{ci:?}");
    }
    fixture.write(
        SPEC,
        format!("{original}VERIFY: git diff --check\n").as_bytes(),
    );
    let ci = fixture.ci();
    assert!(!ci.status.success(), "{ci:?}");
    assert!(
        String::from_utf8_lossy(&ci.stderr).contains("verification_requirement_unmet"),
        "{ci:?}"
    );
}

#[cfg(unix)]
#[test]
fn find_preflight_cli_rejects_links_and_special_files_unix() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new(1);
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("input.txt"), b"needle").unwrap();
    symlink(
        outside.path().join("input.txt"),
        fixture.root().join(".mastermind/link.txt"),
    )
    .unwrap();
    symlink(outside.path(), fixture.root().join(".mastermind/linked")).unwrap();
    let status = Command::new("mkfifo")
        .arg(fixture.root().join(".mastermind/pipe"))
        .status()
        .unwrap();
    assert!(status.success());
    for path in [
        ".mastermind/link.txt",
        ".mastermind/linked/input.txt",
        ".mastermind/pipe",
    ] {
        fixture.write_spec(Some(path), "needle");
        let report = fixture.verify(false);
        assert_eq!(report["errors"].as_array().unwrap().len(), 1, "{report}");
        assert_eq!(report["errors"][0]["kind"], "find_block_unavailable");
    }
}
