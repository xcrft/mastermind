use super::*;
use crate::miner::profile::publish_profile;
use crate::miner::store::{fixture_habit, NewHabit, NewHabitEvidence};

fn pair(db: &mut ProfileStore) -> (Habit, Habit) {
    (
        fixture_habit(db, "Checks every contract"),
        fixture_habit(db, "Checks changed contracts"),
    )
}

fn replace(db: &mut ProfileStore, old: &Habit, new: &Habit) -> SqlResult<HabitSupersessionReceipt> {
    db.supersede_habit(
        old.id,
        new.id,
        &old.review_revision(),
        &new.review_revision(),
    )
}

fn quote<'a>(source: &'a str, episode: &'a str, relation: &'a str) -> NewHabitEvidence<'a> {
    NewHabitEvidence {
        source,
        episode,
        relation,
        source_path: "fixture.jsonl",
        line_no: 1,
        record_digest: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        project: "fixture",
        repository: "fixture-origin",
        quote: "Checks the contract before changing code",
        at: "2026-09-26",
    }
}

fn stored(db: &ProfileStore, id: i64) -> Habit {
    db.habit(id).unwrap().unwrap()
}

fn count(db: &ProfileStore, table: &str) -> i64 {
    db.conn
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn replacement_pins_successor_preserves_review_digest_and_records_both_sides() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let (old, new) = pair(&mut db);
    db.review_habit(old.id, "observed", Some(&old.review_revision()))
        .unwrap()
        .unwrap();
    let before = db.aggregate().unwrap().profile_revision();
    let receipt = replace(&mut db, &old, &new).unwrap();
    assert!(!receipt.repeated);
    let retired = stored(&db, old.id);
    let current = stored(&db, new.id);
    assert_eq!(retired.status, "superseded");
    assert_eq!(retired.superseded_by, Some(new.id));
    assert_eq!(retired.observed_revision, None);
    assert_eq!(current.status, "observed");
    assert_eq!(current.observed_revision, Some(new.review_revision()));
    assert_eq!(current.review_revision(), new.review_revision());
    assert_ne!(current.relations_revision, new.relations_revision);
    assert_ne!(before, db.aggregate().unwrap().profile_revision());
    for habit in [&old, &new] {
        assert_eq!(
            db.habit_review_history(habit.id).unwrap()["items"][0]["reviewed_revision"],
            habit.review_revision()
        );
        let relation = db.habit_relations(habit.id).unwrap();
        assert_eq!(relation["items"][0]["old_id"], old.id);
        assert_eq!(relation["items"][0]["new_id"], new.id);
        assert_eq!(relation["items"][0]["old_revision"], old.review_revision());
        assert_eq!(relation["items"][0]["new_revision"], new.review_revision());
    }
    let after = db.aggregate().unwrap().profile_revision();
    let events = count(&db, "persona_review_event");
    assert!(replace(&mut db, &old, &new).unwrap().repeated);
    assert_eq!(events, count(&db, "persona_review_event"));
    assert_eq!(after, db.aggregate().unwrap().profile_revision());
}

#[test]
fn competing_edges_cycles_and_retries_never_revive_a_retired_or_rejected_successor() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let (a, b) = pair(&mut db);
    let c = fixture_habit(&mut db, "Checks contracts at delivery");
    replace(&mut db, &a, &b).unwrap();
    let before = db.aggregate().unwrap().profile_revision();
    assert!(replace(&mut db, &a, &c).is_err());
    assert!(replace(&mut db, &b, &a).is_err());
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    replace(&mut db, &b, &c).unwrap();
    let before = db.aggregate().unwrap().profile_revision();
    assert_eq!(replace(&mut db, &a, &b).unwrap().new_status, "superseded");
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    assert!(stored(&db, b.id).observed_revision.is_none());
    db.review_habit(c.id, "rejected", None).unwrap().unwrap();
    let before = db.aggregate().unwrap().profile_revision();
    assert_eq!(replace(&mut db, &b, &c).unwrap().new_status, "rejected");
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    assert!(stored(&db, c.id).observed_revision.is_none());
}

#[test]
fn retry_keeps_rebound_or_dismissed_successor_stale_without_new_review_events() {
    for dismiss in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let (old, new) = pair(&mut db);
        replace(&mut db, &old, &new).unwrap();
        if dismiss {
            let evidence = db.habit_evidence(new.id).unwrap()[0].id;
            db.dismiss_habit_evidence(new.id, evidence).unwrap();
        } else {
            let mut moved = quote("session:a", "task-a", "supports");
            moved.source_path = "moved.jsonl";
            db.add_habit_evidence(new.id, &moved).unwrap();
        }
        let before = db.aggregate().unwrap().profile_revision();
        let events = count(&db, "persona_review_event");
        let retry = replace(&mut db, &old, &new).unwrap();
        assert!(retry.repeated);
        assert_eq!(retry.new_status, "stale");
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
        assert_eq!(events, count(&db, "persona_review_event"));
        assert!(stored(&db, new.id).observed_revision.is_none());
        // A different digest is a different request, even for the same endpoints.
        let changed = stored(&db, new.id);
        assert!(replace(&mut db, &old, &changed).is_err());
    }
}

#[test]
fn replacement_requires_matching_scope_role_workflow_exact_revisions_and_live_endpoints() {
    for mutation in [
        "UPDATE persona_claim SET scope='global' WHERE id=2",
        "UPDATE persona_claim SET role='reviewer' WHERE id=2",
        "UPDATE persona_claim SET workflow='delivery' WHERE id=2",
        "UPDATE persona_claim SET status='rejected' WHERE id=1",
        "UPDATE persona_claim SET status='rejected' WHERE id=2",
        "UPDATE persona_claim SET status='superseded' WHERE id=1",
        "UPDATE persona_claim SET status='superseded' WHERE id=2",
        "DELETE FROM persona_claim_evidence WHERE claim_id=2 AND source='session:b'",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let (a, b) = pair(&mut db);
        db.conn.execute_batch(mutation).unwrap();
        let (a, b) = (stored(&db, a.id), stored(&db, b.id));
        let before = db.aggregate().unwrap().profile_revision();
        assert!(replace(&mut db, &a, &b).is_err(), "{mutation}");
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
        assert_eq!(count(&db, "persona_habit_supersession"), 0);
        assert_eq!(count(&db, "persona_review_event"), 0);
    }
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let (a, b) = pair(&mut db);
    let before = db.aggregate().unwrap().profile_revision();
    for (old, new, old_rev, new_rev) in [
        (a.id, a.id, a.review_revision(), a.review_revision()),
        (0, b.id, a.review_revision(), b.review_revision()),
        (a.id, -1, a.review_revision(), b.review_revision()),
        (a.id, 999, a.review_revision(), b.review_revision()),
        (a.id, b.id, "f".repeat(64), b.review_revision()),
        (a.id, b.id, a.review_revision(), "f".repeat(64)),
        (a.id, b.id, "short".into(), b.review_revision()),
    ] {
        assert!(db.supersede_habit(old, new, &old_rev, &new_rev).is_err());
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
    }
}

#[test]
fn counterevidence_may_retire_old_habit_but_blocks_observing_successor() {
    for relation in ["contradicts", "limits"] {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let (old, new) = pair(&mut db);
        db.add_habit_evidence(old.id, &quote("session:c", "task-c", relation))
            .unwrap();
        let old = stored(&db, old.id);
        // Old is a candidate with unresolved counterevidence; retirement is allowed.
        replace(&mut db, &old, &new).unwrap();
        let next = fixture_habit(&mut db, "Checks contracts after discussing changes");
        db.add_habit_evidence(next.id, &quote("session:c", "task-c", relation))
            .unwrap();
        let next = stored(&db, next.id);
        let before = db.aggregate().unwrap().profile_revision();
        assert!(replace(&mut db, &new, &next).is_err());
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
    }
}

#[test]
fn sql_rechecks_evidence_changes_after_review() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let (old, new) = pair(&mut db);
    db.add_habit_evidence(new.id, &quote("session:c", "task-c", "supports"))
        .unwrap();
    let before = db.aggregate().unwrap().profile_revision();
    assert!(replace(&mut db, &old, &new).is_err());
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    assert_eq!(count(&db, "persona_habit_supersession"), 0);
}

#[test]
fn outgoing_relation_blocks_all_reactivation_even_after_an_older_writer_resets_status() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let (old, new) = pair(&mut db);
    replace(&mut db, &old, &new).unwrap();
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
    assert!(db
        .review_habit(old.id, "observed", Some(&old.review_revision()))
        .unwrap()
        .is_err());
    assert!(db.review_habit(old.id, "rejected", None).unwrap().is_err());
    assert!(db
        .add_habit_evidence(old.id, &quote("session:c", "task-c", "supports"))
        .is_err());
    assert!(db
        .record_habit(
            &NewHabit {
                when: &old.when,
                behavior: &old.behavior,
                outcome: &old.outcome,
                exception: &old.exception,
                scope: &old.scope,
                role: &old.role,
                workflow: &old.workflow,
            },
            &quote("session:c", "task-c", "supports")
        )
        .is_err());
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    let citation = db.habit_evidence(old.id).unwrap()[0].id;
    db.dismiss_habit_evidence(old.id, citation).unwrap();
    assert_eq!(stored(&db, old.id).status, "superseded");
    assert!(stored(&db, old.id).observed_revision.is_none());
}

#[test]
fn fan_in_keeps_successor_review_current_and_history_has_an_explicit_limit() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let new = fixture_habit(&mut db, "Checks changed contracts");
    for i in 0..21 {
        let old = fixture_habit(&mut db, &format!("Previous habit {i}"));
        replace(&mut db, &old, &new).unwrap();
        let current = stored(&db, new.id);
        assert_eq!(current.observed_revision, Some(new.review_revision()));
        assert_eq!(current.review_revision(), new.review_revision());
        let history = db.habit_relations(new.id).unwrap();
        assert_eq!(history["items"].as_array().unwrap().len(), (i + 1).min(20));
        assert_eq!(history["truncated"], i == 20);
    }
}

#[test]
fn failure_on_second_review_event_rolls_back_edges_statuses_pins_and_first_event() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let (old, new) = pair(&mut db);
    db.review_habit(old.id, "observed", Some(&old.review_revision()))
        .unwrap()
        .unwrap();
    let before = db.aggregate().unwrap().profile_revision();
    let history = db.habit_review_history(old.id).unwrap();
    db.conn.execute_batch("CREATE TRIGGER fail_successor BEFORE INSERT ON persona_review_event
        WHEN NEW.status LIKE 'supersedes:%' BEGIN SELECT RAISE(ABORT,'fixture review failure'); END;").unwrap();
    assert!(replace(&mut db, &old, &new).is_err());
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    assert_eq!(history, db.habit_review_history(old.id).unwrap());
    assert_eq!(count(&db, "persona_habit_supersession"), 0);
    assert_eq!(count(&db, "persona_review_event"), 1);
}

#[test]
fn publication_failure_retry_after_rejection_publishes_current_state_only() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("style.db");
    let markdown = temp.path().join("style.md");
    let mut db = ProfileStore::open(&path).unwrap();
    let (old, new) = pair(&mut db);
    assert!(publish_profile(
        &path,
        &markdown,
        false,
        |db| {
            let receipt = replace(db, &old, &new)?;
            std::fs::create_dir(&markdown)?;
            Ok(receipt)
        },
        |_| None
    )
    .is_err());
    assert_eq!(stored(&db, old.id).status, "superseded");
    assert_eq!(stored(&db, new.id).status, "observed");
    db.review_habit(new.id, "rejected", None).unwrap().unwrap();
    let before = db.aggregate().unwrap().profile_revision();
    let events = count(&db, "persona_review_event");
    std::fs::remove_dir(&markdown).unwrap();
    let (receipt, _) = publish_profile(
        &path,
        &markdown,
        false,
        |db| Ok(replace(db, &old, &new)?),
        |_| None,
    )
    .unwrap();
    assert!(receipt.repeated);
    assert_eq!(receipt.new_status, "rejected");
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    assert_eq!(events, count(&db, "persona_review_event"));
    let text = std::fs::read_to_string(markdown).unwrap();
    assert!(!text.contains(&old.behavior));
    assert!(!text.contains(&new.behavior));
}

#[test]
fn storage_and_relation_count_caps_roll_back_all_changes() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let (old, new) = pair(&mut db);
    let before = db.aggregate().unwrap().profile_revision();
    db.conn
        .execute_batch(
            "CREATE TABLE fixture_padding(data BLOB);
        CREATE TRIGGER grow_relation AFTER INSERT ON persona_habit_supersession
        BEGIN INSERT INTO fixture_padding VALUES(zeroblob(67108864)); END;",
        )
        .unwrap();
    assert!(replace(&mut db, &old, &new)
        .unwrap_err()
        .to_string()
        .contains("64 MiB"));
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    assert_eq!(count(&db, "fixture_padding"), 0);
    db.conn
        .execute_batch(
            "DROP TRIGGER grow_relation;
        WITH RECURSIVE numbers(n) AS (VALUES(1000) UNION ALL SELECT n+1 FROM numbers WHERE n<20998)
        INSERT INTO persona_habit_supersession SELECT n,999,'old','new',0 FROM numbers;",
        )
        .unwrap();
    replace(&mut db, &old, &new).unwrap();
    assert_eq!(count(&db, "persona_habit_supersession"), 20000);
    let next = fixture_habit(&mut db, "Checks all public APIs");
    let before = db.aggregate().unwrap().profile_revision();
    assert!(replace(&mut db, &new, &next)
        .unwrap_err()
        .to_string()
        .contains("20000"));
    assert_eq!(before, db.aggregate().unwrap().profile_revision());
    assert_eq!(count(&db, "persona_habit_supersession"), 20000);
    assert!(replace(&mut db, &old, &new).unwrap().repeated);
}

#[test]
fn reset_clears_receipts_before_ids_are_reused() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
    let (old, new) = pair(&mut db);
    replace(&mut db, &old, &new).unwrap();
    db.reset().unwrap();
    assert_eq!(count(&db, "persona_habit_supersession"), 0);
    let (again, current) = pair(&mut db);
    assert_eq!(again.id, old.id);
    assert_eq!(current.id, new.id);
    assert!(db
        .habit_supersession_retry(
            old.id,
            new.id,
            &old.review_revision(),
            &new.review_revision()
        )
        .unwrap()
        .is_none());
    assert!(!replace(&mut db, &again, &current).unwrap().repeated);
}

/// Build the released schema directly, without using the new migration.
fn legacy_claim_table(conn: &Connection) {
    conn.execute_batch("DROP INDEX persona_claim_unique;
        CREATE TABLE old_claim (
            id INTEGER PRIMARY KEY, kind TEXT NOT NULL CHECK(kind='habit'),
            when_text TEXT NOT NULL, behavior TEXT NOT NULL, outcome TEXT NOT NULL,
            exception_text TEXT NOT NULL, scope TEXT NOT NULL, role TEXT NOT NULL, workflow TEXT NOT NULL,
            status TEXT NOT NULL CHECK(status IN ('candidate','observed','stale','rejected')));
        INSERT INTO old_claim SELECT id,kind,when_text,behavior,outcome,exception_text,scope,role,workflow,status FROM persona_claim;
        DROP TABLE persona_claim;
        ALTER TABLE old_claim RENAME TO persona_claim;
        CREATE UNIQUE INDEX persona_claim_unique ON persona_claim
            (kind,when_text,behavior,outcome,exception_text,scope,role,workflow);
        DROP TABLE persona_habit_supersession;").unwrap();
}

fn dependent_snapshot(conn: &Connection) -> Vec<Vec<Vec<String>>> {
    [
        "persona_claim_evidence",
        "persona_candidate_habit",
        "persona_evidence_rebind",
        "persona_habit_observation",
        "persona_review_event",
    ]
    .iter()
    .map(|table| {
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let columns = stmt.column_count();
        stmt.query_map([], |row| {
            Ok((0..columns)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap()))
                .collect())
        })
        .unwrap()
        .collect::<SqlResult<Vec<_>>>()
        .unwrap()
    })
    .collect()
}

#[test]
fn legacy_schema_migration_preserves_all_statuses_ids_dependents_and_review_revisions() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("style.db");
    let mut db = ProfileStore::open(&path).unwrap();
    for (index, status) in ["candidate", "observed", "stale", "rejected"]
        .iter()
        .enumerate()
    {
        let habit = fixture_habit(&mut db, &format!("Fixture {index}"));
        let mut moved = quote("session:a", "task-a", "supports");
        moved.source_path = "moved.jsonl";
        db.add_habit_evidence(habit.id, &moved).unwrap();
        if *status == "observed" || *status == "rejected" {
            let revision = stored(&db, habit.id).review_revision();
            db.review_habit(habit.id, status, Some(&revision))
                .unwrap()
                .unwrap();
        }
        db.conn
            .execute(
                "UPDATE persona_claim SET status=?2 WHERE id=?1",
                params![habit.id, status],
            )
            .unwrap();
    }
    // A receipt is deliberately opaque to the migration and stays byte-for-byte.
    db.conn.execute_batch("INSERT INTO persona_candidate_habit VALUES('candidate','revision','request',1,1,'{}',123);").unwrap();
    legacy_claim_table(&db.conn);
    db.conn
        .execute_batch(
            "CREATE INDEX fixture_claim_status ON persona_claim(status);
        CREATE TABLE fixture_updates(id INTEGER);
        CREATE TRIGGER fixture_claim_update AFTER UPDATE ON persona_claim
            BEGIN INSERT INTO fixture_updates VALUES(NEW.id); END;",
        )
        .unwrap();
    let before = db.habits().unwrap();
    let dependents = dependent_snapshot(&db.conn);
    drop(db);
    let readonly = ProfileStore::open_read_only(&path).unwrap();
    assert_eq!(before, readonly.habits().unwrap());
    assert_eq!(
        readonly.habit_relations(1).unwrap()["items"],
        serde_json::json!([])
    );
    assert!(!has_table(&readonly.conn).unwrap());
    drop(readonly);
    let db = ProfileStore::open(&path).unwrap();
    assert_eq!(before, db.habits().unwrap());
    assert_eq!(dependents, dependent_snapshot(&db.conn));
    assert_eq!(count(&db, "fixture_updates"), 0);
    assert!(db
        .conn
        .prepare(
            "SELECT 1 FROM pragma_index_list('persona_claim') WHERE name='fixture_claim_status'"
        )
        .unwrap()
        .exists([])
        .unwrap());
    assert!(db.conn.execute("INSERT INTO persona_claim SELECT 99,kind,when_text,behavior,outcome,exception_text,scope,role,workflow,status,generation FROM persona_claim WHERE id=1", []).is_err());
    drop(db);
    let db = ProfileStore::open(&path).unwrap();
    assert_eq!(before, db.habits().unwrap());
    db.conn
        .execute(
            "UPDATE persona_claim SET status='superseded' WHERE id=1",
            [],
        )
        .unwrap();
    assert_eq!(count(&db, "fixture_updates"), 1);
}

#[test]
fn migration_error_rolls_back_original_table_and_new_schema_objects() {
    for obstruction in [
        "CREATE TABLE persona_claim_replacement(id INTEGER)",
        "ALTER TABLE persona_claim ADD COLUMN unfamiliar TEXT",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("style.db");
        let mut db = ProfileStore::open(&path).unwrap();
        let old = fixture_habit(&mut db, "Original habit");
        legacy_claim_table(&db.conn);
        db.conn.execute_batch(obstruction).unwrap();
        drop(db);
        assert!(ProfileStore::open(&path).is_err());
        let db = ProfileStore::open_read_only(&path).unwrap();
        assert_eq!(old, stored(&db, old.id));
        assert!(!has_table(&db.conn).unwrap());
        let schema: String = db
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='persona_claim'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!schema.contains("'superseded'"));
    }
}

#[test]
fn migration_growth_over_size_cap_rolls_back_to_readable_legacy_store() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("style.db");
    let mut db = ProfileStore::open(&path).unwrap();
    fixture_habit(&mut db, "Original habit");
    legacy_claim_table(&db.conn);
    // The legacy table fits, but its transactional copy would exceed the cap.
    // Keep the padding outside the unique index to isolate table-copy growth.
    db.conn
        .execute_batch(
            "UPDATE persona_claim SET status='candidate';
        DROP INDEX persona_claim_unique;
        UPDATE persona_claim SET behavior=CAST(zeroblob(35*1024*1024) AS TEXT);
        VACUUM;",
        )
        .unwrap();
    let old_size = std::fs::metadata(&path).unwrap().len();
    assert!(old_size < super::super::MAX_STYLE_STORE_SIZE);
    // Exercise the schema transaction directly; reopening would recreate the
    // deliberately removed unique index before this migration is reached.
    assert!(collection::ensure_schema(&mut db.conn)
        .unwrap_err()
        .to_string()
        .contains("64 MiB"));
    drop(db);
    let db = ProfileStore::open_read_only(&path).unwrap();
    assert!(!has_table(&db.conn).unwrap());
    let schema: String = db
        .conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='persona_claim'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!schema.contains("'superseded'"));
    let bytes: i64 = db
        .conn
        .query_row(
            "SELECT length(CAST(behavior AS BLOB)) FROM persona_claim",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bytes, 35 * 1024 * 1024);
    assert_eq!(std::fs::metadata(path).unwrap().len(), old_size);
}
