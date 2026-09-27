use super::*;
use crate::miner::{
    profile::publish_profile,
    store::{fixture_habit, NewHabit, NewHabitEvidence},
};

fn stored(db: &ProfileStore, id: i64) -> Habit {
    db.habit(id).unwrap().unwrap()
}
fn quote<'a>(source: &'a str, episode: &'a str) -> NewHabitEvidence<'a> {
    NewHabitEvidence {
        source,
        episode,
        source_path: "new.jsonl",
        line_no: 1,
        record_digest: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        project: "fixture",
        repository: "fixture-origin",
        quote: "Checks the contract before changing code",
        at: "2026-09-26",
        relation: "supports",
    }
}
fn claim(h: &Habit) -> NewHabit<'_> {
    NewHabit {
        when: &h.when,
        behavior: &h.behavior,
        outcome: &h.outcome,
        exception: &h.exception,
        scope: &h.scope,
        role: &h.role,
        workflow: &h.workflow,
    }
}
fn reject(db: &mut ProfileStore, id: i64) {
    db.review_habit(id, "rejected", None).unwrap().unwrap();
}
fn support(db: &mut ProfileStore, id: i64) {
    db.add_habit_evidence(id, &quote("session:new-a", "new-task-a"))
        .unwrap();
    db.add_habit_evidence(id, &quote("session:new-b", "new-task-b"))
        .unwrap();
}
fn observe(db: &mut ProfileStore, id: i64) {
    let revision = stored(db, id).review_revision();
    db.review_habit(id, "observed", Some(&revision))
        .unwrap()
        .unwrap();
}
fn count(db: &ProfileStore, table: &str) -> i64 {
    db.conn
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn returning_to_identical_description_creates_empty_candidate_and_keeps_current_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let a = fixture_habit(&mut db, "Checks every contract");
    let b = fixture_habit(&mut db, "Checks changed contracts");
    db.supersede_habit(a.id, b.id, &a.review_revision(), &b.review_revision())
        .unwrap();
    let b_before = stored(&db, b.id);
    let receipt = db.renew_habit(a.id, &a.review_revision()).unwrap();
    let next = stored(&db, receipt.child_id);
    assert_eq!(next.generation, 2);
    assert_eq!(next.generation_root, a.id);
    assert_eq!(next.renewed_from, Some(a.id));
    assert_ne!(next.id, a.id);
    assert_eq!(next.behavior, a.behavior);
    assert_eq!(next.when, a.when);
    assert_eq!(next.status, "candidate");
    assert_eq!(
        (
            next.sources,
            next.episodes,
            next.repositories,
            next.contradictions,
            next.limitations
        ),
        (0, 0, 0, 0, 0)
    );
    assert!(next.observed_revision.is_none());
    assert!(db.habit_evidence(next.id).unwrap().is_empty());
    assert!(db.habit_review_history(next.id).unwrap()["items"][0]["reviewed_revision"].is_null());
    assert_eq!(stored(&db, b.id), b_before);
    assert_eq!(stored(&db, a.id).review_revision(), a.review_revision());
    assert!(db
        .supersede_habit(b.id, next.id, &b.review_revision(), &next.review_revision())
        .is_err());
    support(&mut db, next.id);
    let next = stored(&db, next.id);
    db.supersede_habit(b.id, next.id, &b.review_revision(), &next.review_revision())
        .unwrap();
    assert_eq!(stored(&db, a.id).status, "superseded");
    assert_eq!(stored(&db, b.id).status, "superseded");
    assert_eq!(stored(&db, next.id).status, "observed");
    assert!(db
        .record_habit(&claim(&a), &quote("session:another", "task-c"))
        .is_err());
    assert_eq!(db.habit_evidence(next.id).unwrap().len(), 2);
}

#[test]
fn exact_retry_preserves_child_state_even_after_parent_revision_changes() {
    for state in [
        "candidate",
        "observed",
        "rejected",
        "superseded",
        "rebound",
        "dismissed",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let old = fixture_habit(&mut db, "Checks every contract");
        reject(&mut db, old.id);
        let receipt = db.renew_habit(old.id, &old.review_revision()).unwrap();
        let child = receipt.child_id;
        if state != "candidate" {
            support(&mut db, child);
            observe(&mut db, child);
        }
        match state {
            "rejected" => reject(&mut db, child),
            "superseded" => {
                let next = fixture_habit(&mut db, "Checks contracts at delivery");
                db.supersede_habit(
                    child,
                    next.id,
                    &stored(&db, child).review_revision(),
                    &next.review_revision(),
                )
                .unwrap();
            }
            "rebound" => {
                let mut moved = quote("session:new-a", "new-task-a");
                moved.source_path = "moved.jsonl";
                db.add_habit_evidence(child, &moved).unwrap();
            }
            "dismissed" => {
                let evidence = db.habit_evidence(child).unwrap()[0].id;
                db.dismiss_habit_evidence(child, evidence).unwrap();
            }
            _ => {}
        }
        let citation = db.habit_evidence(old.id).unwrap()[0].id;
        db.dismiss_habit_evidence(old.id, citation).unwrap();
        let before = db.aggregate().unwrap().profile_revision();
        let events = count(&db, "persona_review_event");
        let snapshot = stored(&db, child);
        let repeated = db.renew_habit(old.id, &old.review_revision()).unwrap();
        assert!(repeated.repeated);
        assert_eq!(repeated.child_id, child);
        assert_eq!(repeated.child_status, snapshot.status);
        assert_eq!(stored(&db, child), snapshot);
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
        assert_eq!(events, count(&db, "persona_review_event"));
        assert!(db
            .renew_habit(old.id, &stored(&db, old.id).review_revision())
            .is_err());
    }
}

#[test]
fn renewal_requires_exact_revision_and_terminal_parent_without_partial_writes() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let old = fixture_habit(&mut db, "Checks every contract");
    for status in ["candidate", "observed", "stale"] {
        db.conn
            .execute(
                "UPDATE persona_claim SET status=?2 WHERE id=?1",
                params![old.id, status],
            )
            .unwrap();
        assert!(db.renew_habit(old.id, &old.review_revision()).is_err());
    }
    reject(&mut db, old.id);
    let before = db.aggregate().unwrap().profile_revision();
    for (id, revision) in [
        (0, old.review_revision()),
        (999, old.review_revision()),
        (old.id, "short".into()),
        (old.id, "f".repeat(64)),
    ] {
        assert!(db.renew_habit(id, &revision).is_err());
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
        assert_eq!(count(&db, "persona_claim"), 1);
        assert_eq!(count(&db, "persona_habit_generation"), 0);
    }
}

#[test]
fn renewed_rejected_parent_stays_terminal_after_older_writer_restores_status_and_pin() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let old = fixture_habit(&mut db, "Checks every contract");
    let other = fixture_habit(&mut db, "Checks contracts at delivery");
    reject(&mut db, old.id);
    db.renew_habit(old.id, &old.review_revision()).unwrap();
    db.conn
        .execute(
            "UPDATE persona_claim SET status='observed' WHERE id=?1",
            [old.id],
        )
        .unwrap();
    db.conn
        .execute(
            "INSERT INTO persona_habit_observation VALUES(?1,?2)",
            params![old.id, old.review_revision()],
        )
        .unwrap();
    let before = db.aggregate().unwrap().profile_revision();
    assert_eq!(stored(&db, old.id).retired_status(), Some("rejected"));
    assert!(db
        .review_habit(old.id, "observed", Some(&old.review_revision()))
        .unwrap()
        .is_err());
    assert!(db.review_habit(old.id, "rejected", None).unwrap().is_err());
    assert!(db
        .add_habit_evidence(old.id, &quote("session:c", "task-c"))
        .is_err());
    assert!(db
        .record_habit(&claim(&old), &quote("session:c", "task-c"))
        .is_err());
    assert!(db
        .supersede_habit(
            old.id,
            other.id,
            &old.review_revision(),
            &other.review_revision()
        )
        .is_err());
    assert!(db
        .supersede_habit(
            other.id,
            old.id,
            &other.review_revision(),
            &old.review_revision()
        )
        .is_err());
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    let citation = db.habit_evidence(old.id).unwrap()[0].id;
    db.dismiss_habit_evidence(old.id, citation).unwrap();
    assert_eq!(stored(&db, old.id).status, "rejected");
    assert!(stored(&db, old.id).observed_revision.is_none());
}

#[test]
fn definition_matching_for_target_is_exact_and_default_proposals_stay_generation_one() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let old = fixture_habit(&mut db, "Checks every contract");
    reject(&mut db, old.id);
    let child = db
        .renew_habit(old.id, &old.review_revision())
        .unwrap()
        .child_id;
    let fresh = stored(&db, child);
    for field in [
        "when",
        "behavior",
        "outcome",
        "exception",
        "scope",
        "role",
        "workflow",
    ] {
        let mut wrong = fresh.clone();
        match field {
            "when" => wrong.when.push_str(" changed"),
            "behavior" => wrong.behavior.push_str(" changed"),
            "outcome" => wrong.outcome.push_str(" changed"),
            "exception" => wrong.exception.push_str("changed"),
            "scope" => wrong.scope = "global".into(),
            "role" => wrong.role = "auditor".into(),
            "workflow" => wrong.workflow = "strict".into(),
            _ => unreachable!(),
        }
        let tx = db.conn.transaction().unwrap();
        assert!(
            super::super::record_habit_tx(
                &tx,
                &claim(&wrong),
                &quote("session:c", "task-c"),
                true,
                Some(child)
            )
            .is_err(),
            "{field}"
        );
    }
    assert!(db.habit_evidence(child).unwrap().is_empty());
    assert!(db
        .record_habit(&claim(&old), &quote("session:c", "task-c"))
        .is_err());
    let tx = db.conn.transaction().unwrap();
    assert_eq!(
        super::super::record_habit_tx(
            &tx,
            &claim(&fresh),
            &quote("session:c", "task-c"),
            true,
            Some(child)
        )
        .unwrap(),
        child
    );
    tx.commit().unwrap();
    support(&mut db, child);
    observe(&mut db, child);
    let tx = db.conn.transaction().unwrap();
    assert!(super::super::record_habit_tx(
        &tx,
        &claim(&fresh),
        &quote("session:d", "task-d"),
        true,
        Some(child)
    )
    .is_err());
}

#[test]
fn concurrent_renewals_create_one_child_and_one_pair_of_events() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("style.db");
    let mut db = ProfileStore::open(&path).unwrap();
    let old = fixture_habit(&mut db, "Checks every contract");
    reject(&mut db, old.id);
    let barrier = std::sync::Barrier::new(2);
    let receipts = std::thread::scope(|scope| {
        let tasks: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    let mut db = ProfileStore::open(&path).unwrap();
                    barrier.wait();
                    db.renew_habit(old.id, &old.review_revision()).unwrap()
                })
            })
            .collect();
        tasks
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(receipts[0].child_id, receipts[1].child_id);
    assert_ne!(receipts[0].repeated, receipts[1].repeated);
    assert_eq!(count(&db, "persona_claim"), 2);
    assert_eq!(count(&db, "persona_habit_generation"), 1);
    assert_eq!(count(&db, "persona_review_event"), 3);
}

#[test]
fn failed_event_and_store_limit_roll_back_child_lineage_and_parent_changes() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let old = fixture_habit(&mut db, "Checks every contract");
    reject(&mut db, old.id);
    let before = db.aggregate().unwrap().profile_revision();
    db.conn
        .execute_batch(
            "CREATE TRIGGER fail_renewal BEFORE INSERT ON persona_review_event
        WHEN NEW.status LIKE 'renewed_from:%' BEGIN SELECT RAISE(ABORT,'fixture failure'); END;",
        )
        .unwrap();
    assert!(db.renew_habit(old.id, &old.review_revision()).is_err());
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    assert_eq!(count(&db, "persona_review_event"), 1);
    assert_eq!(count(&db, "persona_habit_generation"), 0);
    db.conn
        .execute_batch(
            "DROP TRIGGER fail_renewal; CREATE TABLE fixture_padding(body BLOB);
        CREATE TRIGGER grow_generation AFTER INSERT ON persona_habit_generation
        BEGIN INSERT INTO fixture_padding VALUES(zeroblob(67108864)); END;",
        )
        .unwrap();
    assert!(db
        .renew_habit(old.id, &old.review_revision())
        .unwrap_err()
        .to_string()
        .contains("64 MiB"));
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    assert_eq!(count(&db, "persona_claim"), 1);
    assert_eq!(count(&db, "fixture_padding"), 0);
}

#[test]
fn generation_history_bounds_and_reset_prevent_receipts_from_surviving_id_reuse() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let first = fixture_habit(&mut db, "Checks every contract");
    let mut current = first.clone();
    for index in 0..21 {
        reject(&mut db, current.id);
        let receipt = db
            .renew_habit(current.id, &current.review_revision())
            .unwrap();
        current = stored(&db, receipt.child_id);
        assert_eq!(current.generation, index + 2);
        assert_eq!(current.generation_root, first.id);
        let history = db.habit_generations(first.id).unwrap();
        assert_eq!(
            history["items"].as_array().unwrap().len(),
            ((index + 1) as usize).min(20)
        );
        assert_eq!(history["truncated"], index == 20);
    }
    db.reset().unwrap();
    assert_eq!(count(&db, "persona_habit_generation"), 0);
    let again = fixture_habit(&mut db, &first.behavior);
    assert_eq!(again.id, first.id);
    reject(&mut db, again.id);
    assert!(
        !db.renew_habit(again.id, &again.review_revision())
            .unwrap()
            .repeated
    );
}

#[test]
fn generation_count_gate_preserves_previous_state_at_twenty_thousand() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let old = fixture_habit(&mut db, "Checks every contract");
    reject(&mut db, old.id);
    db.conn
        .execute_batch(
            "WITH RECURSIVE n(i) AS (VALUES(1000) UNION ALL SELECT i+1 FROM n WHERE i<20998)
        INSERT INTO persona_habit_generation SELECT i,i+100000,999,'revision','rejected',0 FROM n;",
        )
        .unwrap();
    let receipt = db.renew_habit(old.id, &old.review_revision()).unwrap();
    assert_eq!(count(&db, "persona_habit_generation"), 20000);
    reject(&mut db, receipt.child_id);
    let child = stored(&db, receipt.child_id);
    let before = db.aggregate().unwrap().profile_revision();
    assert!(db
        .renew_habit(child.id, &child.review_revision())
        .unwrap_err()
        .to_string()
        .contains("20000"));
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    assert!(
        db.renew_habit(old.id, &old.review_revision())
            .unwrap()
            .repeated
    );
}

fn downgrade(conn: &Connection) {
    conn.execute_batch(
        "DROP TABLE persona_habit_generation; DROP INDEX persona_claim_unique;
        ALTER TABLE persona_claim DROP COLUMN generation;
        CREATE UNIQUE INDEX persona_claim_unique ON persona_claim
        (kind,when_text,behavior,outcome,exception_text,scope,role,workflow);",
    )
    .unwrap();
}

#[test]
fn migration_preserves_existing_reviews_evidence_and_readonly_legacy_behavior() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("style.db");
    let mut db = ProfileStore::open(&path).unwrap();
    let old = fixture_habit(&mut db, "Checks every contract");
    observe(&mut db, old.id);
    let before = stored(&db, old.id);
    let history = db.habit_review_history(old.id).unwrap();
    let evidence = db.habit_evidence(old.id).unwrap();
    downgrade(&db.conn);
    drop(db);
    let db = ProfileStore::open_read_only(&path).unwrap();
    assert_eq!(stored(&db, old.id), before);
    assert!(db.habit_generations(old.id).unwrap()["items"]
        .as_array()
        .unwrap()
        .is_empty());
    drop(db);
    for _ in 0..2 {
        let db = ProfileStore::open(&path).unwrap();
        assert_eq!(stored(&db, old.id), before);
        assert_eq!(history, db.habit_review_history(old.id).unwrap());
        assert_eq!(evidence, db.habit_evidence(old.id).unwrap());
        assert_eq!(
            stored(&db, old.id).observed_revision,
            Some(old.review_revision())
        );
    }
}

#[test]
fn migration_failure_after_index_swap_restores_legacy_schema_and_pin() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("style.db");
    let mut db = ProfileStore::open(&path).unwrap();
    let old = fixture_habit(&mut db, "Checks every contract");
    observe(&mut db, old.id);
    let before = stored(&db, old.id);
    downgrade(&db.conn);
    db.conn
        .execute_batch("CREATE VIEW persona_habit_generation AS SELECT 1 AS fixture")
        .unwrap();
    assert!(collection::ensure_schema(&mut db.conn).is_err());
    assert!(!db
        .conn
        .prepare("SELECT 1 FROM pragma_table_info('persona_claim') WHERE name='generation'")
        .unwrap()
        .exists([])
        .unwrap());
    assert!(!db
        .conn
        .prepare("SELECT 1 FROM pragma_index_info('persona_claim_unique') WHERE name='generation'")
        .unwrap()
        .exists([])
        .unwrap());
    assert_eq!(stored(&db, old.id), before);
}

#[test]
fn migration_size_gate_rolls_back_column_index_and_lineage_table() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("style.db");
    let mut db = ProfileStore::open(&path).unwrap();
    let old = fixture_habit(&mut db, "Checks every contract");
    downgrade(&db.conn);
    db.conn
        .execute_batch("CREATE TABLE fixture_padding(body BLOB); VACUUM;")
        .unwrap();
    let page_size: i64 = db
        .conn
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .unwrap();
    let pages: i64 = db
        .conn
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    let remaining = super::super::MAX_STYLE_STORE_SIZE as i64 / page_size - pages;
    db.conn
        .execute(
            "INSERT INTO fixture_padding VALUES(zeroblob(?1))",
            [(remaining - 2) * (page_size - 4)],
        )
        .unwrap();
    collection::check_size(&db.conn).unwrap();
    let bytes = std::fs::metadata(&path).unwrap().len();
    assert!(collection::ensure_schema(&mut db.conn)
        .unwrap_err()
        .to_string()
        .contains("64 MiB"));
    drop(db);
    let db = ProfileStore::open_read_only(&path).unwrap();
    assert!(!has_table(&db.conn).unwrap());
    assert_eq!(old, stored(&db, old.id));
    assert!(!db
        .conn
        .prepare("SELECT 1 FROM pragma_table_info('persona_claim') WHERE name='generation'")
        .unwrap()
        .exists([])
        .unwrap());
    assert_eq!(std::fs::metadata(path).unwrap().len(), bytes);
}

#[test]
fn publication_failure_retry_after_child_rejection_never_creates_another_generation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("style.db");
    let markdown = temp.path().join("style.md");
    let mut db = ProfileStore::open(&path).unwrap();
    let old = fixture_habit(&mut db, "Checks every contract");
    reject(&mut db, old.id);
    assert!(publish_profile(
        &path,
        &markdown,
        false,
        |db| {
            let result = db.renew_habit(old.id, &old.review_revision())?;
            std::fs::create_dir(&markdown)?;
            Ok(result)
        },
        |_| None
    )
    .is_err());
    let child = stored(&db, old.id).renewed_as.unwrap();
    reject(&mut db, child);
    let before = db.aggregate().unwrap().profile_revision();
    std::fs::remove_dir(&markdown).unwrap();
    let (receipt, _) = publish_profile(
        &path,
        &markdown,
        false,
        |db| Ok(db.renew_habit(old.id, &old.review_revision())?),
        |_| None,
    )
    .unwrap();
    assert!(receipt.repeated);
    assert_eq!(receipt.child_status, "rejected");
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    assert_eq!(count(&db, "persona_claim"), 2);
    assert!(!std::fs::read_to_string(markdown)
        .unwrap()
        .contains(&old.behavior));
}
