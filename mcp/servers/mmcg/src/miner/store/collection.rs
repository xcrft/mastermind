//! Private, unreviewed observations. These tables never enter Aggregate.

use super::{ProfileStore, SqlResult};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;

mod search;

const MAX_SOURCES: i64 = 2000;
const MAX_CANDIDATES: i64 = 10_000;
const MAX_REVISIONS: i64 = 20_000;

pub(super) fn ensure_schema(conn: &mut Connection) -> SqlResult<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS persona_collection_source (
             source TEXT PRIMARY KEY, source_path TEXT NOT NULL,
             project_root TEXT NOT NULL, project TEXT NOT NULL, repository TEXT NOT NULL,
             snapshot_digest TEXT NOT NULL, extractor TEXT NOT NULL,
             bytes INTEGER NOT NULL, lines INTEGER NOT NULL, collected_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS persona_candidate (
             id TEXT PRIMARY KEY, source TEXT NOT NULL, kind TEXT NOT NULL,
             quote TEXT NOT NULL, source_path TEXT NOT NULL, line_no INTEGER NOT NULL,
             segment_no INTEGER NOT NULL, record_digest TEXT NOT NULL,
             project_root TEXT NOT NULL, project TEXT NOT NULL, observed_at TEXT NOT NULL,
             extractor TEXT NOT NULL, rule_id TEXT NOT NULL, revision TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'dismissed')),
             present INTEGER NOT NULL DEFAULT 1 CHECK(present IN (0, 1))
         );
         CREATE INDEX IF NOT EXISTS persona_candidate_source ON persona_candidate(source);
         CREATE INDEX IF NOT EXISTS persona_collection_project ON persona_collection_source(project_root, source);
         CREATE TABLE IF NOT EXISTS persona_sync_exclusion (
             source TEXT PRIMARY KEY, excluded_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS persona_candidate_revision (
             candidate_id TEXT NOT NULL, revision TEXT NOT NULL,
             source_path TEXT NOT NULL, line_no INTEGER NOT NULL, segment_no INTEGER NOT NULL,
             record_digest TEXT NOT NULL, extractor TEXT NOT NULL, rule_id TEXT NOT NULL,
             observed_at TEXT NOT NULL, collected_at INTEGER NOT NULL,
             PRIMARY KEY(candidate_id, revision)
         );
         CREATE TABLE IF NOT EXISTS persona_candidate_review (
             id INTEGER PRIMARY KEY, candidate_id TEXT NOT NULL, revision TEXT NOT NULL,
             decision TEXT NOT NULL CHECK(decision = 'dismissed'), reviewed_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS persona_candidate_habit (
             candidate_id TEXT NOT NULL, revision TEXT NOT NULL, request_digest TEXT NOT NULL,
             claim_id INTEGER NOT NULL, evidence_id INTEGER NOT NULL,
             candidate_json TEXT NOT NULL, proposed_at INTEGER NOT NULL,
             PRIMARY KEY(candidate_id, revision)
         );
         CREATE INDEX IF NOT EXISTS persona_candidate_habit_evidence ON persona_candidate_habit(evidence_id);
         CREATE TABLE IF NOT EXISTS persona_candidate_feedback (
             candidate_id TEXT NOT NULL, revision TEXT NOT NULL, request_digest TEXT NOT NULL,
             feedback_key TEXT NOT NULL, candidate_json TEXT NOT NULL, proposed_at INTEGER NOT NULL,
             PRIMARY KEY(candidate_id, revision)
         );
         CREATE INDEX IF NOT EXISTS persona_candidate_feedback_key ON persona_candidate_feedback(feedback_key);
         CREATE TABLE IF NOT EXISTS feedback_acceptance (key TEXT PRIMARY KEY, revision TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS persona_habit_observation (claim_id INTEGER PRIMARY KEY, revision TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS persona_habit_supersession (
             old_id INTEGER PRIMARY KEY, new_id INTEGER NOT NULL,
             old_revision TEXT NOT NULL, new_revision TEXT NOT NULL,
             reviewed_at INTEGER NOT NULL, CHECK(old_id != new_id)
         );
         CREATE INDEX IF NOT EXISTS persona_habit_supersession_new ON persona_habit_supersession(new_id);
         CREATE TABLE IF NOT EXISTS persona_feedback_dismissal (
             candidate_id TEXT PRIMARY KEY, feedback_key TEXT NOT NULL, reviewed_revision TEXT NOT NULL,
             dismissed_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS feedback_supersession (
             old_key TEXT PRIMARY KEY, new_key TEXT NOT NULL,
             old_revision TEXT NOT NULL, new_revision TEXT NOT NULL,
             reviewed_at INTEGER NOT NULL, CHECK(old_key != new_key)
         );
         CREATE INDEX IF NOT EXISTS feedback_supersession_new ON feedback_supersession(new_key);",
    )?;
    let has_review_revision = tx.prepare("SELECT 1 FROM pragma_table_info('feedback_review_event') WHERE name='reviewed_revision'")?.exists([])?;
    if !has_review_revision {
        tx.execute_batch("ALTER TABLE feedback_review_event ADD COLUMN reviewed_revision TEXT")?;
    }
    let has_habit_revision = tx.prepare("SELECT 1 FROM pragma_table_info('persona_review_event') WHERE name='reviewed_revision'")?.exists([])?;
    if !has_habit_revision {
        tx.execute_batch("ALTER TABLE persona_review_event ADD COLUMN reviewed_revision TEXT")?;
    }
    super::habit_supersession::migrate_status(&tx)?;
    super::habit_generations::ensure_schema(&tx)?;
    check_size(&tx)?;
    tx.commit()
}

pub(super) fn check_size(conn: &Connection) -> SqlResult<()> {
    let pages: u64 = conn
        .query_row("PRAGMA page_count", [], |row| row.get::<_, u32>(0))
        .map(u64::from)?;
    let size: u64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get::<_, u32>(0))
        .map(u64::from)?;
    if pages.saturating_mul(size) > super::MAX_STYLE_STORE_SIZE {
        return Err(rusqlite::Error::InvalidParameterName(
            "collection would exceed the 64 MiB profile store limit".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub(crate) struct CollectedCandidate {
    pub id: String,
    pub source: String,
    pub kind: String,
    pub quote: String,
    pub source_path: String,
    pub line_no: usize,
    pub segment_no: usize,
    pub record_digest: String,
    pub project_root: String,
    pub project: String,
    pub observed_at: String,
    pub extractor: String,
    pub rule_id: String,
    pub revision: String,
    pub status: String,
    pub present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CollectionSource {
    pub source: String,
    pub source_path: String,
    pub project_root: String,
    pub project: String,
    pub repository: String,
    pub snapshot_digest: String,
    pub extractor: String,
    pub bytes: usize,
    pub lines: usize,
}

pub(crate) struct CollectionBatch {
    pub source: CollectionSource,
    pub candidates: Vec<CollectedCandidate>,
}

#[derive(Serialize)]
pub(crate) struct CollectionSourceSelection {
    #[serde(flatten)]
    pub snapshot: CollectionSource,
    pub sync_enabled: bool,
}

#[derive(Default, Serialize)]
pub(crate) struct CollectionStats {
    pub sources_updated: usize,
    pub sources_unchanged: usize,
    pub candidates_detected: usize,
}

impl ProfileStore {
    pub(crate) fn has_only_collection_data(&self) -> SqlResult<bool> {
        if !self.has_table("persona_collection_source")?
            || !self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM persona_collection_source)",
                [],
                |row| row.get::<_, bool>(0),
            )?
        {
            return Ok(false);
        }
        for table in ["repo", "feedback", "persona_claim"] {
            if self.has_table(table)?
                && self.conn.query_row(
                    &format!("SELECT EXISTS(SELECT 1 FROM {table})"),
                    [],
                    |row| row.get::<_, bool>(0),
                )?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn collection_source(&self, source: &str) -> SqlResult<Option<CollectionSource>> {
        if !self.has_table("persona_collection_source")? {
            return Ok(None);
        }
        self.conn
            .query_row(
                "SELECT source, source_path, project_root, project, repository, snapshot_digest,
                    extractor, bytes, lines FROM persona_collection_source WHERE source = ?1",
                [source],
                source_row,
            )
            .optional()
    }

    pub(crate) fn collection_sources(
        &self,
        project_root: &str,
        after: &str,
        limit: usize,
        include_excluded: bool,
    ) -> SqlResult<Vec<CollectionSourceSelection>> {
        if !self.has_table("persona_collection_source")? {
            return Ok(Vec::new());
        }
        let enabled = if self.has_table("persona_sync_exclusion")? {
            "NOT EXISTS(SELECT 1 FROM persona_sync_exclusion x WHERE x.source=s.source)"
        } else {
            "1"
        };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT source, source_path, project_root, project, repository, snapshot_digest,
                    extractor, bytes, lines, {enabled} FROM persona_collection_source s
             WHERE project_root = ?1 AND source > ?2 AND (?4 OR ({enabled}))
             ORDER BY source LIMIT ?3"
        ))?;
        let rows = stmt.query_map(
            params![project_root, after, limit as i64, include_excluded],
            |row| {
                Ok(CollectionSourceSelection {
                    snapshot: source_row(row)?,
                    sync_enabled: row.get(9)?,
                })
            },
        )?;
        rows.collect()
    }

    /// Changes only future sync selection; retained evidence and explicit
    /// collection are independent. A collect never removes this exclusion.
    pub(crate) fn set_source_sync_enabled(
        &mut self,
        source: &str,
        project_root: &str,
        enabled: bool,
    ) -> SqlResult<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let matches = tx
            .prepare("SELECT 1 FROM persona_collection_source WHERE source=?1 AND project_root=?2")?
            .exists(params![source, project_root])?;
        if !matches {
            return Err(rusqlite::Error::InvalidParameterName(
                "source is not registered for this project root".into(),
            ));
        }
        if enabled {
            tx.execute(
                "DELETE FROM persona_sync_exclusion WHERE source=?1",
                [source],
            )?;
        } else {
            tx.execute("INSERT OR IGNORE INTO persona_sync_exclusion(source,excluded_at) VALUES(?1,unixepoch())", [source])?;
        }
        check_size(&tx)?;
        tx.commit()
    }

    /// Commit complete source snapshots and their current candidate sets as a
    /// batch. Errors leave the previous successful snapshots intact.
    pub(crate) fn collect_candidates(
        &mut self,
        batches: &[CollectionBatch],
    ) -> SqlResult<CollectionStats> {
        let mut stats = CollectionStats::default();
        let mut changed = Vec::new();
        for batch in batches {
            let old = self.collection_source(&batch.source.source)?;
            if old.as_ref().is_some_and(|old| {
                old.project != batch.source.project || old.project_root != batch.source.project_root
            }) {
                return Err(rusqlite::Error::InvalidParameterName(
                    "collection source cannot change project".into(),
                ));
            }
            if old.as_ref() == Some(&batch.source) {
                stats.sources_unchanged += 1;
            } else {
                changed.push(batch);
            }
        }
        let tx = self.conn.transaction()?;
        for batch in changed {
            let s = &batch.source;
            for (value, max) in [
                (&s.source, 192),
                (&s.source_path, 4096),
                (&s.project_root, 4096),
                (&s.project, 128),
                (&s.repository, 128),
                (&s.snapshot_digest, 128),
                (&s.extractor, 64),
            ] {
                if value.len() > max {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "collection source metadata exceeds its bound".into(),
                    ));
                }
            }
            tx.execute(
                "UPDATE persona_candidate SET present = 0 WHERE source = ?1",
                [&s.source],
            )?;
            for c in &batch.candidates {
                for (value, max) in [
                    (&c.id, 128),
                    (&c.quote, 1200),
                    (&c.kind, 64),
                    (&c.observed_at, 64),
                    (&c.record_digest, 128),
                    (&c.rule_id, 64),
                    (&c.revision, 128),
                ] {
                    if value.len() > max {
                        return Err(rusqlite::Error::InvalidParameterName(
                            "candidate metadata exceeds its bound".into(),
                        ));
                    }
                }
                if c.source != s.source
                    || c.source_path != s.source_path
                    || c.project_root != s.project_root
                    || c.project != s.project
                    || c.extractor != s.extractor
                {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "candidate provenance disagrees with its source".into(),
                    ));
                }
                tx.execute(
                    "INSERT INTO persona_candidate
                     (id, source, kind, quote, source_path, line_no, segment_no, record_digest,
                      project_root, project, observed_at, extractor, rule_id, revision)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                     ON CONFLICT(id) DO UPDATE SET source_path=excluded.source_path,
                     line_no=excluded.line_no, segment_no=excluded.segment_no,
                     record_digest=excluded.record_digest, observed_at=excluded.observed_at,
                     extractor=excluded.extractor, rule_id=excluded.rule_id,
                     revision=excluded.revision, present=1",
                    params![
                        c.id,
                        c.source,
                        c.kind,
                        c.quote,
                        c.source_path,
                        c.line_no as i64,
                        c.segment_no as i64,
                        c.record_digest,
                        c.project_root,
                        c.project,
                        c.observed_at,
                        c.extractor,
                        c.rule_id,
                        c.revision
                    ],
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO persona_candidate_revision
                     (candidate_id, revision, source_path, line_no, segment_no, record_digest,
                      extractor, rule_id, observed_at, collected_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, unixepoch())",
                    params![
                        c.id,
                        c.revision,
                        c.source_path,
                        c.line_no as i64,
                        c.segment_no as i64,
                        c.record_digest,
                        c.extractor,
                        c.rule_id,
                        c.observed_at
                    ],
                )?;
            }
            tx.execute(
                "INSERT INTO persona_collection_source
                 (source, source_path, project_root, project, repository, snapshot_digest,
                  extractor, bytes, lines, collected_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, unixepoch())
                 ON CONFLICT(source) DO UPDATE SET source_path=excluded.source_path,
                 repository=excluded.repository, snapshot_digest=excluded.snapshot_digest,
                 extractor=excluded.extractor, bytes=excluded.bytes, lines=excluded.lines,
                 collected_at=excluded.collected_at",
                params![
                    s.source,
                    s.source_path,
                    s.project_root,
                    s.project,
                    s.repository,
                    s.snapshot_digest,
                    s.extractor,
                    s.bytes as i64,
                    s.lines as i64
                ],
            )?;
            stats.sources_updated += 1;
            stats.candidates_detected += batch.candidates.len();
        }
        for (table, limit) in [
            ("persona_collection_source", MAX_SOURCES),
            ("persona_candidate", MAX_CANDIDATES),
            ("persona_candidate_revision", MAX_REVISIONS),
        ] {
            let count: i64 = tx.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })?;
            if count > limit {
                return Err(rusqlite::Error::InvalidParameterName(format!(
                    "collection exceeds {limit} rows in {table}"
                )));
            }
        }
        check_size(&tx)?;
        tx.commit()?;
        Ok(stats)
    }

    pub(crate) fn collected_candidates(
        &self,
        after: &str,
        status: &str,
        limit: usize,
    ) -> SqlResult<Vec<CollectedCandidate>> {
        if !self.has_table("persona_candidate")? {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT id, source, kind, quote, source_path, line_no, segment_no, record_digest,
                    project_root, project, observed_at, extractor, rule_id, revision, status, present
             FROM persona_candidate WHERE id > ?1 AND (?2 = 'all' OR status = ?2) ORDER BY id LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![after, status, limit.min(101) as u32], candidate_row)?;
        rows.collect()
    }

    pub(crate) fn collected_candidate(&self, id: &str) -> SqlResult<Option<CollectedCandidate>> {
        if !self.has_table("persona_candidate")? {
            return Ok(None);
        }
        self.conn.query_row(
            "SELECT id, source, kind, quote, source_path, line_no, segment_no, record_digest,
                    project_root, project, observed_at, extractor, rule_id, revision, status, present
             FROM persona_candidate WHERE id = ?1", [id], candidate_row,
        ).optional()
    }

    pub(crate) fn dismiss_collected_candidate(
        &mut self,
        id: &str,
        revision: &str,
    ) -> SqlResult<bool> {
        let tx = self.conn.transaction()?;
        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM persona_candidate WHERE id = ?1 AND revision = ?2",
                params![id, revision],
                |row| row.get(0),
            )
            .optional()?;
        let Some(status) = status else {
            return Ok(false);
        };
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM persona_candidate_feedback WHERE candidate_id=?1)",
            [id],
            |row| row.get::<_, bool>(0),
        )? {
            return Err(rusqlite::Error::InvalidParameterName(
                "candidate has a preference proposal; reject the feedback instead".into(),
            ));
        }
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM persona_candidate_habit WHERE candidate_id=?1)",
            [id],
            |row| row.get::<_, bool>(0),
        )? {
            return Err(rusqlite::Error::InvalidParameterName(
                "candidate has a habit proposal; dismiss its habit evidence instead".into(),
            ));
        }
        if status != "dismissed" {
            tx.execute(
                "UPDATE persona_candidate SET status='dismissed' WHERE id=?1",
                [id],
            )?;
            tx.execute("INSERT INTO persona_candidate_review (candidate_id, revision, decision, reviewed_at)
                        VALUES (?1, ?2, 'dismissed', unixepoch())", params![id, revision])?;
        }
        check_size(&tx)?;
        tx.commit()?;
        Ok(true)
    }

    pub(crate) fn candidate_history(&self, id: &str) -> SqlResult<serde_json::Value> {
        if !self.has_table("persona_candidate_revision")? {
            return Ok(serde_json::json!({}));
        }
        let mut stmt = self.conn.prepare(
            "SELECT revision, source_path, line_no, segment_no, record_digest, extractor,
                    rule_id, observed_at FROM persona_candidate_revision WHERE candidate_id=?1
             ORDER BY collected_at DESC, revision LIMIT 21",
        )?;
        let mut revisions: Vec<serde_json::Value> = stmt
            .query_map([id], |row| {
                Ok(serde_json::json!({
                    "revision":row.get::<_, String>(0)?, "source_path":row.get::<_, String>(1)?,
                    "line_no":row.get::<_, i64>(2)?, "segment_no":row.get::<_, i64>(3)?,
                    "record_digest":row.get::<_, String>(4)?, "extractor":row.get::<_, String>(5)?,
                    "rule_id":row.get::<_, String>(6)?, "observed_at":row.get::<_, String>(7)?
                }))
            })?
            .collect::<SqlResult<_>>()?;
        let truncated = revisions.len() > 20;
        revisions.truncate(20);
        let mut stmt = self.conn.prepare(
            "SELECT revision, decision, reviewed_at FROM persona_candidate_review
             WHERE candidate_id=?1 ORDER BY id DESC LIMIT 20",
        )?;
        let reviews: Vec<serde_json::Value> = stmt
            .query_map([id], |row| {
                Ok(serde_json::json!({
                    "revision":row.get::<_, String>(0)?, "decision":row.get::<_, String>(1)?,
                    "reviewed_at":row.get::<_, i64>(2)?
                }))
            })?
            .collect::<SqlResult<_>>()?;
        Ok(
            serde_json::json!({"revisions":revisions, "revisions_truncated":truncated, "reviews":reviews,
                "habit_proposals":self.candidate_habit_history(Some(id), None)?,
                "preference_proposals":self.candidate_feedback_history(Some(id), None)?}),
        )
    }
}

fn source_row(row: &rusqlite::Row<'_>) -> SqlResult<CollectionSource> {
    Ok(CollectionSource {
        source: row.get(0)?,
        source_path: row.get(1)?,
        project_root: row.get(2)?,
        project: row.get(3)?,
        repository: row.get(4)?,
        snapshot_digest: row.get(5)?,
        extractor: row.get(6)?,
        bytes: row.get::<_, u32>(7)? as usize,
        lines: row.get::<_, u32>(8)? as usize,
    })
}

pub(super) fn candidate_row(row: &rusqlite::Row<'_>) -> SqlResult<CollectedCandidate> {
    Ok(CollectedCandidate {
        id: row.get(0)?,
        source: row.get(1)?,
        kind: row.get(2)?,
        quote: row.get(3)?,
        source_path: row.get(4)?,
        line_no: row.get::<_, u32>(5)? as usize,
        segment_no: row.get::<_, u32>(6)? as usize,
        record_digest: row.get(7)?,
        project_root: row.get(8)?,
        project: row.get(9)?,
        observed_at: row.get(10)?,
        extractor: row.get(11)?,
        rule_id: row.get(12)?,
        revision: row.get(13)?,
        status: row.get(14)?,
        present: row.get(15)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(source: &str) -> CollectionBatch {
        CollectionBatch {
            source: CollectionSource {
                source: source.into(),
                source_path: "/source".into(),
                project_root: "/project".into(),
                project: "project-a".into(),
                repository: String::new(),
                snapshot_digest: "snapshot-a".into(),
                extractor: "v1".into(),
                bytes: 100,
                lines: 3,
            },
            candidates: vec![CollectedCandidate {
                id: source.into(),
                source: source.into(),
                kind: "possible_stated_preference".into(),
                quote: "I prefer short replies.".into(),
                source_path: "/source".into(),
                line_no: 3,
                segment_no: 0,
                record_digest: "record-a".into(),
                project_root: "/project".into(),
                project: "project-a".into(),
                observed_at: "2026-09-26".into(),
                extractor: "v1".into(),
                rule_id: "preference.en".into(),
                revision: "revision-a".into(),
                status: "pending".into(),
                present: true,
            }],
        }
    }

    #[test]
    fn sync_exclusions_are_scoped_idempotent_private_and_cleared_by_reset() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("style.db");
        let mut db = ProfileStore::open(&path).unwrap();
        let b = batch("source-a");
        db.collect_candidates(std::slice::from_ref(&b)).unwrap();
        let revision = db.aggregate().unwrap().profile_revision();
        for _ in 0..2 {
            db.set_source_sync_enabled("source-a", "/project", false)
                .unwrap();
        }
        assert!(db
            .collection_sources("/project", "", 10, false)
            .unwrap()
            .is_empty());
        assert!(!db.collection_sources("/project", "", 10, true).unwrap()[0].sync_enabled);
        assert!(db
            .set_source_sync_enabled("source-a", "/another", true)
            .is_err());
        assert!(db
            .set_source_sync_enabled("missing", "/project", false)
            .is_err());
        db.collect_candidates(std::slice::from_ref(&b)).unwrap();
        assert!(db
            .collection_sources("/project", "", 10, false)
            .unwrap()
            .is_empty());
        assert_eq!(db.aggregate().unwrap().profile_revision(), revision);
        db.set_source_sync_enabled("source-a", "/project", true)
            .unwrap();
        assert_eq!(
            db.collection_sources("/project", "", 10, false)
                .unwrap()
                .len(),
            1
        );
        db.conn.execute_batch("CREATE TRIGGER reject_exclusion BEFORE INSERT ON persona_sync_exclusion BEGIN SELECT RAISE(FAIL,'fixture'); END").unwrap();
        assert!(db
            .set_source_sync_enabled("source-a", "/project", false)
            .is_err());
        assert!(db.collection_sources("/project", "", 10, true).unwrap()[0].sync_enabled);
        db.conn
            .execute_batch("DROP TRIGGER reject_exclusion")
            .unwrap();
        db.set_source_sync_enabled("source-a", "/project", false)
            .unwrap();
        db.reset().unwrap();
        db.collect_candidates(&[b]).unwrap();
        assert!(db.collection_sources("/project", "", 10, true).unwrap()[0].sync_enabled);
        // A read-only older store has no exclusion policy and is not migrated.
        db.conn
            .execute_batch("DROP TABLE persona_sync_exclusion")
            .unwrap();
        drop(db);
        let original = std::fs::read(&path).unwrap();
        let old = ProfileStore::open_read_only(&path).unwrap();
        assert!(old.collection_sources("/project", "", 10, true).unwrap()[0].sync_enabled);
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn rejection_and_versions_survive_rescan_relocation_and_detector_update() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let mut b = batch("source-a");
        let profile_revision = db.aggregate().unwrap().profile_revision();
        db.collect_candidates(std::slice::from_ref(&b)).unwrap();
        assert_eq!(db.aggregate().unwrap().profile_revision(), profile_revision);
        assert_eq!(
            db.collect_candidates(std::slice::from_ref(&b))
                .unwrap()
                .sources_unchanged,
            1
        );
        assert!(!db
            .dismiss_collected_candidate("source-a", "wrong-revision")
            .unwrap());
        assert!(db
            .dismiss_collected_candidate("source-a", "revision-a")
            .unwrap());
        b.source.source_path = "/archive".into();
        b.source.extractor = "v2".into();
        b.candidates[0].source_path = "/archive".into();
        b.candidates[0].extractor = "v2".into();
        b.candidates[0].revision = "revision-b".into();
        db.collect_candidates(std::slice::from_ref(&b)).unwrap();
        let c = db.collected_candidate("source-a").unwrap().unwrap();
        assert_eq!(c.status, "dismissed");
        assert_eq!(c.revision, "revision-b");
        assert!(!db
            .dismiss_collected_candidate("source-a", "revision-a")
            .unwrap());
        let history = db.candidate_history("source-a").unwrap();
        assert_eq!(history["revisions"].as_array().unwrap().len(), 2);
        assert_eq!(history["reviews"][0]["revision"], "revision-a");
        b.source.snapshot_digest = "rewritten".into();
        b.candidates.clear();
        db.collect_candidates(&[b]).unwrap();
        let removed = db.collected_candidate("source-a").unwrap().unwrap();
        assert!(!removed.present);
        assert_eq!(removed.status, "dismissed");
        db.reset().unwrap();
        assert!(db.collected_candidate("source-a").unwrap().is_none());
        assert!(db.collection_source("source-a").unwrap().is_none());
        assert!(db.candidate_history("source-a").unwrap()["reviews"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn failed_batch_rolls_back_candidates_and_checkpoints_from_earlier_sources() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let mut first = batch("source-a");
        db.collect_candidates(std::slice::from_ref(&first)).unwrap();
        first.source.snapshot_digest = "changed".into();
        first.candidates[0].revision = "changed".into();
        db.conn
            .execute_batch(
                "CREATE TRIGGER reject_second BEFORE INSERT ON persona_collection_source
            WHEN NEW.source = 'source-b' BEGIN SELECT RAISE(FAIL, 'fixture failure'); END;",
            )
            .unwrap();
        assert!(db.collect_candidates(&[first, batch("source-b")]).is_err());
        assert_eq!(
            db.collection_source("source-a")
                .unwrap()
                .unwrap()
                .snapshot_digest,
            "snapshot-a"
        );
        assert_eq!(
            db.collected_candidate("source-a")
                .unwrap()
                .unwrap()
                .revision,
            "revision-a"
        );
        assert!(db.collected_candidate("source-b").unwrap().is_none());
        assert!(db.collection_source("source-b").unwrap().is_none());
    }

    #[test]
    fn source_identity_cannot_be_reassigned_to_a_different_project() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let mut b = batch("source-a");
        db.collect_candidates(std::slice::from_ref(&b)).unwrap();
        b.source.project = "project-b".into();
        assert!(db.collect_candidates(&[b]).is_err());
        assert_eq!(
            db.collection_source("source-a").unwrap().unwrap().project,
            "project-a"
        );
    }

    #[test]
    fn size_cap_rolls_back_growth_and_keeps_the_previous_store_readable() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("style.db");
        let mut db = ProfileStore::open(&path).unwrap();
        db.conn
            .execute_batch(
                "CREATE TABLE fixture_padding (body BLOB);
            INSERT INTO fixture_padding VALUES (zeroblob(60 * 1024 * 1024));",
            )
            .unwrap();
        let revision = db.aggregate().unwrap().profile_revision();
        let mut b = batch("source-a");
        b.source.source_path = "s".repeat(4000);
        b.source.project_root = "p".repeat(4000);
        let mut c = b.candidates.remove(0);
        c.source_path.clone_from(&b.source.source_path);
        c.project_root.clone_from(&b.source.project_root);
        b.candidates = (0..512)
            .map(|i| {
                let mut row = c.clone();
                row.id = format!("candidate-{i}");
                row
            })
            .collect();
        let error = db
            .collect_candidates(&[b])
            .err()
            .expect("growth must be rejected");
        assert!(error.to_string().contains("64 MiB"), "{error}");
        drop(db);
        let db = ProfileStore::open_read_only(&path).unwrap();
        assert_eq!(db.aggregate().unwrap().profile_revision(), revision);
        assert!(db.collected_candidates("", "all", 100).unwrap().is_empty());
        assert!(db.collection_source("source-a").unwrap().is_none());
        assert!(std::fs::metadata(path).unwrap().len() <= super::super::MAX_STYLE_STORE_SIZE);
    }
}
