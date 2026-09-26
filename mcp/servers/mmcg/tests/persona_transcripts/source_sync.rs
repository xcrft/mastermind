use super::*;

fn result(f: &Fixture, args: &[&str]) -> Value {
    serde_json::from_slice(&f.success(args).stdout).unwrap()
}

fn source(f: &Fixture, id: &str, quotes: &[&str]) -> PathBuf {
    f.write(
        &format!("sessions/2026/09/26/rollout-{id}.jsonl"),
        &f.preferences(id, quotes),
    )
}

fn sources(f: &Fixture) -> Value {
    result(f, &["miner", "sources", "list"])
}

#[test]
fn source_sync_exclusion_preserves_evidence_and_survives_explicit_archive_collection() {
    let f = Fixture::new();
    let a = source(&f, "source-a", &["I prefer short replies."]);
    let b = source(&f, "source-b", &["I prefer brief replies."]);
    assert!(f.collect(&[&a, &b], false).status.success());
    let inbox = f.inbox();
    let candidate = inbox["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["candidate"]["source"] == "session:codex:source-a")
        .unwrap()["candidate"]
        .clone();
    let proposal = f.propose_preference(&candidate, "Prefer short replies", None);
    assert!(proposal.status.success(), "{proposal:?}");
    let revision = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .aggregate()
        .unwrap()
        .profile_revision();
    let style = f.style();
    let archive = f.write(
        "archived_sessions/rollout-excluded.jsonl",
        &fs::read_to_string(&a).unwrap(),
    );
    fs::remove_file(&a).unwrap();
    fs::write(
        &b,
        f.preferences("source-b", &["I usually review code before changing it."]),
    )
    .unwrap();
    let excluded = result(
        &f,
        &["miner", "sources", "exclude", "session:codex:source-a"],
    );
    assert_eq!(excluded["sync_enabled"], false);
    assert_eq!(sources(&f)["sources"][0]["sync_enabled"], false);
    let synced = result(&f, &["miner", "sync"]);
    assert_eq!(synced["source_ids"], json!(["session:codex:source-b"]));
    assert_eq!(synced["collection"]["sources_updated"], 1);
    assert!(f.collect(&[&archive], false).status.success());
    assert_eq!(sources(&f)["sources"][0]["sync_enabled"], false);
    assert_eq!(result(&f, &["miner", "sync"])["sources_selected"], 1);
    let shown = result(
        &f,
        &[
            "miner",
            "candidates",
            "show",
            candidate["id"].as_str().unwrap(),
        ],
    );
    assert_eq!(
        shown["history"]["preference_proposals"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(shown["history"]["revisions"].as_array().unwrap().len(), 2);
    assert_eq!(f.style(), style);
    assert_eq!(
        ProfileStore::open_read_only(&f.db_path())
            .unwrap()
            .aggregate()
            .unwrap()
            .profile_revision(),
        revision
    );
    result(
        &f,
        &["miner", "sources", "include", "session:codex:source-a"],
    );
    assert_eq!(
        result(&f, &["miner", "sync"])["collection"]["sources_unchanged"],
        2
    );
}

#[test]
fn source_sync_exclusion_filters_before_pagination_and_requires_the_exact_project() {
    let f = Fixture::new();
    let a = source(&f, "source-a", &[]);
    let b = source(&f, "source-b", &[]);
    assert!(f.collect(&[&a, &b], false).status.success());
    fs::remove_file(&a).unwrap();
    fs::remove_file(&b).unwrap();
    for id in ["session:codex:source-a", "session:codex:source-b"] {
        result(&f, &["miner", "sources", "exclude", id]);
    }
    let before = fs::read(f.db_path()).unwrap();
    let empty = result(&f, &["miner", "sync", "--limit", "1"]);
    assert_eq!(empty["sources_selected"], 0);
    assert!(empty["next_cursor"].is_null());
    result(
        &f,
        &["miner", "sources", "exclude", "session:codex:source-a"],
    );
    assert_eq!(fs::read(f.db_path()).unwrap(), before);
    let other = f._temp.path().join("other");
    fs::create_dir(&other).unwrap();
    for args in [
        vec![
            "miner",
            "sources",
            "include",
            "session:codex:source-a",
            "--project-root",
            other.to_str().unwrap(),
        ],
        vec!["miner", "sources", "exclude", "session:codex:missing"],
        vec!["miner", "sources", "exclude", "partial-id"],
    ] {
        assert!(!f.run(&args).status.success());
        assert_eq!(fs::read(f.db_path()).unwrap(), before);
    }
    let list = result(&f, &["miner", "sources", "list", "--limit", "1"]);
    assert_eq!(list["sources"][0]["sync_enabled"], false);
    assert_eq!(list["next_cursor"], "session:codex:source-a");
    fs::write(&b, f.preferences("source-b", &[])).unwrap();
    result(
        &f,
        &["miner", "sources", "include", "session:codex:source-b"],
    );
    let selected = result(&f, &["miner", "sync", "--limit", "1"]);
    assert_eq!(selected["source_ids"], json!(["session:codex:source-b"]));
    assert!(selected["next_cursor"].is_null());
}

#[test]
fn source_sync_empty_invalid_and_legacy_selections_do_not_create_or_migrate_a_store() {
    let f = Fixture::new();
    assert_eq!(sources(&f)["sources"], json!([]));
    for args in [vec!["miner", "sync"], vec!["miner", "sync", "--dry-run"]] {
        let value = result(&f, &args);
        assert_eq!(value["sources_selected"], 0);
        assert!(value["next_cursor"].is_null());
    }
    for args in [
        vec!["miner", "sync", "--limit", "17"],
        vec!["miner", "sync", "--limit", "0"],
        vec!["miner", "sync", "--after", "session:codex:"],
        vec!["miner", "sources", "list", "--after", "source-prefix"],
        vec!["miner", "sources", "list", "--limit", "101"],
    ] {
        assert!(!f.run(&args).status.success());
    }
    assert!(!f.home.join(".mastermind").exists());

    fs::create_dir(f.db_path().parent().unwrap()).unwrap();
    let db = Connection::open(f.db_path()).unwrap();
    db.execute_batch(
        "CREATE TABLE legacy_fixture(value TEXT); INSERT INTO legacy_fixture VALUES('retained')",
    )
    .unwrap();
    drop(db);
    let original = fs::read(f.db_path()).unwrap();
    assert_eq!(sources(&f)["sources"], json!([]));
    assert_eq!(result(&f, &["miner", "sync"])["sources_selected"], 0);
    assert_eq!(
        result(&f, &["miner", "sync", "--dry-run"])["sources_selected"],
        0
    );
    assert_eq!(fs::read(f.db_path()).unwrap(), original);
    assert!(!f.home.join(".mastermind/.style-profile.lock").exists());
}

#[test]
fn source_sync_pages_exact_projects_and_never_reads_the_lookahead_or_discovers_files() {
    let f = Fixture::new();
    let paths: Vec<_> = (0..17)
        .map(|i| source(&f, &format!("source-{i:02}"), &[]))
        .collect();
    for chunk in paths.chunks(16) {
        let out = f.collect(
            &chunk.iter().map(PathBuf::as_path).collect::<Vec<_>>(),
            false,
        );
        assert!(out.status.success(), "{out:?}");
    }
    // Neither a different root nor a never-selected file belongs to this pass.
    let other = f._temp.path().join("project-b");
    fs::create_dir(&other).unwrap();
    let mut records = f.records("other-project");
    records[0]["payload"]["cwd"] = json!(other);
    records[1]["payload"]["cwd"] = json!(other);
    let other_path = f.write("sessions/2026/09/26/rollout-other.jsonl", &jsonl(&records));
    f.success(&[
        "miner",
        "collect",
        "--project-root",
        other.to_str().unwrap(),
        "--transcript",
        other_path.to_str().unwrap(),
    ]);
    f.write(
        "sessions/2026/09/26/rollout-unselected.jsonl",
        "not valid JSON",
    );
    fs::remove_file(&other_path).unwrap();
    fs::remove_file(&paths[16]).unwrap();

    let listed = result(&f, &["miner", "sources", "list", "--limit", "16"]);
    assert_eq!(listed["freshness"], "not_checked");
    assert_eq!(listed["sources"].as_array().unwrap().len(), 16);
    let first = result(&f, &["miner", "sync"]);
    assert_eq!(first["collection"]["sources_unchanged"], 16);
    assert_eq!(first["collection"]["candidates_detected"], 0);
    assert_eq!(first["next_cursor"], "session:codex:source-15");
    let cursor = first["next_cursor"].as_str().unwrap();
    let tail = result(&f, &["miner", "sources", "list", "--after", cursor]);
    assert_eq!(tail["sources"].as_array().unwrap().len(), 1);
    assert!(tail["next_cursor"].is_null());
    let missing = f.run(&["miner", "sync", "--after", cursor]);
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("source-16"));
    fs::write(&paths[16], f.preferences("source-16", &[])).unwrap();
    let last = result(&f, &["miner", "sync", "--after", cursor]);
    assert_eq!(last["sources_selected"], 1);
    assert!(last["next_cursor"].is_null());

    // Keyset cursors are not persistent mining cursors. Appends and new IDs
    // sorting before the cursor require a new pass from the beginning.
    let earlier = source(&f, "before-source", &[]);
    assert!(f.collect(&[&earlier], false).status.success());
    fs::write(
        &earlier,
        f.preferences("before-source", &["I prefer short replies."]),
    )
    .unwrap();
    fs::write(
        &paths[0],
        f.preferences("source-00", &["I usually review the code."]),
    )
    .unwrap();
    assert_eq!(
        result(&f, &["miner", "sync", "--after", cursor])["collection"]["candidates_detected"],
        0
    );
    let restarted = result(&f, &["miner", "sync"]);
    assert_eq!(restarted["collection"]["sources_updated"], 2);
    assert_eq!(restarted["collection"]["candidates_detected"], 2);
    assert_eq!(restarted["source_ids"][0], "session:codex:before-source");
    assert!(!f.home.join(".mastermind/style.md").exists());
}

#[test]
fn source_sync_rereads_after_preview_and_keeps_curation_and_publication_separate() {
    let f = Fixture::new();
    let path = source(&f, "source-a", &["I prefer short replies."]);
    assert!(f.collect(&[&path], false).status.success());
    let candidate = f.inbox()["candidates"][0]["candidate"].clone();
    let proposal = f.propose_preference(&candidate, "Prefer short replies", None);
    assert!(proposal.status.success(), "{proposal:?}");
    let style = f.style();
    let revision = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .aggregate()
        .unwrap()
        .profile_revision();
    let before = fs::read(f.db_path()).unwrap();
    fs::write(
        &path,
        f.preferences(
            "source-a",
            &[
                "I prefer short replies.",
                "I usually review code before changing it.",
            ],
        ),
    )
    .unwrap();
    let preview = result(&f, &["miner", "sync", "--dry-run"]);
    assert_eq!(preview["candidates"].as_array().unwrap().len(), 2);
    assert_eq!(fs::read(f.db_path()).unwrap(), before);
    fs::write(
        &path,
        f.preferences(
            "source-a",
            &[
                "I prefer short replies.",
                "I usually review tests before changing them.",
            ],
        ),
    )
    .unwrap();
    assert_eq!(
        result(&f, &["miner", "sync"])["collection"]["sources_updated"],
        1
    );
    let inbox = f.inbox();
    assert_eq!(inbox["candidates"].as_array().unwrap().len(), 2);
    assert!(inbox["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["candidate"]["quote"] == "I usually review tests before changing them."));
    let unchanged = result(&f, &["miner", "sync"]);
    assert_eq!(unchanged["collection"]["sources_unchanged"], 1);
    let empty_preview = result(&f, &["miner", "sync", "--dry-run"]);
    assert_eq!(empty_preview["candidates"], json!([]));
    assert_eq!(f.style(), style);
    assert_eq!(
        ProfileStore::open_read_only(&f.db_path())
            .unwrap()
            .aggregate()
            .unwrap()
            .profile_revision(),
        revision
    );
    let shown = result(
        &f,
        &[
            "miner",
            "candidates",
            "show",
            candidate["id"].as_str().unwrap(),
        ],
    );
    assert_eq!(
        shown["history"]["preference_proposals"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn source_sync_identity_checks_precede_dedup_and_abort_the_whole_page() {
    let f = Fixture::new();
    let a = source(&f, "source-a", &["I prefer short replies."]);
    let b = source(&f, "source-b", &["I prefer brief replies."]);
    assert!(f.collect(&[&a, &b], false).status.success());
    let before = fs::read(f.db_path()).unwrap();
    let original_a = fs::read(&a).unwrap();
    let original_b = fs::read(&b).unwrap();
    fs::write(&a, f.preferences("source-a", &["I usually review code."])).unwrap();
    for replacement in [
        // Two selected files now report the same source; dedup must not hide it.
        fs::read(&a).unwrap(),
        f.preferences("source-c", &["I prefer detailed replies."])
            .into_bytes(),
        b"invalid JSON".to_vec(),
    ] {
        fs::write(&b, replacement).unwrap();
        for args in [vec!["miner", "sync"], vec!["miner", "sync", "--dry-run"]] {
            let failed = f.run(&args);
            assert!(!failed.status.success());
            assert!(failed.stdout.is_empty());
            assert!(String::from_utf8_lossy(&failed.stderr).contains("source-b"));
            assert_eq!(fs::read(f.db_path()).unwrap(), before);
        }
    }
    fs::write(&a, &original_b).unwrap();
    fs::write(&b, &original_a).unwrap();
    assert!(!f.run(&["miner", "sync"]).status.success());
    assert_eq!(fs::read(f.db_path()).unwrap(), before);
    fs::write(&a, original_a).unwrap();
    fs::write(&b, original_b).unwrap();
    assert_eq!(
        result(&f, &["miner", "sync"])["collection"]["sources_unchanged"],
        2
    );
}

#[test]
fn source_sync_missing_sources_need_explicit_relocation_and_project_changes_fail() {
    let f = Fixture::new();
    let path = source(&f, "source-a", &["I prefer short replies."]);
    assert!(f.collect(&[&path], false).status.success());
    let original = f.inbox()["candidates"][0]["candidate"].clone();
    let archive = f.write(
        "archived_sessions/rollout-archive.jsonl",
        &fs::read_to_string(&path).unwrap(),
    );
    fs::remove_file(&path).unwrap();
    let before = fs::read(f.db_path()).unwrap();
    assert_eq!(sources(&f)["freshness"], "not_checked");
    assert!(!f.run(&["miner", "sync"]).status.success());
    assert_eq!(fs::read(f.db_path()).unwrap(), before);
    assert!(f.collect(&[&archive], false).status.success());
    assert_eq!(
        result(&f, &["miner", "sync"])["collection"]["sources_unchanged"],
        1
    );
    let relocated = f.inbox()["candidates"][0]["candidate"].clone();
    assert_eq!(relocated["id"], original["id"]);
    assert_ne!(relocated["revision"], original["revision"]);
    assert_eq!(
        sources(&f)["sources"][0]["source_path"],
        archive.canonicalize().unwrap().to_str().unwrap()
    );
    let db = Connection::open(f.db_path()).unwrap();
    db.execute(
        "UPDATE persona_collection_source SET project = 'different-identity'",
        [],
    )
    .unwrap();
    drop(db);
    let before = fs::read(f.db_path()).unwrap();
    let failure = f.run(&["miner", "sync"]);
    assert!(!failure.status.success());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("cannot change project"));
    assert_eq!(fs::read(f.db_path()).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn source_sync_does_not_follow_a_retargeted_path_even_for_the_same_session() {
    let f = Fixture::new();
    let path = source(&f, "source-a", &["I prefer short replies."]);
    assert!(f.collect(&[&path], false).status.success());
    let copy = f.write(
        "archived_sessions/rollout-copy.jsonl",
        &fs::read_to_string(&path).unwrap(),
    );
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&copy, &path).unwrap();
    let before = fs::read(f.db_path()).unwrap();
    assert!(!f.run(&["miner", "sync"]).status.success());
    assert_eq!(fs::read(f.db_path()).unwrap(), before);
    // A root alias still selects the same canonical project, without changing
    // the stored source's locator.
    let root_alias = f._temp.path().join("project-alias");
    std::os::unix::fs::symlink(&f.project, &root_alias).unwrap();
    assert_eq!(
        result(
            &f,
            &[
                "miner",
                "sources",
                "list",
                "--project-root",
                root_alias.to_str().unwrap()
            ]
        )["sources"],
        sources(&f)["sources"]
    );
}

#[test]
fn source_sync_rolls_back_sql_failure_in_a_later_source() {
    let f = Fixture::new();
    let a = source(&f, "source-a", &["I prefer short replies."]);
    let b = source(&f, "source-b", &["I prefer brief replies."]);
    assert!(f.collect(&[&a, &b], false).status.success());
    let db = Connection::open(f.db_path()).unwrap();
    db.execute_batch("CREATE TRIGGER reject_sync BEFORE UPDATE ON persona_collection_source WHEN NEW.source = 'session:codex:source-b' BEGIN SELECT RAISE(FAIL, 'fixture failure'); END").unwrap();
    fs::write(&a, f.preferences("source-a", &["I usually review code."])).unwrap();
    fs::write(&b, f.preferences("source-b", &["I usually review tests."])).unwrap();
    let before = fs::read(f.db_path()).unwrap();
    let failed = f.run(&["miner", "sync"]);
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
    assert_eq!(fs::read(f.db_path()).unwrap(), before);
    db.execute_batch("DROP TRIGGER reject_sync").unwrap();
    assert_eq!(
        result(&f, &["miner", "sync"])["collection"]["sources_updated"],
        2
    );
    assert_eq!(
        result(&f, &["miner", "sync"])["collection"]["sources_unchanged"],
        2
    );
}

#[test]
fn source_sync_preserves_dismissal_through_removal_reappearance_and_extractor_upgrade() {
    let f = Fixture::new();
    let path = source(&f, "source-a", &["I prefer short replies."]);
    assert!(f.collect(&[&path], false).status.success());
    let original = f.inbox()["candidates"][0]["candidate"].clone();
    let db = Connection::open(f.db_path()).unwrap();
    db.execute("UPDATE persona_candidate SET status = 'dismissed'", [])
        .unwrap();
    fs::write(&path, f.preferences("source-a", &[])).unwrap();
    result(&f, &["miner", "sync"]);
    let removed = f.inbox();
    assert_eq!(removed["candidates"][0]["freshness"], "removed");
    assert_eq!(removed["candidates"][0]["candidate"]["status"], "dismissed");
    fs::write(
        &path,
        f.preferences("source-a", &["I prefer short replies."]),
    )
    .unwrap();
    result(&f, &["miner", "sync"]);
    db.execute(
        "UPDATE persona_collection_source SET extractor = 'old-extractor'",
        [],
    )
    .unwrap();
    let before = fs::read(f.db_path()).unwrap();
    let preview = result(&f, &["miner", "sync", "--dry-run"]);
    assert_eq!(preview["candidates"][0]["status"], "dismissed");
    assert_eq!(fs::read(f.db_path()).unwrap(), before);
    assert_eq!(
        result(&f, &["miner", "sync"])["collection"]["sources_updated"],
        1
    );
    let latest = f.inbox();
    assert_eq!(latest["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(latest["candidates"][0]["candidate"]["id"], original["id"]);
    assert_eq!(latest["candidates"][0]["candidate"]["status"], "dismissed");
    assert_eq!(latest["candidates"][0]["freshness"], "current");
}

#[test]
fn source_sync_uses_the_shared_candidate_budget_and_recovers_with_smaller_pages() {
    let f = Fixture::new();
    let a = source(&f, "source-a", &[]);
    let b = source(&f, "source-b", &[]);
    assert!(f.collect(&[&a, &b], false).status.success());
    let quotes: Vec<_> = (0..300)
        .map(|i| format!("I prefer short replies with example {i}."))
        .collect();
    let quotes: Vec<_> = quotes.iter().map(String::as_str).collect();
    fs::write(&a, f.preferences("source-a", &quotes)).unwrap();
    fs::write(&b, f.preferences("source-b", &quotes)).unwrap();
    let before = fs::read(f.db_path()).unwrap();
    let failure = f.run(&["miner", "sync"]);
    assert!(!failure.status.success());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("512 candidates"));
    assert_eq!(fs::read(f.db_path()).unwrap(), before);
    let first = result(&f, &["miner", "sync", "--limit", "1"]);
    assert_eq!(first["collection"]["candidates_detected"], 300);
    let last = result(
        &f,
        &[
            "miner",
            "sync",
            "--limit",
            "1",
            "--after",
            first["next_cursor"].as_str().unwrap(),
        ],
    );
    assert_eq!(last["collection"]["candidates_detected"], 300);
    assert!(last["next_cursor"].is_null());
    assert!(!f.home.join(".mastermind/style.md").exists());
}

#[test]
fn source_sync_finds_late_codex_context_and_explicitly_selected_claude_sources() {
    let f = Fixture::new();
    let mut records = f.records("late-context");
    records[2]["payload"]["content"][0]["text"] = json!("I prefer short replies.");
    let context = records.remove(1);
    let path = f.write("sessions/2026/09/26/rollout-late.jsonl", &jsonl(&records));
    assert!(f.collect(&[&path], false).status.success());
    let claude = f
        .home
        .join(".claude/projects")
        .join(claude_project_slug(&f.project.canonicalize().unwrap()))
        .join("claude-source.jsonl");
    fs::create_dir_all(claude.parent().unwrap()).unwrap();
    let mut turn = json!({"type":"user", "sessionId":"claude-source", "cwd":f.project,
        "origin":{"kind":"human"}, "message":{"content":"I prefer brief replies."}});
    fs::write(&claude, format!("{turn}\n")).unwrap();
    assert!(f.collect(&[&claude], false).status.success());
    records.push(context);
    fs::write(&path, jsonl(&records)).unwrap();
    turn["message"]["content"] = json!("I usually review the code before changing it.");
    fs::write(&claude, format!("{turn}\n")).unwrap();
    let synced = result(&f, &["miner", "sync"]);
    assert_eq!(synced["collection"]["sources_updated"], 2);
    assert_eq!(synced["collection"]["candidates_detected"], 2);
    assert_eq!(
        synced["source_ids"],
        json!(["session:claude-source", "session:codex:late-context"])
    );
}
