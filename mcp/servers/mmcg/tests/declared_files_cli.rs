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
    fn state(&self) -> run_task::RunState {
        run_task::load_state(&run_task::state_file_path(self.root(), &self.spec()))
            .unwrap()
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
            "claims": [], "defects": [], "verifications": [{"cmd": VERIFY, "result": "pass"}],
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
    run_task::save_state(
        &run_task::state_file_path(fixture.root(), &fixture.spec()),
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
