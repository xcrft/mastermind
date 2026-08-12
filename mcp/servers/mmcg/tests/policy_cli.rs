use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

#[test]
fn policy_cli_emits_sarif_and_uses_exit_status_as_the_gate() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path();
    git(repo, &["init", "-b", "main"]);
    git(repo, &["config", "user.email", "policy@example.com"]);
    git(repo, &["config", "user.name", "Policy Test"]);
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join("src/app.py"), "def app():\n    return 1\n").unwrap();
    fs::write(
        repo.join("mastermind-policy.yml"),
        "rules:\n  - id: critical\n    critical: src/**\n    require_workflow: strict\n",
    )
    .unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-m", "baseline"]);

    let binary = env!("CARGO_BIN_EXE_mmcg");
    let indexed = Command::new(binary)
        .args(["index", ".", "--force"])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(indexed.status.success(), "{:?}", indexed);

    let clean = policy(binary, repo);
    assert!(clean.status.success(), "{:?}", clean);
    let clean: Value = serde_json::from_slice(&clean.stdout).unwrap();
    assert_eq!(clean["runs"][0]["properties"]["passed"], true);
    assert_eq!(clean["runs"][0]["results"].as_array().unwrap().len(), 0);

    fs::write(repo.join("src/app.py"), "def app():\n    return 2\n").unwrap();
    let refreshed = Command::new(binary)
        .args(["index", ".", "--force"])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(refreshed.status.success(), "{:?}", refreshed);

    let failed = policy(binary, repo);
    assert!(
        !failed.status.success(),
        "policy violation must exit non-zero"
    );
    let failed: Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_eq!(failed["runs"][0]["properties"]["passed"], false);
    assert_eq!(failed["runs"][0]["properties"]["partial"], false);
    assert_eq!(failed["runs"][0]["results"][0]["ruleId"], "critical");
    assert_eq!(failed["runs"][0]["results"][0]["level"], "error");
}

fn policy(binary: &str, root: &Path) -> Output {
    Command::new(binary)
        .args(["policy", "check", "--since", "HEAD", "--format", "sarif"])
        .current_dir(root)
        .output()
        .unwrap()
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
}
