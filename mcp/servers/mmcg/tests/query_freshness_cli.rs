use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const STALE: &str = "warning: index is not current";
const SKIPPED: &str = "freshness check skipped";
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn run(index: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mmcg"))
        .arg("--index")
        .arg(index)
        .args(args)
        .env("MMCG_QUERY_BUDGET_MS", "60000")
        .output()
        .unwrap()
}

fn succeeds(index: &Path, args: &[&str]) -> Output {
    let output = run(index, args);
    assert!(
        output.status.success(),
        "{args:?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(root)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// A committed repository with two files, indexed, then changed by `change`
/// so the index is behind the files.
struct Indexed {
    source: tempfile::TempDir,
    _graph: tempfile::TempDir,
    index: PathBuf,
}

fn indexed(change: impl FnOnce(&Path)) -> Indexed {
    let source = tempfile::tempdir().unwrap();
    let graph = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("lib.rs"), "pub fn pending_file() {}\n").unwrap();
    std::fs::write(source.path().join("gone.rs"), "pub fn gone_file() {}\n").unwrap();
    git(source.path(), &["init", "-q"]);
    git(source.path(), &["add", "."]);
    git(source.path(), &["commit", "-q", "-m", "start"]);
    let index = graph.path().join("graph.db");
    succeeds(&index, &["index", source.path().to_str().unwrap()]);
    change(source.path());
    Indexed {
        source,
        _graph: graph,
        index,
    }
}

fn later_mtime(file: &Path) {
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(later)
        .unwrap();
}

fn edit_lib(root: &Path) {
    let file = root.join("lib.rs");
    std::fs::write(&file, "pub fn pending_file() {}\npub fn added() {}\n").unwrap();
    later_mtime(&file);
}

/// The freshness check sees a changed file, but its content and Git do not
/// change, so `impact` can still answer.
fn touch_lib(root: &Path) {
    later_mtime(&root.join("lib.rs"));
}

fn set_meta(index: &Path, key: &str, value: &str) {
    let connection = rusqlite::Connection::open(index).unwrap();
    let changed = connection
        .execute("UPDATE meta SET value = ?2 WHERE key = ?1", [key, value])
        .unwrap();
    assert_eq!(changed, 1, "meta {key}");
}

/// Runs `args` with and without the flag, which goes where `at` says, and
/// checks that only the flagged run skips the check and says so.
fn compare(index: &Path, args: &[&str], at: usize) {
    let checked = succeeds(index, args);
    let mut flagged = args.to_vec();
    flagged.insert(at, "--skip-freshness-check");
    let skipped = succeeds(index, &flagged);
    assert!(stderr(&checked).contains(STALE), "{args:?} warns");
    assert!(!stderr(&checked).contains(SKIPPED));
    assert!(
        stderr(&skipped).lines().any(|line| line == SKIPPED),
        "{flagged:?} says it skipped the check:\n{}",
        stderr(&skipped)
    );
    assert!(
        !stderr(&skipped).contains(STALE),
        "{flagged:?} does not warn"
    );
    assert_eq!(checked.stdout, skipped.stdout, "{args:?} answers the same");
}

#[test]
fn query_skips_the_check_before_its_subcommand_and_says_so() {
    let repo = indexed(edit_lib);
    compare(&repo.index, &["query", "search", "pending_file"], 1);
    let answer = succeeds(
        &repo.index,
        &["query", "--skip-freshness-check", "search", "added"],
    );
    let answer: serde_json::Value = serde_json::from_slice(&answer.stdout).unwrap();
    assert_eq!(
        answer["results"],
        serde_json::json!([]),
        "The index is behind the files, so the new function is not in the answer"
    );
}

#[test]
fn a_deleted_file_still_answers_from_the_index_with_no_warning() {
    let repo = indexed(|root| std::fs::remove_file(root.join("gone.rs")).unwrap());
    compare(&repo.index, &["query", "search", "gone_file"], 1);
    let answer = succeeds(
        &repo.index,
        &["query", "--skip-freshness-check", "search", "gone_file"],
    );
    assert!(String::from_utf8_lossy(&answer.stdout).contains("gone.rs"));
}

#[test]
fn an_index_of_an_older_extractor_still_warns_with_the_flag() {
    let repo = indexed(|_| {});
    set_meta(
        &repo.index,
        "extractor_contract_version",
        "mmcg-extractors-v0",
    );
    let skipped = succeeds(
        &repo.index,
        &["query", "--skip-freshness-check", "search", "pending_file"],
    );
    let said = stderr(&skipped);
    assert!(said.lines().any(|line| line == SKIPPED));
    assert!(
        said.contains(STALE) && said.contains("built by an older extractor"),
        "{said}"
    );
}

#[test]
fn map_repo_map_and_impact_take_the_flag_too() {
    let repo = indexed(touch_lib);
    let root = repo.source.path().to_str().unwrap();
    compare(&repo.index, &["map"], 1);
    compare(
        &repo.index,
        &["repo-map", "pending", "--source-sha256", DIGEST],
        1,
    );
    compare(
        &repo.index,
        &["impact", "--since", "HEAD", "--root", root],
        1,
    );
}

#[test]
fn a_missing_or_old_schema_index_still_fails_with_the_flag() {
    let repo = indexed(|_| {});
    let root = repo.source.path().to_str().unwrap().to_string();
    let missing = repo.index.with_file_name("missing.db");
    set_meta(&repo.index, "schema_version", "0");
    let commands: [Vec<&str>; 4] = [
        vec!["query", "--skip-freshness-check", "search", "pending_file"],
        vec!["map", "--skip-freshness-check"],
        vec![
            "repo-map",
            "pending",
            "--source-sha256",
            DIGEST,
            "--skip-freshness-check",
        ],
        vec![
            "impact",
            "--since",
            "HEAD",
            "--root",
            &root,
            "--skip-freshness-check",
        ],
    ];
    for index in [&missing, &repo.index] {
        for args in &commands {
            let output = run(index, args);
            assert_eq!(
                output.status.code(),
                Some(1),
                "{args:?} on {index:?}: {}",
                stderr(&output)
            );
            assert!(output.stdout.is_empty(), "{args:?} printed no answer");
        }
    }
}
