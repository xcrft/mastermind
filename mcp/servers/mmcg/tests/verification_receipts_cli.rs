//! Public CLI evidence for observed verification, including real child processes.

#[cfg(unix)]
mod unix {
    use mmcg::{indexer::Indexer, store::Store};
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Output, Stdio};
    use std::time::{Duration, Instant};

    const SPEC: &str = ".mastermind/tasks/001-receipts/spec.md";
    const REPORT: &str = ".mastermind/tasks/001-receipts/executor-report.md";
    const RECEIPT: &str = ".mastermind/tasks/001-receipts/verification/unit.json";
    const STATE: &str = ".mastermind/tasks/001-receipts/state.json";
    const HISTORY: &str = ".mastermind/tasks/001-receipts/history-review.md";
    const PROBE: &str = r##"#!/bin/sh
case "$1" in
  success) printf 'proof\n'; printf run > .mastermind/ran ;;
  fail) printf 'failed\n' >&2; exit 7 ;;
  switch) if test -f .mastermind/block; then sleep 30 & printf '%s' "$!" > .mastermind/child; printf '%s' "$$" > .mastermind/started; wait; fi; if test -f .mastermind/fail; then exit 7; fi; printf 'proof\n' ;;
  args) shift; printf '%s\n' "$@" ;;
  sleep) sleep 30 & child=$!; printf '%s' "$child" > .mastermind/child; printf '%s' "$$" > .mastermind/started; wait ;;
  background) sleep 30 > /dev/null 2>&1 & printf '%s' "$!" > .mastermind/child; exit 0 ;;
  output) exec /usr/bin/yes output ;;
  change) printf 'def keep():\n    return 9\n' > service.py ;;
  *) exit 8 ;;
esac
"##;

    struct Fixture {
        directory: tempfile::TempDir,
        command: String,
    }

    impl Fixture {
        fn new(mode: &str, timeout: u64) -> Self {
            Self::with_argv(
                vec!["./.mastermind/probe.sh".into(), mode.into()],
                ".",
                timeout,
            )
        }

        fn with_argv(argv: Vec<String>, cwd: &str, timeout: u64) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            fs::create_dir_all(root.join(".mastermind/home")).unwrap();
            fs::create_dir_all(root.join(SPEC).parent().unwrap()).unwrap();
            fs::create_dir(root.join("sub")).unwrap();
            fs::write(root.join("sub/README.md"), "dependency\n").unwrap();
            fs::write(root.join(".gitignore"), ".mastermind/\n").unwrap();
            fs::write(root.join("service.py"), "def keep():\n    return 1\n").unwrap();
            for args in [
                vec!["init", "-q", "--initial-branch=main"],
                vec!["config", "user.name", "Receipt Test"],
                vec!["config", "user.email", "receipt@example.invalid"],
                vec!["config", "commit.gpgsign", "false"],
                vec!["add", "."],
                vec!["commit", "-q", "-m", "baseline"],
                vec!["tag", "baseline"],
            ] {
                let output = Self::git_command(root).args(args).output().unwrap();
                assert!(output.status.success(), "{output:?}");
            }
            fs::write(root.join(".mastermind/probe.sh"), PROBE).unwrap();
            fs::set_permissions(
                root.join(".mastermind/probe.sh"),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            let command = mmcg::verification_receipts::display_command(&argv);
            let metadata = json!({
                "mode": "verified", "touches": [{"file": "service.py", "symbols": ["keep"]}],
                "verify": [{"cmd": command, "run": {"id": "unit", "argv": argv, "cwd": cwd, "timeout_secs": timeout}}],
            });
            fs::write(root.join(SPEC), format!(
                "---\n{}---\n# Receipt fixture\n## Goals\nUpdate the service.\n## Scope\nEdit service.py.\n## Acceptance Criteria\nThe service returns two.\n## Tests Plan\nRun the declared check.\n## Final Verification\nRun the declared command.\n",
                serde_norway::to_string(&metadata).unwrap()
            )).unwrap();
            let fixture = Self { directory, command };
            fixture.refresh();
            fixture
        }

        fn root(&self) -> &Path {
            self.directory.path()
        }
        fn path(&self, path: &str) -> PathBuf {
            self.root().join(path)
        }
        fn git_command(root: &Path) -> Command {
            let mut command = Command::new("/usr/bin/git");
            command
                .current_dir(root)
                .env_clear()
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("HOME", root.join(".mastermind/home"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null");
            command
        }
        fn cli(&self, args: &[&str]) -> Command {
            let mut command = Command::new(env!("CARGO_BIN_EXE_mmcg"));
            command
                .current_dir(self.root())
                .env_clear()
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("HOME", self.path(".mastermind/home"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("MMCG_GIT_TIMEOUT_MS", "20000")
                .args(["--index", ".mastermind/index.db"])
                .args(args);
            command
        }
        fn run(&self, args: &[&str]) -> Output {
            self.cli(args).output().unwrap()
        }
        fn refresh(&self) {
            let mut store = Store::open(self.path(".mastermind/index.db")).unwrap();
            let stats = Indexer::new(self.root())
                .index_all(&mut store, false)
                .unwrap();
            assert_eq!(stats.files_failed, 0);
        }
        fn prepare(&self) {
            let pre = self.run(&["run-task", SPEC, "--pre-only"]);
            assert!(pre.status.success(), "{pre:?}");
            fs::write(self.path("service.py"), "def keep():\n    return 2\n").unwrap();
            self.refresh();
        }
        fn verify(&self) -> (Output, Value) {
            let output = self.run(&["verification", "run", SPEC, "--id", "unit", "--json"]);
            let receipt = serde_json::from_slice(&output.stdout)
                .unwrap_or_else(|error| panic!("{error}: {output:?}"));
            (output, receipt)
        }
        fn write_report(&self) {
            fs::write(self.path(REPORT), json!({
                "schema_version": 1, "spec": SPEC, "status": "complete", "phases": [],
                "files_modified": ["service.py"], "claims": [], "defects": [],
                "verifications": [{"cmd": self.command, "result": "pass", "observed": {"exit_code": 0}}],
            }).to_string()).unwrap();
        }
        fn post(&self) -> Output {
            self.run(&["run-task", SPEC, "--post-only"])
        }
        fn state(&self) -> Value {
            serde_json::from_slice(&fs::read(self.path(STATE)).unwrap()).unwrap()
        }
        fn resolve_history_review(&self) {
            let review = fs::read_to_string(self.path(HISTORY)).unwrap();
            fs::write(
                self.path(HISTORY),
                review
                    .replace("**Context:** pending", "**Context:** not applicable")
                    .replace("**Lesson:** pending", "**Lesson:** not applicable")
                    .replace(
                        "**Reason:** semantic review required",
                        "**Reason:** this fixture changes no durable project knowledge",
                    ),
            )
            .unwrap();
        }
        fn audit(&self) -> (Output, Value) {
            self.refresh();
            let output = self.run(&[
                "audit-spec",
                SPEC,
                "--since",
                "baseline",
                "--executor-report",
                REPORT,
                "--json",
            ]);
            let report: Value = serde_json::from_slice(&output.stdout)
                .unwrap_or_else(|error| panic!("{error}: {output:?}"));
            (output, report)
        }
        fn spawn_verification(&self) -> Child {
            self.cli(&["verification", "run", SPEC, "--id", "unit", "--json"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap()
        }
        fn await_started(&self) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !self.path(".mastermind/started").exists() {
                assert!(
                    Instant::now() < deadline,
                    "runner did not reach the fixture process"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    fn assert_receipt_broken(fixture: &Fixture) {
        let (output, report) = fixture.audit();
        assert!(!output.status.success(), "{output:?}");
        assert_eq!(report["verdict"], "broken", "{report}");
        assert!(
            report["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(
                    |finding| finding["kind"] == "verification_requirement_unmet"
                        && finding["cmd"] == "observed verification receipts"
                ),
            "{report}"
        );
    }

    fn no_running_process(pid: i32) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let output = Command::new("/bin/ps")
                .args(["-p", &pid.to_string(), "-o", "stat="])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&output.stdout);
            if !output.status.success() || state.trim().is_empty() || state.trim().starts_with('Z')
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "process {pid} remains running: {state}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn observed_success_requires_a_real_run_and_is_bound_into_history_review() {
        let fixture = Fixture::new("success", 10);
        fixture.prepare();
        assert!(
            !fixture.path(REPORT).exists(),
            "runner must not require or create the executor report"
        );
        let (output, receipt) = fixture.verify();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(receipt["status"], "passed");
        assert_eq!(receipt["exit_code"], 0);
        assert_eq!(receipt["provenance"], "local_runner_unsigned");
        assert_eq!(receipt["snapshot_before"], receipt["snapshot_after"]);
        assert!(fixture.path(".mastermind/ran").exists());
        assert_eq!(
            fs::metadata(fixture.path(RECEIPT))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fixture.write_report();
        let post = fixture.post();
        assert!(post.status.success(), "{post:?}");
        assert_eq!(fixture.state()["status"], "history_review_required");
        let (output, receipt) = fixture.verify();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(receipt["attempt"], 2);
        let resumed = fixture.run(&["run-task", SPEC]);
        assert!(
            !resumed.status.success(),
            "a changed receipt must require new audit: {resumed:?}"
        );
        assert_eq!(fixture.state()["status"], "audit_required");
    }

    #[test]
    fn missing_receipt_cannot_fall_back_to_report_and_audits_never_execute() {
        let fixture = Fixture::new("success", 10);
        fixture.prepare();
        fixture.write_report();
        assert_receipt_broken(&fixture);
        assert!(!fixture.post().status.success());
        assert!(!fixture.path(".mastermind/ran").exists());
        let output = fixture.run(&["verify-spec", SPEC]);
        assert!(output.status.success(), "{output:?}");
        assert!(!fixture.path(".mastermind/ran").exists());
    }

    fn assert_stale_receipt_cannot_finish_history(change: &str) {
        let executable_directory = tempfile::tempdir().unwrap();
        let executable = executable_directory.path().join("probe.sh");
        fs::write(&executable, PROBE).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let fixture = Fixture::with_argv(
            vec![executable.to_str().unwrap().into(), "success".into()],
            ".",
            10,
        );
        fixture.prepare();
        assert!(fixture.verify().0.status.success());
        fixture.write_report();
        let post = fixture.post();
        assert!(post.status.success(), "{post:?}");
        assert_eq!(fixture.state()["status"], "history_review_required");
        let receipt = fs::read(fixture.path(RECEIPT)).unwrap();
        match change {
            "executable" => fs::write(&executable, format!("{PROBE}\n# changed tool\n")).unwrap(),
            "hidden_dependency" => {
                let output = Fixture::git_command(fixture.root())
                    .args(["update-index", "--assume-unchanged", "sub/README.md"])
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{output:?}");
                fs::write(fixture.path("sub/README.md"), "changed hidden dependency\n").unwrap();
            }
            _ => unreachable!(),
        }
        fixture.resolve_history_review();
        let resumed = fixture.run(&["run-task", SPEC]);
        assert!(!resumed.status.success(), "{change}: {resumed:?}");
        assert_eq!(fixture.state()["status"], "audit_required", "{change}");
        assert_eq!(fs::read(fixture.path(RECEIPT)).unwrap(), receipt);
    }

    #[test]
    fn changed_external_executable_cannot_close_pending_history_review() {
        assert_stale_receipt_cannot_finish_history("executable");
    }

    #[test]
    fn changed_assume_unchanged_dependency_cannot_close_pending_history_review() {
        assert_stale_receipt_cannot_finish_history("hidden_dependency");
    }

    #[test]
    fn late_external_executable_change_cannot_close_legacy_repeated_postflight() {
        for mutate in [false, true] {
            let harness = tempfile::tempdir().unwrap();
            let executable = harness.path().join("probe.sh");
            fs::write(&executable, PROBE).unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            let fixture = Fixture::with_argv(
                vec![executable.to_str().unwrap().into(), "success".into()],
                ".",
                10,
            );
            fixture.prepare();
            assert!(fixture.verify().0.status.success());
            fixture.write_report();
            let post = fixture.post();
            assert!(post.status.success(), "{post:?}");
            assert_eq!(fixture.state()["status"], "history_review_required");
            assert_eq!(fixture.state()["semantic_review_required"], false);
            fixture.resolve_history_review();
            let before = fixture.state();
            let receipt = fs::read(fixture.path(RECEIPT)).unwrap();
            let history = fs::read(fixture.path(HISTORY)).unwrap();
            let wrapper = harness.path().join("git");
            fs::write(
                &wrapper,
                r#"#!/bin/sh
set -eu
if test "$#" -eq 10 && test "$1" = -c &&
   test "$2" = core.fsmonitor=false && test "$3" = diff && test "$4" = --stat; then
  /bin/cp "$MMCG_LATE_POST_RECEIPT" "$MMCG_LATE_POST_HARNESS/receipt-at-release"
  /bin/cp "$MMCG_LATE_POST_HISTORY" "$MMCG_LATE_POST_HARNESS/history-at-release"
  printf 'release-stat\n' >> "$MMCG_LATE_POST_HARNESS/release-seen"
  if test "$MMCG_LATE_POST_MUTATE" = 1; then
    printf '\n# External tool changed after the audit.\n' >> "$MMCG_LATE_POST_EXECUTABLE"
    printf mutated > "$MMCG_LATE_POST_HARNESS/mutated"
  fi
fi
exec /usr/bin/git "$@"
"#,
            )
            .unwrap();
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
            // Release notes are computed after observed checks have been
            // audited. The wrapper changes only the external executable at
            // that seam, leaving the successful receipt and Markdown intact.
            let output = fixture
                .cli(&["run-task", SPEC, "--post-only"])
                .env(
                    "PATH",
                    format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", harness.path().display()),
                )
                .env("MMCG_LATE_POST_HARNESS", harness.path())
                .env("MMCG_LATE_POST_RECEIPT", fixture.path(RECEIPT))
                .env("MMCG_LATE_POST_HISTORY", fixture.path(HISTORY))
                .env("MMCG_LATE_POST_EXECUTABLE", &executable)
                .env("MMCG_LATE_POST_MUTATE", if mutate { "1" } else { "0" })
                .output()
                .unwrap();
            assert_eq!(
                fs::read_to_string(harness.path().join("release-seen")).unwrap(),
                "release-stat\n"
            );
            assert_eq!(
                fs::read(harness.path().join("receipt-at-release")).unwrap(),
                receipt
            );
            assert_eq!(
                fs::read(harness.path().join("history-at-release")).unwrap(),
                history
            );
            assert_eq!(fs::read(fixture.path(RECEIPT)).unwrap(), receipt);
            assert_eq!(fixture.state()["iteration"], before["iteration"]);
            assert_eq!(fixture.state()["baseline_ref"], before["baseline_ref"]);
            assert_eq!(harness.path().join("mutated").exists(), mutate);
            if mutate {
                assert_ne!(fs::read(&executable).unwrap(), PROBE.as_bytes());
                assert!(
                    !output.status.success(),
                    "late executable mutation must stop completion: {output:?}"
                );
                assert_ne!(fixture.state()["status"], "learned");
            } else {
                assert_eq!(fs::read(&executable).unwrap(), PROBE.as_bytes());
                assert!(
                    output.status.success(),
                    "unchanged legacy re-audit must complete: {output:?}"
                );
                assert_eq!(fixture.state()["status"], "learned");
            }
        }
    }

    #[test]
    fn current_receipt_can_complete_history_and_completed_status_remains_historical() {
        let fixture = Fixture::new("success", 10);
        fixture.prepare();
        assert!(fixture.verify().0.status.success());
        fixture.write_report();
        assert!(fixture.post().status.success());
        fixture.resolve_history_review();
        let resumed = fixture.run(&["run-task", SPEC]);
        assert!(resumed.status.success(), "{resumed:?}");
        assert_eq!(fixture.state()["status"], "learned");

        fs::write(fixture.path(".mastermind/probe.sh"), "#!/bin/sh\nexit 7\n").unwrap();
        let historical = fixture.run(&["run-task", SPEC]);
        assert!(historical.status.success(), "{historical:?}");
        assert_eq!(fixture.state()["status"], "learned");
        assert!(!fixture.post().status.success());
        assert_ne!(fixture.state()["status"], "learned");
    }

    #[test]
    fn observed_failure_overrules_reported_pass_and_revokes_a_previous_success() {
        let fixture = Fixture::new("switch", 10);
        fixture.prepare();
        assert!(fixture.verify().0.status.success());
        fixture.write_report();
        fs::write(fixture.path(".mastermind/fail"), "fail").unwrap();
        let (output, receipt) = fixture.verify();
        assert!(!output.status.success());
        assert_eq!(receipt["status"], "failed");
        assert_eq!(receipt["exit_code"], 7);
        assert_eq!(receipt["attempt"], 2);
        assert_receipt_broken(&fixture);
        assert!(!fixture.post().status.success());
    }

    #[test]
    fn literal_argv_and_cwd_are_observed_without_shell_expansion() {
        let literal = "$(touch SHOULD_NOT_EXIST); echo injected";
        let argv = vec![
            "../.mastermind/probe.sh".into(),
            "args".into(),
            "two words".into(),
            literal.into(),
            "".into(),
        ];
        let fixture = Fixture::with_argv(argv, "sub", 10);
        fixture.prepare();
        let (output, receipt) = fixture.verify();
        assert!(output.status.success(), "{output:?}");
        let expected = format!("two words\n{literal}\n\n");
        assert_eq!(
            receipt["stdout"]["sha256"],
            Sha256::digest(expected.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        assert!(!fixture.path("sub/SHOULD_NOT_EXIST").exists());
    }

    #[test]
    fn stale_dependency_spec_preflight_and_executable_invalidate_receipts() {
        for change in [
            "dependency",
            "hidden_dependency",
            "spec",
            "preflight",
            "executable",
            "head",
        ] {
            let fixture = Fixture::new("success", 10);
            fixture.prepare();
            assert!(fixture.verify().0.status.success());
            fixture.write_report();
            match change {
                "dependency" => {
                    fs::write(fixture.path("sub/README.md"), "changed dependency\n").unwrap()
                }
                "hidden_dependency" => {
                    assert!(Fixture::git_command(fixture.root())
                        .args(["update-index", "--assume-unchanged", "sub/README.md"])
                        .status()
                        .unwrap()
                        .success());
                    fs::write(fixture.path("sub/README.md"), "hidden dependency change\n").unwrap();
                }
                "spec" => {
                    let spec = fs::read_to_string(fixture.path(SPEC)).unwrap();
                    fs::write(
                        fixture.path(SPEC),
                        spec.replace("Update the service.", "Update the service safely."),
                    )
                    .unwrap();
                    let pre = fixture.run(&["run-task", SPEC, "--pre-only"]);
                    assert!(pre.status.success(), "{pre:?}");
                }
                "preflight" => assert!(fixture
                    .run(&["run-task", SPEC, "--pre-only"])
                    .status
                    .success()),
                "executable" => fs::write(
                    fixture.path(".mastermind/probe.sh"),
                    format!("{PROBE}\n# changed executable\n"),
                )
                .unwrap(),
                "head" => assert!(Fixture::git_command(fixture.root())
                    .args(["commit", "--allow-empty", "-q", "-m", "new head"])
                    .status()
                    .unwrap()
                    .success()),
                _ => unreachable!(),
            }
            assert_receipt_broken(&fixture);
        }
    }

    #[test]
    fn input_mutation_during_execution_is_not_a_pass() {
        let fixture = Fixture::new("change", 10);
        fixture.prepare();
        let (output, receipt) = fixture.verify();
        assert!(!output.status.success());
        assert_eq!(receipt["exit_code"], 0);
        assert_eq!(receipt["status"], "input_changed");
        fixture.write_report();
        assert_receipt_broken(&fixture);
    }

    #[test]
    fn timeout_output_limit_and_successful_parent_cleanup_are_real_process_boundaries() {
        let fixture = Fixture::new("sleep", 1);
        fixture.prepare();
        let start = Instant::now();
        let (output, receipt) = fixture.verify();
        assert!(!output.status.success());
        assert_eq!(receipt["status"], "timeout");
        assert!(start.elapsed() < Duration::from_secs(10));
        let child: i32 = fs::read_to_string(fixture.path(".mastermind/child"))
            .unwrap()
            .parse()
            .unwrap();
        no_running_process(child);

        let fixture = Fixture::new("output", 10);
        fixture.prepare();
        let (output, receipt) = fixture.verify();
        assert!(!output.status.success());
        assert_eq!(receipt["status"], "output_limit");

        let fixture = Fixture::new("background", 10);
        fixture.prepare();
        let (output, receipt) = fixture.verify();
        assert!(output.status.success(), "{output:?}: {receipt}");
        let child: i32 = fs::read_to_string(fixture.path(".mastermind/child"))
            .unwrap()
            .parse()
            .unwrap();
        no_running_process(child);
    }

    #[test]
    fn concurrent_writer_and_interruption_leave_no_success_receipt() {
        let fixture = Fixture::new("sleep", 30);
        fixture.prepare();
        let mut runner = fixture.spawn_verification();
        fixture.await_started();
        let second = fixture.run(&["verification", "run", SPEC, "--id", "unit", "--json"]);
        assert!(!second.status.success());
        assert!(
            String::from_utf8_lossy(&second.stderr).contains("verification_busy"),
            "{second:?}"
        );
        // SAFETY: this is the owned CLI child created by the fixture.
        unsafe {
            libc::kill(runner.id() as i32, libc::SIGTERM);
        }
        assert!(!runner.wait().unwrap().success());
        let receipt: Value =
            serde_json::from_slice(&fs::read(fixture.path(RECEIPT)).unwrap()).unwrap();
        assert_eq!(receipt["status"], "interrupted");
        let child: i32 = fs::read_to_string(fixture.path(".mastermind/child"))
            .unwrap()
            .parse()
            .unwrap();
        no_running_process(child);
    }

    #[test]
    fn killed_runner_leaves_pending_and_cannot_reuse_old_success() {
        let fixture = Fixture::new("switch", 30);
        fixture.prepare();
        assert!(fixture.verify().0.status.success());
        fs::write(fixture.path(".mastermind/block"), "block").unwrap();
        let mut runner = fixture.spawn_verification();
        fixture.await_started();
        let group: i32 = fs::read_to_string(fixture.path(".mastermind/started"))
            .unwrap()
            .parse()
            .unwrap();
        runner.kill().unwrap();
        runner.wait().unwrap();
        // SIGKILL cannot run Rust cleanup; the test owns and removes its child group.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
        let receipt: Value =
            serde_json::from_slice(&fs::read(fixture.path(RECEIPT)).unwrap()).unwrap();
        assert_eq!(receipt["status"], "pending");
        assert_eq!(receipt["attempt"], 2);
        fixture.write_report();
        assert_receipt_broken(&fixture);
    }

    #[test]
    fn malformed_large_and_linked_receipts_fail_closed() {
        let fixture = Fixture::new("success", 10);
        fixture.prepare();
        assert!(fixture.verify().0.status.success());
        fixture.write_report();
        let original = fs::read(fixture.path(RECEIPT)).unwrap();
        fs::write(fixture.path(RECEIPT), b"{invalid").unwrap();
        assert_receipt_broken(&fixture);
        fs::write(fixture.path(RECEIPT), vec![b' '; 128 * 1024 + 1]).unwrap();
        assert_receipt_broken(&fixture);
        fs::remove_file(fixture.path(RECEIPT)).unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        fs::write(outside.path(), &original).unwrap();
        symlink(outside.path(), fixture.path(RECEIPT)).unwrap();
        assert_receipt_broken(&fixture);
        let runner = fixture.run(&["verification", "run", SPEC, "--id", "unit"]);
        assert!(!runner.status.success());
        assert_eq!(fs::read(outside.path()).unwrap(), original);
    }

    #[test]
    fn a_receipt_from_another_repository_cannot_satisfy_the_same_task_name() {
        let source = Fixture::new("success", 10);
        source.prepare();
        assert!(source.verify().0.status.success());
        let destination = Fixture::new("success", 10);
        destination.prepare();
        destination.write_report();
        fs::create_dir_all(destination.path(RECEIPT).parent().unwrap()).unwrap();
        fs::write(destination.path(RECEIPT).with_file_name("run.lock"), "").unwrap();
        fs::copy(source.path(RECEIPT), destination.path(RECEIPT)).unwrap();
        assert_receipt_broken(&destination);
    }

    #[test]
    fn malformed_run_declarations_cannot_enter_approved_state() {
        let fixture = Fixture::new("success", 10);
        let source = fs::read_to_string(fixture.path(SPEC)).unwrap();
        let metadata: Value = serde_norway::from_str(source.split("---").nth(1).unwrap()).unwrap();
        let body = source.splitn(3, "---").nth(2).unwrap();
        for invalid in [
            "command",
            "cwd",
            "duplicate",
            "unknown",
            "timeout",
            "empty_argv",
        ] {
            let mut value = metadata.clone();
            match invalid {
                "command" => value["verify"][0]["cmd"] = json!("true"),
                "cwd" => value["verify"][0]["run"]["cwd"] = json!("../outside"),
                "duplicate" => {
                    let entry = value["verify"][0].clone();
                    value["verify"].as_array_mut().unwrap().push(entry);
                }
                "unknown" => value["verify"][0]["run"]["ignored"] = json!(true),
                "timeout" => value["verify"][0]["run"]["timeout_secs"] = json!(0),
                "empty_argv" => value["verify"][0]["run"]["argv"] = json!([]),
                _ => unreachable!(),
            }
            fs::write(
                fixture.path(SPEC),
                format!("---\n{}---{body}", serde_norway::to_string(&value).unwrap()),
            )
            .unwrap();
            let output = fixture.run(&["run-task", SPEC, "--pre-only"]);
            assert!(!output.status.success(), "{invalid}: {output:?}");
            assert!(!fixture.path(STATE).exists(), "{invalid}");
            assert!(!fixture.path(".mastermind/ran").exists(), "{invalid}");
        }
    }

    #[test]
    fn legacy_report_only_contract_stays_compatible_and_invalid_run_is_not_downgraded() {
        let fixture = Fixture::new("success", 10);
        let original = fs::read_to_string(fixture.path(SPEC)).unwrap();
        let mut metadata: Value =
            serde_norway::from_str(original.split("---").nth(1).unwrap()).unwrap();
        metadata["verify"][0].as_object_mut().unwrap().remove("run");
        let body = original.splitn(3, "---").nth(2).unwrap();
        fs::write(
            fixture.path(SPEC),
            format!(
                "---\n{}---{body}",
                serde_norway::to_string(&metadata).unwrap()
            ),
        )
        .unwrap();
        fixture.prepare();
        fixture.write_report();
        let output = fixture.post();
        assert!(output.status.success(), "{output:?}");
        assert!(!fixture.path(".mastermind/ran").exists());

        let fixture = Fixture::new("success", 10);
        let source = fs::read_to_string(fixture.path(SPEC)).unwrap();
        fs::write(
            fixture.path(SPEC),
            source.replace("timeout_secs: 10", "timeout_secs: 0"),
        )
        .unwrap();
        let output = fixture.run(&["run-task", SPEC, "--pre-only"]);
        assert!(!output.status.success(), "{output:?}");
        assert!(!fixture.path(STATE).exists());
    }
}

#[cfg(not(unix))]
#[test]
fn unsupported_platform_refuses_before_loading_or_executing_a_command() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mmcg"))
        .args([
            "verification",
            "run",
            "missing-spec.md",
            "--id",
            "unit",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("verification_execution_unsupported_platform"));
}
