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
    // impact prints only a code, so the code names the problem. The other
    // commands say it in words.
    let missing_text = format!("index `{}` does not exist", missing.display());
    let schema_text = format!(
        "index schema at `{}` is missing or outdated",
        repo.index.display()
    );
    for (index, said, code) in [
        (&missing, &missing_text, "index_missing"),
        (&repo.index, &schema_text, "schema_incompatible"),
    ] {
        for args in &commands {
            let output = run(index, args);
            let error = stderr(&output);
            assert_eq!(
                output.status.code(),
                Some(1),
                "{args:?} on {index:?}: {error}"
            );
            assert!(output.stdout.is_empty(), "{args:?} printed no answer");
            assert!(
                !error.contains(SKIPPED),
                "{args:?} failed before the skip: {error}"
            );
            if args[0] == "impact" {
                assert_eq!(error.trim(), code, "{args:?} on {index:?}");
            } else {
                assert!(
                    error.contains(said.as_str()),
                    "{args:?} on {index:?}: {error}"
                );
                assert!(
                    !error.contains("index_stale"),
                    "{args:?} on {index:?}: {error}"
                );
            }
        }
    }
}

fn impact_code(repo: &Indexed, since: &str) -> (Option<i32>, String) {
    let root = repo.source.path().to_str().unwrap();
    let output = run(
        &repo.index,
        &[
            "impact",
            "--since",
            since,
            "--root",
            root,
            "--format",
            "json",
            "--skip-freshness-check",
        ],
    );
    let error = stderr(&output);
    let code = error
        .lines()
        .filter(|line| *line != SKIPPED && !line.starts_with("warning:"))
        .collect::<Vec<_>>()
        .join("\n");
    (output.status.code(), code)
}

fn head(root: &Path) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn assert_stale(repo: &Indexed, since: &str, case: &str) {
    assert_eq!(
        impact_code(repo, since),
        (Some(1), "index_stale".to_string()),
        "{case}"
    );
}

fn assert_current(repo: &Indexed, since: &str, case: &str) {
    let (status, error) = impact_code(repo, since);
    assert_eq!(status, Some(0), "{case}: {error}");
}

fn reindex(repo: &Indexed) {
    succeeds(
        &repo.index,
        &["index", repo.source.path().to_str().unwrap()],
    );
}

#[test]
fn impact_fails_on_a_deleted_file_even_inside_the_since_diff() {
    let repo = indexed(|root| std::fs::remove_file(root.join("gone.rs")).unwrap());
    let base = head(repo.source.path());
    assert_stale(&repo, &base, "an uncommitted delete");
    reindex(&repo);
    assert_current(&repo, &base, "the same delete after index");
}

#[test]
fn impact_fails_on_a_file_committed_after_indexing_outside_the_since_diff() {
    let repo = indexed(|root| {
        std::fs::write(root.join("lib.rs"), "pub fn changed_after_index() {}\n").unwrap();
        git(root, &["commit", "-q", "-am", "edit"]);
    });
    assert_stale(&repo, "HEAD", "lib.rs is not in the diff from HEAD");
    reindex(&repo);
    assert_current(&repo, "HEAD", "the same commit after index");
}

#[test]
fn impact_fails_on_a_committed_delete() {
    let repo = indexed(|root| {
        git(root, &["rm", "-q", "gone.rs"]);
        git(root, &["commit", "-q", "-m", "delete"]);
    });
    assert_stale(&repo, "HEAD", "gone.rs was deleted and committed");
    reindex(&repo);
    assert_current(&repo, "HEAD", "the same delete after index");
}

#[test]
fn impact_fails_when_a_file_dirty_at_index_time_goes_back_to_the_commit() {
    let repo = indexed(|_| {});
    let root = repo.source.path();
    std::fs::write(root.join("lib.rs"), "pub fn only_in_the_index() {}\n").unwrap();
    reindex(&repo);
    assert_current(&repo, "HEAD", "the index holds the edit");
    git(root, &["checkout", "-q", "--", "lib.rs"]);
    assert_stale(&repo, "HEAD", "lib.rs matches HEAD but not the index");
}

#[test]
fn impact_ignores_files_the_index_never_holds() {
    let repo = indexed(|root| {
        std::fs::write(root.join("notes.txt"), "not code\n").unwrap();
        std::fs::write(root.join("blob.rs"), b"pub fn x() {}\0binary").unwrap();
    });
    assert_current(&repo, "HEAD", "a new text file and a binary source");
    git(repo.source.path(), &["add", "."]);
    git(repo.source.path(), &["commit", "-q", "-m", "more"]);
    assert_current(&repo, "HEAD", "the same files committed");
}

#[test]
fn impact_on_an_index_with_no_recorded_commit_falls_back_to_the_full_scan() {
    let repo = indexed(|_| {});
    let connection = rusqlite::Connection::open(&repo.index).unwrap();
    let removed = connection
        .execute(
            "DELETE FROM meta WHERE key IN ('indexed_commit', 'indexed_changed_paths')",
            [],
        )
        .unwrap();
    assert_eq!(removed, 2, "the index recorded its commit");
    drop(connection);
    assert_current(&repo, "HEAD", "an index current with the files");
    let root = repo.source.path();
    std::fs::write(root.join("lib.rs"), "pub fn changed_after_index() {}\n").unwrap();
    later_mtime(&root.join("lib.rs"));
    git(root, &["commit", "-q", "-am", "edit"]);
    assert_stale(&repo, "HEAD", "lib.rs committed after indexing");
}

#[test]
fn impact_names_a_newer_or_damaged_index() {
    let repo = indexed(|_| {});
    set_meta(&repo.index, "schema_version", "999");
    assert_eq!(
        impact_code(&repo, "HEAD"),
        (Some(1), "schema_incompatible".to_string())
    );
    std::fs::write(&repo.index, b"this is not a database, only text").unwrap();
    for sidecar in ["graph.db-wal", "graph.db-shm"] {
        let _ = std::fs::remove_file(repo.index.with_file_name(sidecar));
    }
    assert_eq!(
        impact_code(&repo, "HEAD"),
        (Some(1), "index_unavailable".to_string())
    );
}
