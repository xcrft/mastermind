use mmcg::{audit_spec, indexer::Indexer, run_task, spec, store::Store, verify_spec};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BASELINE: &str = "class A:\n    def run(self):\n        return 1\n\nclass B:\n    def run(self):\n        return 2\n\ndef caller(service):\n    return service.run()\n";

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
        std::fs::write(root.join("service.py"), BASELINE).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "baseline"]);
        git(root, &["tag", "baseline"]);
        let mut store = Store::open(root.join("graph.db")).unwrap();
        Indexer::new(root).index_all(&mut store, false).unwrap();
        Self { store, directory }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn source(&mut self, source: &str) {
        std::fs::write(self.root().join("service.py"), source).unwrap();
        Indexer::new(self.root())
            .index_all(&mut self.store, false)
            .unwrap();
    }

    fn write_spec(&self, name: &str, yaml_snapshot: bool, callers: Option<u32>) -> PathBuf {
        let path = self.root().join(".mastermind/tasks/001-scoped/spec.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let count = callers
            .map(|n| format!("        callers: {n}\n"))
            .unwrap_or_default();
        let (symbols, snapshot) = if yaml_snapshot {
            (
                format!("      - name: {name}\n        signature: 'def run(self)'\n{count}"),
                String::new(),
            )
        } else {
            let count = callers
                .map(|n| format!("{n} callers, "))
                .unwrap_or_default();
            (
                format!("      - {name}\n"),
                format!("\n## Pre-edit symbol snapshot\n- `{name}` — {count}signature `def run(self)`\n"),
            )
        };
        std::fs::write(&path, format!("---\nmode: lite\ntouches:\n  - file: service.py\n    language: python\n    symbols:\n{symbols}---\n# Scoped snapshot\n## Goals\nInspect the service method.\n{snapshot}")).unwrap();
        path
    }

    fn command(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mmcg"))
            .current_dir(self.root())
            .args(["--index", "graph.db"])
            .args(args)
            .env("MMCG_QUERY_BUDGET_MS", "60000")
            .output()
            .unwrap()
    }

    fn gate(&self, command: &str, path: &Path, success: bool) -> Value {
        let path = path.to_str().unwrap();
        let args = if command == "audit-spec" {
            vec![command, path, "--since", "baseline", "--json"]
        } else {
            vec![command, path, "--require-index", "--json"]
        };
        let output = self.command(&args);
        assert_eq!(output.status.success(), success, "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn cli_markdown_and_yaml_snapshots_detect_scoped_signature_drift() {
    for yaml in [false, true] {
        let mut fixture = Fixture::new();
        let path = fixture.write_spec("B.run", yaml, None);
        assert_eq!(fixture.gate("verify-spec", &path, true)["verdict"], "pass");
        fixture.source(&BASELINE.replace(
            "class B:\n    def run(self):",
            "class B:\n    def run(self, flag=False):",
        ));
        let verify = fixture.gate("verify-spec", &path, false);
        assert!(verify["errors"].as_array().unwrap().iter().any(|finding| {
            finding["kind"] == "snapshot_signature_drift"
                && finding["symbol"] == "B.run"
                && finding["index_says"] == "def run(self, flag=False)"
        }));
        let audit = fixture.gate("audit-spec", &path, true);
        assert_eq!(audit["verdict"], "drift");
        assert_eq!(audit["findings"].as_array().unwrap().len(), 1);
        assert_eq!(audit["findings"][0]["kind"], "snapshot_signature_drift");
        assert_eq!(audit["findings"][0]["symbol"], "B.run");
        assert_eq!(audit["symbol_diff"]["signature_changed"][0]["new_line"], 6);
    }
}

#[test]
fn ambiguous_snapshots_fail_cli_ci_controller_and_bundle() {
    let mut fixture = Fixture::new();
    let path = fixture.write_spec("run", false, None);
    fixture.source(&BASELINE.replace("return 2", "return 3"));
    let verify = fixture.gate("verify-spec", &path, false);
    assert!(verify["errors"].as_array().unwrap().iter().any(|finding| {
        finding["kind"] == "snapshot_unresolved"
            && finding["reason"] == "ambiguous"
            && finding["matches"] == 2
    }));
    let audit = fixture.gate("audit-spec", &path, false);
    assert_eq!(audit["verdict"], "broken");
    assert_eq!(audit["findings"][0]["kind"], "snapshot_unresolved");
    let ci = fixture.command(&["ci", "--since", "baseline"]);
    assert!(!ci.status.success(), "{ci:?}");
    assert!(String::from_utf8_lossy(&ci.stderr).contains("snapshot_unresolved"));
    let outcome = run_task::run(
        &path,
        fixture.root(),
        &fixture.root().join("graph.db"),
        run_task::RunOpts {
            pre_only: true,
            ..Default::default()
        },
    );
    assert_eq!(outcome, run_task::Outcome::PreFailed);
    assert!(!run_task::state_file_path(fixture.root(), &path).exists());
    let parsed = spec::parse_file(&path).unwrap();
    let report = audit_spec::run(&parsed, &fixture.store, fixture.root(), "baseline").unwrap();
    let bundle = audit_spec::Bundle::from_report(&report, None);
    assert_eq!(bundle.snapshot_drift.len(), 1);
    assert!(
        bundle.human_summary.contains("1 errors, 0 warnings"),
        "{}",
        bundle.human_summary
    );
    assert!(report.render_text().contains("snapshot_unresolved"));
}

#[test]
fn deleted_scoped_snapshots_cannot_reuse_a_surviving_same_name_method() {
    for yaml in [false, true] {
        let mut fixture = Fixture::new();
        let path = fixture.write_spec("B.run", yaml, None);
        fixture.source("class A:\n    def run(self):\n        return 1\n");
        let verify = fixture.gate("verify-spec", &path, false);
        assert!(verify["errors"].as_array().unwrap().iter().any(|finding| {
            finding["kind"] == "missing_symbol_at_file" && finding["symbol"] == "B.run"
        }));
        let audit = fixture.gate("audit-spec", &path, false);
        assert_eq!(audit["verdict"], "broken");
        assert!(audit["findings"].as_array().unwrap().iter().any(|finding| {
            finding["kind"] == "snapshot_symbol_gone" && finding["symbol"] == "B.run"
        }));
    }
}

#[test]
fn scoped_snapshot_risk_keeps_leaf_callers_and_explicit_language() {
    let mut fixture = Fixture::new();
    std::fs::write(
        fixture.root().join("other.py"),
        "class B:\n    def run(self):\n        pass\n",
    )
    .unwrap();
    std::fs::write(
        fixture.root().join("other.rs"),
        "fn run() {}\nfn rust_caller() { run(); }\n",
    )
    .unwrap();
    let root = fixture.root().to_path_buf();
    Indexer::new(&root)
        .index_all(&mut fixture.store, false)
        .unwrap();
    let path = fixture.write_spec("B.run", false, Some(1));
    let parsed = spec::parse_file(&path).unwrap();
    let verify = verify_spec::run(&parsed, Some(&fixture.store), fixture.root());
    assert_eq!(verify.verdict, verify_spec::Verdict::Pass, "{verify:?}");
    let risk = run_task::compute_risk_report(&parsed, &fixture.store).unwrap();
    assert_eq!(risk.total_snapshot_callers, 1);
    assert_eq!(risk.worst_callers.as_ref().unwrap().name, "B.run");
    assert_eq!(risk.worst_callers.as_ref().unwrap().callers, 1);
}

#[test]
fn controller_keeps_scoped_snapshot_drift_and_ambiguity_pending_review() {
    for (source, outcome, finding) in [
        (
            BASELINE.replace(
                "class B:\n    def run(self):",
                "class B:\n    def run(self, flag=False):",
            ),
            run_task::Outcome::PostDrift,
            "snapshot_signature_drift",
        ),
        (
            format!("{BASELINE}\nclass B:\n    def run(self):\n        return 3\n"),
            run_task::Outcome::PostBroken,
            "snapshot_unresolved",
        ),
    ] {
        let mut fixture = Fixture::new();
        let path = fixture.write_spec("B.run", false, None);
        let index = fixture.root().join("graph.db");
        assert_eq!(
            run_task::run(
                &path,
                fixture.root(),
                &index,
                run_task::RunOpts {
                    pre_only: true,
                    ..Default::default()
                }
            ),
            run_task::Outcome::PreReady,
        );
        fixture.source(&source);
        std::fs::write(
            path.with_file_name("executor-report.md"),
            "schema_version: 1\nspec: .mastermind/tasks/001-scoped/spec.md\nstatus: complete\nphases:\n  - id: '1'\n    status: done\nfiles_modified: [service.py]\nclaims: []\ndefects: []\nverifications: []\n",
        ).unwrap();
        assert_eq!(
            run_task::run(
                &path,
                fixture.root(),
                &index,
                run_task::RunOpts {
                    post_only: true,
                    ..Default::default()
                }
            ),
            outcome,
        );
        let audit = std::fs::read_to_string(path.with_file_name("audit.md")).unwrap();
        assert!(audit.contains(finding), "{audit}");
        let state = run_task::load_state(&run_task::state_file_path(fixture.root(), &path))
            .unwrap()
            .unwrap();
        assert_eq!(state.next_step.as_deref(), Some("planner_review"));
        assert!(state.held_snapshot_sha256.is_none());
        assert!(!run_task::release_file_path(fixture.root(), &path).exists());
    }
}

#[test]
fn snapshot_caller_query_failure_is_not_a_verified_zero() {
    let mut fixture = Fixture::new();
    let path = fixture.write_spec("B.run", false, Some(1));
    fixture.source(&BASELINE.replace("return 2", "return 3"));
    let raw = rusqlite::Connection::open(fixture.root().join("graph.db")).unwrap();
    raw.execute_batch("DROP TABLE edges").unwrap();
    let parsed = spec::parse_file(&path).unwrap();
    let verify = verify_spec::run(&parsed, Some(&fixture.store), fixture.root());
    assert_eq!(verify.verdict, verify_spec::Verdict::Fail);
    assert!(verify.errors.iter().any(|finding| matches!(
        finding, verify_spec::Finding::SnapshotUnresolved { reason, matches: None, .. }
            if reason == "caller_query_failed"
    )));
    let audit = audit_spec::run(&parsed, &fixture.store, fixture.root(), "baseline").unwrap();
    assert_eq!(audit.verdict, audit_spec::Verdict::Broken);
    assert!(audit.findings.iter().any(|finding| matches!(
        finding, audit_spec::Finding::SnapshotUnresolved { reason, matches: None, .. }
            if reason == "caller_query_failed"
    )));
    assert!(run_task::compute_risk_report(&parsed, &fixture.store).is_err());
}
