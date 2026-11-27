use std::path::Path;
use std::process::{Command, Output};

const STALE: &str = "warning: index is not current";

fn mmcg(index: &Path, args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_mmcg"))
        .arg("--index")
        .arg(index)
        .args(args)
        .env("MMCG_QUERY_BUDGET_MS", "60000")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn stale_index() -> (tempfile::TempDir, tempfile::TempDir) {
    let source = tempfile::tempdir().unwrap();
    let graph = tempfile::tempdir().unwrap();
    let file = source.path().join("lib.rs");
    std::fs::write(&file, "pub fn pending_file() {}\n").unwrap();
    let index = graph.path().join("graph.db");
    mmcg(&index, &["index", source.path().to_str().unwrap()]);
    std::fs::write(&file, "pub fn pending_file() {}\npub fn added() {}\n").unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(later)
        .unwrap();
    (source, graph)
}

#[test]
fn a_query_warns_about_a_stale_index_unless_the_caller_skips_the_check() {
    let (_source, graph) = stale_index();
    let index = graph.path().join("graph.db");

    let checked = mmcg(&index, &["query", "search", "pending_file"]);
    let skipped = mmcg(
        &index,
        &["query", "--skip-freshness-check", "search", "pending_file"],
    );

    assert!(String::from_utf8_lossy(&checked.stderr).contains(STALE));
    assert!(!String::from_utf8_lossy(&skipped.stderr).contains(STALE));
    assert_eq!(checked.stdout, skipped.stdout);
    let rows: serde_json::Value = serde_json::from_slice(&skipped.stdout).unwrap();
    assert!(rows.to_string().contains("pending_file"));
    assert!(!rows.to_string().contains("added"));
}
