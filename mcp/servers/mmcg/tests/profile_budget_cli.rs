//! CLI coverage for the configurable profile delivery budget: `init
//! --profile-budget` persistence and range enforcement, and the `doctor`
//! "profile budget" check wired into the real command.
#![cfg(unix)]

use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        for name in ["project", "home"] {
            fs::create_dir(temp.path().join(name)).unwrap();
        }
        let fixture = Self {
            root: temp.path().join("project").canonicalize().unwrap(),
            home: temp.path().join("home").canonicalize().unwrap(),
            _temp: temp,
        };
        let output = fixture
            .command("/usr/bin/git")
            .args(["init", "-q", "--initial-branch=main"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        fixture
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        command
    }

    fn mmcg(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_mmcg"))
            .args(args)
            .output()
            .unwrap()
    }

    fn success(&self, args: &[&str]) -> Value {
        let output = self.mmcg(args);
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error}: {output:?}"))
    }

    fn setup_json(&self) -> Option<Value> {
        let path = self.root.join(".mastermind/setup.json");
        let bytes = fs::read(path).ok()?;
        Some(serde_json::from_slice(&bytes).unwrap())
    }
}

#[test]
fn dry_run_changes_only_the_profile_budget_key() {
    let f = Fixture::new();
    let baseline = f.success(&["init", "--client", "none", "--dry-run", "--json"]);
    let planned = f.success(&[
        "init",
        "--client",
        "none",
        "--profile-budget",
        "6000",
        "--dry-run",
        "--json",
    ]);
    assert_eq!(planned["settings"]["profile_budget_tokens"], 6000);
    let mut without_budget = planned["settings"].clone();
    without_budget
        .as_object_mut()
        .unwrap()
        .remove("profile_budget_tokens");
    let mut baseline_settings = baseline["settings"].clone();
    baseline_settings
        .as_object_mut()
        .unwrap()
        .remove("profile_budget_tokens");
    assert_eq!(
        without_budget, baseline_settings,
        "only the profile budget changed"
    );
}

#[test]
fn out_of_range_profile_budget_values_are_rejected() {
    let f = Fixture::new();
    for value in ["100", "9000"] {
        let output = f.mmcg(&[
            "init",
            "--client",
            "none",
            "--profile-budget",
            value,
            "--dry-run",
        ]);
        assert!(
            !output.status.success(),
            "{value} must be rejected: {output:?}"
        );
        assert!(!f.root.join(".mastermind").exists());
    }
}

#[test]
fn the_default_profile_budget_is_not_persisted_and_a_set_value_survives_reinit() {
    let f = Fixture::new();
    f.success(&["init", "--client", "none", "--json"]);
    let setup = f.setup_json().expect("setup.json written");
    assert!(
        setup.get("profile_budget_tokens").is_none(),
        "the default value must be omitted: {setup}"
    );

    let configured = f.success(&[
        "init",
        "--client",
        "none",
        "--profile-budget",
        "5500",
        "--json",
    ]);
    assert_eq!(configured["settings"]["profile_budget_tokens"], 5500);
    let setup = f.setup_json().unwrap();
    assert_eq!(setup["profile_budget_tokens"], 5500);

    // Re-running init without the flag keeps the previously saved value.
    let again = f.success(&["init", "--client", "none", "--json"]);
    assert_eq!(again["settings"]["profile_budget_tokens"], 5500);
}

#[test]
fn doctor_reports_profile_budget_as_not_configured_by_default() {
    let f = Fixture::new();
    f.success(&["init", "--client", "none", "--json"]);
    // Other checks (e.g. an empty index) can fail the exit code in this
    // minimal fixture; only the "profile budget" check is under test here.
    let output = f.mmcg(&["doctor", "--json"]);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let checks = report["checks"].as_array().unwrap();
    let check = checks
        .iter()
        .find(|check| check["name"] == "profile budget")
        .expect("profile budget check is present");
    assert_eq!(check["status"], "ok");
    assert!(check["message"]
        .as_str()
        .unwrap()
        .contains("not configured"));
}
