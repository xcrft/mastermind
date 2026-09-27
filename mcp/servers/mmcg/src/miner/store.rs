//! `~/.mastermind/style.db` — user-global SQLite store that accumulates per-repo
//! style evidence so the author profile enriches across every repo they mine.
//!
//! The codegraph DB (`.mastermind/mmcg.db`) is per-project; this one is global on
//! purpose — "write like me" is one fingerprint summed over all the person's
//! repos. Each mine upserts its repo's contribution (idempotent — re-mining the
//! same repo replaces, never doubles); the rendered profile is the SUM over repos.
//!
//! Evidence is stored per sampled commit, because confidence is counted in
//! commits: lines within one commit share one author decision and often one
//! formatter run, so they are not independent samples. Repositories mined by the
//! older line-level format have no commit rows; they stay visible as legacy until
//! re-mined instead of mixing two sampling units.

use crate::bounded_fs::{BoundedReadError, ReadControl, StableFileIdentity};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Result as SqlResult};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod collection;
mod curation;
mod habit_generations;
mod habit_review;
mod habit_supersession;
mod preferences;
mod supersession;
pub(super) use collection::{
    CollectedCandidate, CollectionBatch, CollectionSource, CollectionSourceSelection,
    CollectionStats,
};
#[cfg(test)]
pub(crate) use habit_review::fixture_habit;
#[cfg(test)]
pub(super) use preferences::fixture_preference;

const MAX_STYLE_STORE_SIZE: u64 = 64 * 1024 * 1024;

/// Raw per-detector tallies, summable across repos. Keys like `indent.space`.
pub type Counts = BTreeMap<String, i64>;

/// `(author, latest_sha, latest_date)` for a stored repo — what staleness needs.
pub type RepoMeta = (String, Option<String>, Option<String>);

/// Distinct author labels and emails already contributing to the one user-global
/// profile. The miner uses this to prevent accidentally combining two people.
pub type OwnerSignals = (Vec<String>, Vec<String>);

/// Per-repo provenance persisted next to its counts.
pub struct RepoProvenance {
    pub author: String,
    pub commits_total: i64,
    pub commits_sampled: i64,
    pub added_lines_sampled: i64,
    pub latest_sha: Option<String>,
    pub latest_date: Option<String>,
    /// Unix seconds when this repo was last mined — drives retention.
    pub mined_at_epoch: i64,
    /// Detector contract that produced the stored commit tallies. A later mine
    /// reuses them only while this matches.
    pub extractor: String,
}

/// A repository's stored commit evidence, reusable by the next incremental mine.
pub struct RepoEvidence {
    pub author: String,
    pub extractor: String,
    pub commits: Vec<CommitEvidence>,
}

/// One sampled authored commit and its detector tallies — the unit of evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitEvidence {
    pub sha: String,
    pub authored_at: String,
    pub counts: Counts,
}

/// A preference the author stated to a coding agent, quoted verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feedback {
    /// Normalised statement; one entry per distinct preference.
    pub key: String,
    pub statement: String,
    pub category: String,
    pub scope: String,
    pub quote: String,
    pub first_at: String,
    pub last_at: String,
    /// Distinct sessions or memory files that stated it.
    pub sources: i64,
    /// `candidate`, accepted `active`, or terminal `rejected` / `superseded`.
    pub status: String,
    /// All retained evidence and immutable candidate receipts, including legacy rows.
    pub evidence_revision: String,
    /// Exact definition/evidence revision explicitly accepted by the author.
    pub accepted_revision: Option<String>,
    /// Replacement review is separate from the review of source evidence.
    pub relations_revision: String,
    pub superseded_by: Option<String>,
}

impl Feedback {
    pub fn review_revision(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"mastermind-preference-review-v1\0");
        for field in [
            &self.key,
            &self.statement,
            &self.category,
            &self.scope,
            &self.evidence_revision,
        ] {
            digest_str(&mut digest, field);
        }
        crate::hex::encode(&digest.finalize())
    }
}

/// A newly observed statement and where it came from.
pub struct NewFeedback<'a> {
    pub statement: &'a str,
    pub category: &'a str,
    pub scope: &'a str,
    pub quote: &'a str,
    pub at: &'a str,
    pub source: &'a str,
}

/// One retained quote supporting a stated preference. A memory-file quote is
/// agent-authored and must remain distinguishable from a human session turn.
pub struct FeedbackEvidence {
    pub source: String,
    pub quote: String,
    pub at: String,
    pub attribution: String,
}

/// A reviewed description of behavior in a situation. It is advisory, never
/// an instruction to the agent. IDs survive edits to the wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Habit {
    pub id: i64,
    pub generation: i64,
    pub generation_root: i64,
    pub renewed_from: Option<i64>,
    pub renewed_as: Option<i64>,
    /// Terminal state retained when this claim became a renewal parent.
    pub(crate) renewal_status: Option<String>,
    pub when: String,
    pub behavior: String,
    pub outcome: String,
    pub exception: String,
    pub scope: String,
    pub role: String,
    pub workflow: String,
    pub status: String,
    pub episodes: i64,
    pub sources: i64,
    pub projects: i64,
    pub repositories: i64,
    pub contradictions: i64,
    pub limitations: i64,
    /// Digest of the cited SQL rows; a relocated source changes the profile revision.
    pub evidence_revision: String,
    /// Exact definition and evidence revision reviewed as observed.
    pub observed_revision: Option<String>,
    /// Reviewed replacement relations, independent of the source review digest.
    pub relations_revision: String,
    pub superseded_by: Option<i64>,
}

impl Habit {
    pub fn review_revision(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"mastermind-habit-review-v1\0");
        hash.update(self.id.to_le_bytes());
        for field in [
            &self.when,
            &self.behavior,
            &self.outcome,
            &self.exception,
            &self.scope,
            &self.role,
            &self.workflow,
            &self.evidence_revision,
        ] {
            digest_str(&mut hash, field);
        }
        // Existing generation-one review pins remain valid after migration.
        if self.generation != 1 {
            hash.update(b"habit-generation-identity-v1\0");
            hash.update(self.generation.to_le_bytes());
            hash.update(self.generation_root.to_le_bytes());
            hash.update(self.renewed_from.unwrap_or(0).to_le_bytes());
        }
        crate::hex::encode(&hash.finalize())
    }

    pub(super) fn retired_status(&self) -> Option<&str> {
        if self.superseded_by.is_some() || self.status == "superseded" {
            Some("superseded")
        } else if self.renewed_as.is_some() {
            Some(self.renewal_status.as_deref().unwrap_or("rejected"))
        } else if self.status == "rejected" {
            Some("rejected")
        } else {
            None
        }
    }

    pub(super) fn observation_problem(&self) -> Option<&'static str> {
        if self.retired_status().is_some()
            || !matches!(self.status.as_str(), "candidate" | "stale" | "observed")
        {
            Some("a rejected or superseded habit cannot be observed")
        } else if self.episodes < 2 || self.sources < 2 {
            Some("observation needs support from two distinct sessions and task episodes")
        } else if !self.scope.starts_with("project:") && self.repositories < 2 {
            Some("a cross-project habit needs support from two distinct Git remotes")
        } else if self.contradictions > 0 || self.limitations > 0 {
            Some("counterevidence or a limiting case is unresolved; the habit cannot be observed")
        } else {
            None
        }
    }
}

/// A cited session turn and its task episode. Different sessions belonging to
/// one task must use the same episode key.
pub struct NewHabitEvidence<'a> {
    pub source: &'a str,
    pub source_path: &'a str,
    pub line_no: i64,
    pub record_digest: &'a str,
    pub episode: &'a str,
    pub project: &'a str,
    pub repository: &'a str,
    pub quote: &'a str,
    pub at: &'a str,
    pub relation: &'a str,
}

pub struct NewHabit<'a> {
    pub when: &'a str,
    pub behavior: &'a str,
    pub outcome: &'a str,
    pub exception: &'a str,
    pub scope: &'a str,
    pub role: &'a str,
    pub workflow: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HabitEvidence {
    pub id: i64,
    pub source: String,
    pub source_path: String,
    pub line_no: i64,
    pub record_digest: String,
    pub episode: String,
    pub project: String,
    pub repository: String,
    pub quote: String,
    pub at: String,
    pub relation: String,
    pub status: String,
}

/// A relocated citation keeps its previous locator for local review.
pub struct HabitEvidenceRebind {
    pub evidence_id: i64,
    pub old_source_path: String,
    pub old_line_no: i64,
    pub old_record_digest: String,
    pub old_at: String,
    pub new_source_path: String,
    pub new_line_no: i64,
    pub new_record_digest: String,
    pub new_at: String,
}

/// Sum across all mined repos — what the rendered profile is built from.
pub struct Aggregate {
    pub repos: usize,
    /// Repositories mined before commit-level evidence. Their line-level tallies
    /// are gone; they contribute nothing until re-mined.
    pub legacy_repos: usize,
    pub commits_total: i64,
    /// Distinct commit OIDs with a measured diff and no conflicting context.
    pub commits_sampled: i64,
    pub added_lines_sampled: i64,
    pub identities: Vec<String>,
    /// Sum of distinct sampled Git commits' tallies.
    pub counts: Counts,
    /// Distinct listed Git commits in SHA order, including explicit conflicts.
    pub commits: Vec<CommitEvidence>,
    /// Stated preferences, ordered by key.
    pub feedback: Vec<Feedback>,
    pub habits: Vec<Habit>,
}

impl Aggregate {
    /// Stable digest of the rendered profile and its cited habit evidence.
    /// Identity rows are deliberately excluded because they are private ownership
    /// evidence and never affect the rendered Markdown.
    pub fn profile_revision(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"mastermind-style-aggregate-v9\0");
        digest.update((self.repos as u64).to_le_bytes());
        digest.update((self.legacy_repos as u64).to_le_bytes());
        digest.update(self.commits_total.to_le_bytes());
        digest.update(self.commits_sampled.to_le_bytes());
        digest.update(self.added_lines_sampled.to_le_bytes());
        for entry in &self.feedback {
            for field in [
                &entry.key,
                &entry.statement,
                &entry.category,
                &entry.scope,
                &entry.quote,
                &entry.first_at,
                &entry.last_at,
                &entry.status,
                &entry.evidence_revision,
                &entry.relations_revision,
                entry.accepted_revision.as_deref().unwrap_or(""),
            ] {
                digest_str(&mut digest, field);
            }
            digest.update(entry.sources.to_le_bytes());
        }
        for habit in &self.habits {
            digest.update(habit.id.to_le_bytes());
            digest.update(habit.generation.to_le_bytes());
            digest.update(habit.generation_root.to_le_bytes());
            digest.update(habit.renewed_from.unwrap_or(0).to_le_bytes());
            for field in [
                &habit.when,
                &habit.behavior,
                &habit.outcome,
                &habit.exception,
                &habit.scope,
                &habit.role,
                &habit.workflow,
                &habit.status,
                &habit.evidence_revision,
                &habit.relations_revision,
                habit.observed_revision.as_deref().unwrap_or(""),
            ] {
                digest_str(&mut digest, field);
            }
            digest.update(habit.episodes.to_le_bytes());
            digest.update(habit.sources.to_le_bytes());
            digest.update(habit.projects.to_le_bytes());
            digest.update(habit.repositories.to_le_bytes());
            digest.update(habit.contradictions.to_le_bytes());
            digest.update(habit.limitations.to_le_bytes());
        }
        for commit in &self.commits {
            digest_str(&mut digest, &commit.sha);
            digest_str(&mut digest, &commit.authored_at);
            digest.update((commit.counts.len() as u64).to_le_bytes());
            for (key, value) in &commit.counts {
                digest_str(&mut digest, key);
                digest.update(value.to_le_bytes());
            }
        }
        crate::hex::encode(&digest.finalize())
    }
}

fn digest_str(digest: &mut Sha256, value: &str) {
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value.as_bytes());
}

/// Repeated copies are one observation. A measured copy dominates a metadata
/// copy; disagreements between equally measured contexts are withheld.
/// Area labels are checkout provenance: union their aliases once per SHA.
/// They never choose, duplicate or replace measurement counters.
fn reconcile_commit(copies: Vec<CommitEvidence>) -> CommitEvidence {
    let measured = copies
        .iter()
        .any(|c| c.counts.get("diff.sampled").copied().unwrap_or(0) > 0);
    let selected: Vec<_> = copies
        .iter()
        .filter(|c| !measured || c.counts.get("diff.sampled").copied().unwrap_or(0) > 0)
        .collect();
    let first = selected[0];
    if selected.iter().all(|c| {
        c.authored_at == first.authored_at
            && c.counts
                .iter()
                .filter(|(key, _)| !key.starts_with("range.area."))
                .eq(first
                    .counts
                    .iter()
                    .filter(|(key, _)| !key.starts_with("range.area.")))
    }) {
        let mut combined = first.clone();
        combined
            .counts
            .retain(|key, _| !key.starts_with("range.area."));
        for copy in &copies {
            for (key, value) in &copy.counts {
                if key.starts_with("range.area.") && *value > 0 {
                    combined.counts.insert(key.clone(), 1);
                }
            }
        }
        return combined;
    }
    CommitEvidence {
        sha: first.sha.clone(),
        authored_at: selected
            .iter()
            .map(|c| &c.authored_at)
            .min()
            .unwrap()
            .clone(),
        counts: Counts::from([("evidence.context_conflict".into(), 1)]),
    }
}

pub struct ProfileStore {
    conn: Connection,
}

impl ProfileStore {
    /// `~/.mastermind/style.db`.
    pub fn db_path() -> Option<PathBuf> {
        std::env::home_dir().map(|h| h.join(".mastermind").join("style.db"))
    }

    pub fn open(path: &Path) -> SqlResult<Self> {
        let (root, target) = crate::bounded_fs::prepare_file_target(path)
            .map_err(|error| sqlite_path_error("prepare style store", error))?;
        let existing_identity = match crate::bounded_fs::read_regular_file_with_capability(
            &root,
            &target,
            MAX_STYLE_STORE_SIZE,
            0,
            ReadControl::default(),
        ) {
            Ok(file) => Some(file.identity),
            Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                crate::bounded_fs::inspect_absent_path(&root, &target, ReadControl::default())
                    .map_err(|error| sqlite_path_error("inspect style store", error))?
                    .ok_or_else(sqlite_snapshot_changed)?;
                None
            }
            Err(error) => return Err(sqlite_path_error("inspect style store", error)),
        };
        let mut created_file = None;
        let expected_identity = match existing_identity {
            Some(identity) => identity,
            None => {
                let (file, identity) =
                    crate::bounded_fs::create_regular_file_with_capability(&root, &target, true)
                        .map_err(|error| sqlite_path_error("create style store", error))?;
                created_file = Some(file);
                identity
            }
        };
        let mut conn = Connection::open_with_flags(
            &target,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        verify_store_identity(
            &root,
            &target,
            expected_identity,
            existing_identity.is_some(),
        )?;
        drop(created_file);
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS repo (
                 repo_key            TEXT PRIMARY KEY,
                 author              TEXT NOT NULL,
                 commits_total       INTEGER NOT NULL,
                 commits_sampled     INTEGER NOT NULL,
                 added_lines_sampled INTEGER NOT NULL,
                 latest_sha          TEXT,
                 latest_date         TEXT,
                 mined_at_epoch      INTEGER NOT NULL DEFAULT 0,
                 extractor           TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE IF NOT EXISTS identity (
                 repo_key TEXT NOT NULL,
                 email    TEXT NOT NULL,
                 PRIMARY KEY (repo_key, email)
             );
             CREATE TABLE IF NOT EXISTS sampled_commit (
                 repo_key    TEXT NOT NULL,
                 sha         TEXT NOT NULL,
                 authored_at TEXT NOT NULL,
                 PRIMARY KEY (repo_key, sha)
             );
             CREATE TABLE IF NOT EXISTS commit_counter (
                 repo_key TEXT NOT NULL,
                 sha      TEXT NOT NULL,
                 key      TEXT NOT NULL,
                 value    INTEGER NOT NULL,
                 PRIMARY KEY (repo_key, sha, key)
             );
             CREATE TABLE IF NOT EXISTS feedback (
                 key       TEXT PRIMARY KEY,
                 statement TEXT NOT NULL,
                 category  TEXT NOT NULL,
                 scope     TEXT NOT NULL,
                 quote     TEXT NOT NULL,
                 first_at  TEXT NOT NULL,
                 last_at   TEXT NOT NULL,
                 status    TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS feedback_source (
                 key    TEXT NOT NULL,
                 source TEXT NOT NULL,
                 PRIMARY KEY (key, source)
             );
             CREATE TABLE IF NOT EXISTS feedback_evidence (
                 key         TEXT NOT NULL,
                 source      TEXT NOT NULL,
                 quote       TEXT NOT NULL,
                 observed_at TEXT NOT NULL,
                 attribution TEXT NOT NULL,
                 PRIMARY KEY (key, source, quote)
             );
             CREATE TABLE IF NOT EXISTS feedback_review_event (
                 id         INTEGER PRIMARY KEY,
                 key        TEXT NOT NULL,
                 status     TEXT NOT NULL,
                 at_epoch   INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS profile_reader_grant (
                 target_root TEXT NOT NULL,
                 client_id   TEXT NOT NULL,
                 PRIMARY KEY (target_root, client_id)
             );
             CREATE TABLE IF NOT EXISTS persona_claim (
                 id             INTEGER PRIMARY KEY,
                 kind           TEXT NOT NULL CHECK(kind = 'habit'),
                 when_text      TEXT NOT NULL,
                 behavior       TEXT NOT NULL,
                 outcome        TEXT NOT NULL,
                 exception_text TEXT NOT NULL,
                 scope          TEXT NOT NULL,
                 role           TEXT NOT NULL,
                 workflow       TEXT NOT NULL,
                 status         TEXT NOT NULL CHECK(status IN ('candidate', 'observed', 'stale', 'rejected', 'superseded'))
             );
             CREATE UNIQUE INDEX IF NOT EXISTS persona_claim_unique ON persona_claim
                 (kind, when_text, behavior, outcome, exception_text, scope, role, workflow);
             CREATE TABLE IF NOT EXISTS persona_claim_evidence (
                 id          INTEGER PRIMARY KEY,
                 claim_id    INTEGER NOT NULL,
                 source      TEXT NOT NULL,
                 source_path TEXT NOT NULL,
                 line_no     INTEGER NOT NULL,
                 record_digest TEXT NOT NULL,
                 episode     TEXT NOT NULL,
                 project     TEXT NOT NULL,
                 repository  TEXT NOT NULL,
                 quote       TEXT NOT NULL,
                 observed_at TEXT NOT NULL,
                 attribution TEXT NOT NULL,
                 relation    TEXT NOT NULL CHECK(relation IN ('supports', 'contradicts', 'limits')),
                 status      TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active', 'dismissed')),
                 UNIQUE(claim_id, source, quote, relation)
             );
             CREATE TABLE IF NOT EXISTS persona_review_event (
                 id       INTEGER PRIMARY KEY,
                 claim_id INTEGER NOT NULL,
                 status   TEXT NOT NULL,
                 at_epoch INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS persona_evidence_rebind (
                 id                INTEGER PRIMARY KEY,
                 claim_id          INTEGER NOT NULL,
                 evidence_id       INTEGER NOT NULL,
                 old_source_path   TEXT NOT NULL,
                 old_line_no       INTEGER NOT NULL,
                 old_record_digest TEXT NOT NULL,
                 old_observed_at   TEXT NOT NULL,
                 new_source_path   TEXT NOT NULL,
                 new_line_no       INTEGER NOT NULL,
                 new_record_digest TEXT NOT NULL,
                 new_observed_at   TEXT NOT NULL,
                 at_epoch          INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS persona_evidence_rebind_claim
                 ON persona_evidence_rebind (claim_id, id);
             DROP TABLE IF EXISTS counter;",
        )?;
        let has_extractor = conn
            .prepare("SELECT 1 FROM pragma_table_info('repo') WHERE name = 'extractor'")?
            .exists([])?;
        if !has_extractor {
            conn.execute_batch("ALTER TABLE repo ADD COLUMN extractor TEXT NOT NULL DEFAULT ''")?;
        }
        collection::ensure_schema(&mut conn)?;
        migrate_feedback_keys(&mut conn)?;
        verify_store_identity(&root, &target, expected_identity, false)?;
        Ok(Self { conn })
    }

    /// Open an existing profile store for diagnostics without creating its
    /// parent, database, schema, or SQLite sidecars.
    pub fn open_read_only(path: &Path) -> SqlResult<Self> {
        Self::open_optional_read_only(path)?.ok_or_else(|| {
            sqlite_path_error(
                "open style store",
                BoundedReadError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "style store does not exist",
                )),
            )
        })
    }

    pub(crate) fn open_optional_read_only(path: &Path) -> SqlResult<Option<Self>> {
        let (root, target) = match crate::bounded_fs::open_file_target(path) {
            Ok(target) => target,
            Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(sqlite_path_error("open style store parent", error)),
        };
        let expected = match crate::bounded_fs::read_regular_file_with_capability(
            &root,
            &target,
            MAX_STYLE_STORE_SIZE,
            0,
            ReadControl::default(),
        ) {
            Ok(file) => file.identity,
            Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                match crate::bounded_fs::inspect_absent_path(&root, &target, ReadControl::default())
                    .map_err(|error| sqlite_path_error("inspect style store", error))?
                {
                    Some(_) => return Ok(None),
                    None => return Err(sqlite_snapshot_changed()),
                }
            }
            Err(error) => return Err(sqlite_path_error("inspect style store", error)),
        };
        let conn = Connection::open_with_flags(
            &target,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        verify_store_identity(&root, &target, expected, true)?;
        conn.execute_batch("PRAGMA query_only = ON;")?;
        verify_store_identity(&root, &target, expected, true)?;
        Ok(Some(Self { conn }))
    }

    /// Replace `repo_key`'s contribution and remove its legacy checkout aliases
    /// in one transaction, so re-mining never sums the old and new identities.
    pub fn upsert_repo(
        &mut self,
        repo_key: &str,
        prov: &RepoProvenance,
        identities: &[String],
        commits: &[CommitEvidence],
        aliases: &[String],
    ) -> SqlResult<()> {
        self.apply_mine(false, aliases, repo_key, prov, identities, commits)
    }

    /// Apply the complete mutation for one mine in a single transaction. A
    /// failed replacement cannot leave a reset or retention sweep committed.
    pub fn apply_mine(
        &mut self,
        replace_all: bool,
        removed: &[String],
        repo_key: &str,
        prov: &RepoProvenance,
        identities: &[String],
        commits: &[CommitEvidence],
    ) -> SqlResult<()> {
        let tx = self.conn.transaction()?;
        if replace_all {
            tx.execute_batch(DELETE_ALL)?;
        }
        for key in removed {
            delete_repo(&tx, key)?;
        }
        delete_repo(&tx, repo_key)?;
        tx.execute(
            "INSERT OR REPLACE INTO repo (repo_key, author, commits_total, commits_sampled, \
             added_lines_sampled, latest_sha, latest_date, mined_at_epoch, extractor) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                repo_key,
                prov.author,
                prov.commits_total,
                prov.commits_sampled,
                prov.added_lines_sampled,
                prov.latest_sha,
                prov.latest_date,
                prov.mined_at_epoch,
                prov.extractor,
            ],
        )?;
        for commit in commits {
            tx.execute(
                "INSERT INTO sampled_commit (repo_key, sha, authored_at) VALUES (?1, ?2, ?3)",
                params![repo_key, commit.sha, commit.authored_at],
            )?;
            for (key, value) in &commit.counts {
                tx.execute(
                    "INSERT INTO commit_counter (repo_key, sha, key, value) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![repo_key, commit.sha, key, value],
                )?;
            }
        }
        for email in identities {
            tx.execute(
                "INSERT OR IGNORE INTO identity (repo_key, email) VALUES (?1, ?2)",
                params![repo_key, email],
            )?;
        }
        tx.commit()
    }

    /// Wipe everything — `--force` rebuilds the whole profile from scratch.
    pub fn reset(&mut self) -> SqlResult<()> {
        self.conn.execute_batch(DELETE_ALL)
    }

    /// Read access is fail-closed for stores predating this table. A grant is
    /// bound to one canonical repository root and one server-side client ID.
    pub fn reader_allowed(&self, target_root: &str, client_id: &str) -> SqlResult<bool> {
        if !self.has_table("profile_reader_grant")? {
            return Ok(false);
        }
        self.conn
            .query_row(
                "SELECT 1 FROM profile_reader_grant WHERE target_root = ?1 AND client_id = ?2",
                params![target_root, client_id],
                |_| Ok(true),
            )
            .optional()
            .map(|value| value.unwrap_or(false))
    }

    pub fn set_reader_grant(
        &mut self,
        target_root: &str,
        client_id: &str,
        allowed: bool,
    ) -> SqlResult<()> {
        if allowed {
            self.conn.execute(
                "INSERT OR IGNORE INTO profile_reader_grant (target_root, client_id) VALUES (?1, ?2)",
                params![target_root, client_id],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM profile_reader_grant WHERE target_root = ?1 AND client_id = ?2",
                params![target_root, client_id],
            )?;
        }
        Ok(())
    }

    pub fn reader_grants(&self) -> SqlResult<Vec<(String, String)>> {
        if !self.has_table("profile_reader_grant")? {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT target_root, client_id FROM profile_reader_grant ORDER BY target_root, client_id",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    /// Whether an older store opened read-only lacks a table a write-open
    /// would have created; readers treat it as empty.
    fn has_table(&self, name: &str) -> SqlResult<bool> {
        self.conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?
            .exists(params![name])
    }

    /// Sum counts and provenance across every repo mined with commit evidence.
    pub fn aggregate(&self) -> SqlResult<Aggregate> {
        if !self.has_table("sampled_commit")? || !self.has_table("commit_counter")? {
            // A line-level store read before its first write-open: every
            // repository is legacy and there is no commit evidence yet.
            let legacy_repos = self
                .conn
                .query_row("SELECT COUNT(*) FROM repo", [], |r| r.get::<_, i64>(0))?
                as usize;
            return Ok(Aggregate {
                repos: 0,
                legacy_repos,
                commits_total: 0,
                commits_sampled: 0,
                added_lines_sampled: 0,
                identities: Vec::new(),
                counts: Counts::new(),
                commits: Vec::new(),
                feedback: self.feedback()?,
                habits: self.habits()?,
            });
        }
        let mut commits: Vec<CommitEvidence> = Vec::new();
        let mut index: BTreeMap<(String, String), usize> = BTreeMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT repo_key, sha, authored_at FROM sampled_commit ORDER BY repo_key, sha",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (repo_key, sha, authored_at) = row?;
            index.insert((repo_key, sha.clone()), commits.len());
            commits.push(CommitEvidence {
                sha,
                authored_at,
                counts: Counts::new(),
            });
        }
        let mut stmt = self
            .conn
            .prepare("SELECT repo_key, sha, key, value FROM commit_counter")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        for row in rows {
            let (repo_key, sha, key, value) = row?;
            if let Some(&i) = index.get(&(repo_key, sha)) {
                commits[i].counts.insert(key, value);
            }
        }

        // Independent clones have different repo keys. A sampled Git commit is
        // still one author decision even when both checkouts were mined.
        let mut by_sha: BTreeMap<String, Vec<CommitEvidence>> = BTreeMap::new();
        for commit in commits {
            by_sha.entry(commit.sha.clone()).or_default().push(commit);
        }
        let commits: Vec<_> = by_sha.into_values().map(reconcile_commit).collect();
        let mut counts = Counts::new();
        for commit in &commits {
            for (key, value) in &commit.counts {
                *counts.entry(key.clone()).or_insert(0) += value;
            }
        }

        let (repos, commits_total) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(commits_total), 0) FROM repo WHERE EXISTS \
             (SELECT 1 FROM sampled_commit s WHERE s.repo_key = repo.repo_key)",
            [],
            |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)?)),
        )?;
        let legacy_repos = self.conn.query_row(
            "SELECT COUNT(*) FROM repo WHERE NOT EXISTS \
             (SELECT 1 FROM sampled_commit s WHERE s.repo_key = repo.repo_key)",
            [],
            |r| r.get::<_, i64>(0),
        )? as usize;

        let mut idstmt = self
            .conn
            .prepare("SELECT DISTINCT email FROM identity ORDER BY email")?;
        let identities: Vec<String> = idstmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<SqlResult<_>>()?;

        let feedback = self.feedback()?;
        Ok(Aggregate {
            repos,
            legacy_repos,
            commits_total,
            commits_sampled: commits
                .iter()
                .filter(|c| c.counts.get("diff.sampled").copied().unwrap_or(0) > 0)
                .count() as i64,
            added_lines_sampled: counts.get("diff.lines").copied().unwrap_or(0),
            identities,
            counts,
            commits,
            feedback,
            habits: self.habits()?,
        })
    }

    /// Record one stated preference as a candidate. Repeats from another source
    /// widen its date range and count but never change its status: agents and
    /// files can repeat a statement, only the author accepts or rejects it.
    pub fn record_feedback(&mut self, entry: &NewFeedback<'_>) -> SqlResult<Feedback> {
        let tx = self.conn.transaction()?;
        let key = record_feedback_tx(&tx, entry)?;
        collection::check_size(&tx)?;
        tx.commit()?;
        Ok(self
            .feedback()?
            .into_iter()
            .find(|stored| stored.key == key)
            .expect("recorded feedback is stored"))
    }

    /// Set the status of the entry whose key starts with `prefix`. An ambiguous
    /// or unknown prefix changes nothing and returns the number of matches.
    #[cfg(test)]
    pub fn set_feedback_status(&mut self, prefix: &str, status: &str) -> SqlResult<usize> {
        let entries = self.feedback()?;
        let matches: Vec<_> = entries
            .iter()
            .filter(|entry| entry.key.starts_with(prefix))
            .collect();
        if let [entry] = matches.as_slice() {
            self.conn.execute(
                "UPDATE feedback SET status=?2 WHERE key=?1",
                params![entry.key, status],
            )?;
        }
        Ok(matches.len())
    }

    pub fn feedback_evidence(&self, key: &str) -> SqlResult<Vec<FeedbackEvidence>> {
        if !self.has_table("feedback_evidence")? {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT source, quote, observed_at, attribution FROM feedback_evidence \
             WHERE key = ?1 ORDER BY observed_at, source, quote",
        )?;
        let rows = stmt.query_map(params![key], |row| {
            Ok(FeedbackEvidence {
                source: row.get(0)?,
                quote: row.get(1)?,
                at: row.get(2)?,
                attribution: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// Every stated preference, ordered by key, with its complete review revision.
    pub fn feedback(&self) -> SqlResult<Vec<Feedback>> {
        preferences::read_feedback(&self.conn)
    }

    /// Create an advisory habit candidate, or attach another exact quote to an
    /// identical candidate. Repeated sessions from one task share an episode.
    pub fn record_habit(
        &mut self,
        claim: &NewHabit<'_>,
        evidence: &NewHabitEvidence<'_>,
    ) -> SqlResult<Habit> {
        let tx = self.conn.transaction()?;
        let id = record_habit_tx(&tx, claim, evidence, false, None)?;
        collection::check_size(&tx)?;
        tx.commit()?;
        Ok(self.habit(id)?.expect("recorded habit is stored"))
    }

    /// Attach support, a limit, or a counterexample. Changed evidence withdraws
    /// the observation until the new content has been reviewed.
    pub fn add_habit_evidence(
        &mut self,
        id: i64,
        evidence: &NewHabitEvidence<'_>,
    ) -> SqlResult<Option<Habit>> {
        let tx = self.conn.transaction()?;
        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM persona_claim WHERE id = ?1 AND kind = 'habit'",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(status) = status else {
            return Ok(None);
        };
        if status == "superseded"
            || habit_supersession::has_successor(&tx, id)?
            || habit_generations::has_child(&tx, id)?
        {
            return Err(rusqlite::Error::InvalidParameterName(
                "a retired habit cannot receive evidence".into(),
            ));
        }
        if status != "rejected" {
            let scope: String = tx.query_row(
                "SELECT scope FROM persona_claim WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )?;
            validate_habit_project_scope(&scope, evidence.project)?;
            insert_habit_evidence(&tx, id, evidence)?;
        }
        collection::check_size(&tx)?;
        tx.commit()?;
        self.habit(id)
    }

    pub fn habits(&self) -> SqlResult<Vec<Habit>> {
        habit_review::read_habits(&self.conn, None)
    }

    pub fn habit(&self, id: i64) -> SqlResult<Option<Habit>> {
        Ok(habit_review::read_habits(&self.conn, Some(id))?
            .into_iter()
            .next())
    }

    pub fn habit_evidence(&self, id: i64) -> SqlResult<Vec<HabitEvidence>> {
        if !self.has_table("persona_claim_evidence")? {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT id, source, source_path, line_no, record_digest, episode, project, repository, quote, observed_at, relation, status \
             FROM persona_claim_evidence WHERE claim_id = ?1 ORDER BY observed_at, id",
        )?;
        let rows = stmt.query_map(params![id], |row| {
            Ok(HabitEvidence {
                id: row.get(0)?,
                source: row.get(1)?,
                source_path: row.get(2)?,
                line_no: row.get(3)?,
                record_digest: row.get(4)?,
                episode: row.get(5)?,
                project: row.get(6)?,
                repository: row.get(7)?,
                quote: row.get(8)?,
                at: row.get(9)?,
                relation: row.get(10)?,
                status: row.get(11)?,
            })
        })?;
        rows.collect()
    }

    pub fn habit_rebinds(&self, id: i64) -> SqlResult<Vec<HabitEvidenceRebind>> {
        if !self.has_table("persona_evidence_rebind")? {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT evidence_id, old_source_path, old_line_no, old_record_digest, old_observed_at, \
             new_source_path, new_line_no, new_record_digest, new_observed_at \
             FROM persona_evidence_rebind WHERE claim_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![id], |row| {
            Ok(HabitEvidenceRebind {
                evidence_id: row.get(0)?,
                old_source_path: row.get(1)?,
                old_line_no: row.get(2)?,
                old_record_digest: row.get(3)?,
                old_at: row.get(4)?,
                new_source_path: row.get(5)?,
                new_line_no: row.get(6)?,
                new_record_digest: row.get(7)?,
                new_at: row.get(8)?,
            })
        })?;
        rows.collect()
    }

    /// Keep an incorrect citation in the audit trail but stop counting it.
    /// A stale claim still needs a fresh observe review after the correction.
    pub fn dismiss_habit_evidence(&mut self, claim_id: i64, evidence_id: i64) -> SqlResult<bool> {
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "UPDATE persona_claim_evidence SET status = 'dismissed' \
             WHERE id = ?1 AND claim_id = ?2 AND status = 'active'",
            params![evidence_id, claim_id],
        )?;
        if changed > 0 {
            invalidate_habit_observation(&tx, claim_id)?;
            tx.execute(
                "INSERT INTO persona_review_event (claim_id, status, at_epoch) \
                 VALUES (?1, ?2, unixepoch())",
                params![claim_id, format!("dismissed_evidence:{evidence_id}")],
            )?;
        }
        collection::check_size(&tx)?;
        tx.commit()?;
        Ok(changed > 0)
    }

    /// Author labels and identities already represented in the store.
    ///
    /// `style.db` intentionally models one person across repositories. Callers
    /// must reject a new contribution when neither its author label nor any of
    /// its identities overlap these signals.
    pub fn owner_signals(&self) -> SqlResult<OwnerSignals> {
        let mut authors_stmt = self
            .conn
            .prepare("SELECT DISTINCT author FROM repo ORDER BY author")?;
        let authors = authors_stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<SqlResult<Vec<_>>>()?;

        let mut identities_stmt = self
            .conn
            .prepare("SELECT DISTINCT email FROM identity ORDER BY email")?;
        let identities = identities_stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<SqlResult<Vec<_>>>()?;

        Ok((authors, identities))
    }

    /// One repository's author, extractor stamp and commit tallies, so an
    /// incremental mine can skip diffs it has already measured.
    pub fn repo_evidence(&self, repo_key: &str) -> SqlResult<Option<RepoEvidence>> {
        let Some((author, extractor)) = self
            .conn
            .query_row(
                "SELECT author, extractor FROM repo WHERE repo_key = ?1",
                params![repo_key],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
        else {
            return Ok(None);
        };
        let mut commits: Vec<CommitEvidence> = Vec::new();
        let mut index: BTreeMap<String, usize> = BTreeMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT sha, authored_at FROM sampled_commit WHERE repo_key = ?1 ORDER BY sha",
        )?;
        let rows = stmt.query_map(params![repo_key], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (sha, authored_at) = row?;
            index.insert(sha.clone(), commits.len());
            commits.push(CommitEvidence {
                sha,
                authored_at,
                counts: Counts::new(),
            });
        }
        let mut stmt = self
            .conn
            .prepare("SELECT sha, key, value FROM commit_counter WHERE repo_key = ?1")?;
        let rows = stmt.query_map(params![repo_key], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (sha, key, value) = row?;
            if let Some(&i) = index.get(&sha) {
                commits[i].counts.insert(key, value);
            }
        }
        Ok(Some(RepoEvidence {
            author,
            extractor,
            commits,
        }))
    }

    /// The stored mine point (SHA) for a repo — `doctor` checks `<sha>..HEAD`.
    pub fn repo_latest_sha(&self, repo_key: &str) -> SqlResult<Option<String>> {
        self.conn
            .query_row(
                "SELECT latest_sha FROM repo WHERE repo_key = ?1",
                params![repo_key],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .map(|o| o.flatten())
    }

    /// `(author, latest_sha, latest_date)` for a repo — staleness needs the
    /// author to count `--author=X <sha>..HEAD` and the date to display.
    pub fn repo_meta(&self, repo_key: &str) -> SqlResult<Option<RepoMeta>> {
        self.conn
            .query_row(
                "SELECT author, latest_sha, latest_date FROM repo WHERE repo_key = ?1",
                params![repo_key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
    }

    /// `(repo_key, mined_at_epoch)` for every stored repo. The caller decides
    /// retention (gone-from-disk / too-old) — this layer stays pure CRUD.
    pub fn list_repos(&self) -> SqlResult<Vec<(String, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT repo_key, mined_at_epoch FROM repo")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Drop the given repos and all their counts/identities (retention sweep).
    pub fn prune_repos(&mut self, keys: &[String]) -> SqlResult<()> {
        let tx = self.conn.transaction()?;
        for key in keys {
            delete_repo(&tx, key)?;
        }
        tx.commit()
    }
}

const DELETE_ALL: &str = "DELETE FROM commit_counter; DELETE FROM sampled_commit; \
                          DELETE FROM identity; DELETE FROM repo; \
                          DELETE FROM feedback_review_event; DELETE FROM feedback_evidence; \
                          DELETE FROM feedback_source; DELETE FROM feedback; \
                          DELETE FROM feedback_acceptance; DELETE FROM persona_candidate_feedback; \
                          DELETE FROM persona_feedback_dismissal; \
                          DELETE FROM feedback_supersession; \
                          DELETE FROM profile_reader_grant; \
                          DELETE FROM persona_evidence_rebind; \
                          DELETE FROM persona_habit_observation; \
                          DELETE FROM persona_habit_supersession; \
                          DELETE FROM persona_habit_generation; \
                          DELETE FROM persona_review_event; DELETE FROM persona_claim_evidence; \
                          DELETE FROM persona_claim; \
                          DELETE FROM persona_candidate_habit; DELETE FROM persona_candidate_review; DELETE FROM persona_candidate_revision; \
                          DELETE FROM persona_candidate; DELETE FROM persona_sync_exclusion; DELETE FROM persona_collection_source;";

fn record_feedback_tx(
    tx: &rusqlite::Transaction<'_>,
    entry: &NewFeedback<'_>,
) -> SqlResult<String> {
    let key = feedback_key(entry.statement, entry.category, entry.scope);
    let existing: Option<(String, String, String)> = tx
        .query_row(
            "SELECT statement, category, scope FROM feedback WHERE key=?1",
            [&key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let normalize = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    if existing.is_some_and(|(statement, category, scope)| {
        normalize(&statement) != normalize(entry.statement)
            || category != entry.category
            || scope != entry.scope
    }) {
        return Err(rusqlite::Error::InvalidParameterName(
            "feedback key collision; use the exact stored statement or a distinct statement".into(),
        ));
    }
    tx.execute(
        "INSERT INTO feedback (key, statement, category, scope, quote, first_at, last_at, status) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, 'candidate') \
             ON CONFLICT(key) DO UPDATE SET \
                 first_at = min(first_at, excluded.first_at), \
                 last_at = max(last_at, excluded.last_at)",
        params![
            key,
            entry.statement,
            entry.category,
            entry.scope,
            entry.quote,
            entry.at,
        ],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO feedback_source (key, source) VALUES (?1, ?2)",
        params![key, entry.source],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO feedback_evidence \
             (key, source, quote, observed_at, attribution) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            key,
            entry.source,
            entry.quote,
            entry.at,
            if entry.source.starts_with("session:") {
                "human_turn"
            } else if entry.source.starts_with("memory:") {
                "agent_memory"
            } else {
                "unknown"
            }
        ],
    )?;
    Ok(key)
}

fn record_habit_tx(
    tx: &rusqlite::Transaction<'_>,
    claim: &NewHabit<'_>,
    evidence: &NewHabitEvidence<'_>,
    candidate_only: bool,
    target: Option<i64>,
) -> SqlResult<i64> {
    validate_habit_project_scope(claim.scope, evidence.project)?;
    if target.is_none() {
        tx.execute(
        "INSERT OR IGNORE INTO persona_claim \
             (kind, when_text, behavior, outcome, exception_text, scope, role, workflow, status, generation) \
             VALUES ('habit', ?1, ?2, ?3, ?4, ?5, ?6, ?7, 'candidate', 1)",
        params![
            claim.when,
            claim.behavior,
            claim.outcome,
            claim.exception,
            claim.scope,
            claim.role,
            claim.workflow
        ],
    )?;
    }
    let (id, status): (i64, String) = tx
        .query_row(
            "SELECT id, status FROM persona_claim WHERE kind = 'habit' AND when_text = ?1 \
             AND behavior = ?2 AND outcome = ?3 AND exception_text = ?4 \
             AND scope = ?5 AND role = ?6 AND workflow = ?7 \
             AND ((?8 IS NULL AND generation=1) OR id=?8)",
            params![
                claim.when,
                claim.behavior,
                claim.outcome,
                claim.exception,
                claim.scope,
                claim.role,
                claim.workflow,
                target
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| {
            rusqlite::Error::InvalidParameterName(
                "target habit missing or definition/scope/role/workflow differs".into(),
            )
        })?;
    if status == "superseded"
        || habit_supersession::has_successor(tx, id)?
        || habit_generations::has_child(tx, id)?
    {
        return Err(rusqlite::Error::InvalidParameterName(
            "a retired habit cannot be proposed again".into(),
        ));
    }
    if candidate_only && status != "candidate" {
        return Err(rusqlite::Error::InvalidParameterName(
            "existing habit already reviewed; proposal cannot add evidence".into(),
        ));
    }
    if status != "rejected" {
        insert_habit_evidence(tx, id, evidence)?;
    }
    Ok(id)
}

fn invalidate_habit_observation(tx: &rusqlite::Transaction<'_>, id: i64) -> SqlResult<()> {
    tx.execute(
        "DELETE FROM persona_habit_observation WHERE claim_id=?1",
        [id],
    )?;
    tx.execute(
        "UPDATE persona_claim SET status=CASE WHEN EXISTS(SELECT 1 FROM persona_habit_supersession WHERE old_id=?1)
         THEN 'superseded' WHEN EXISTS(SELECT 1 FROM persona_habit_generation WHERE parent_id=?1)
         THEN (SELECT parent_status FROM persona_habit_generation WHERE parent_id=?1)
         ELSE 'stale' END WHERE id=?1 AND (status='observed'
         OR EXISTS(SELECT 1 FROM persona_habit_supersession WHERE old_id=?1)
         OR EXISTS(SELECT 1 FROM persona_habit_generation WHERE parent_id=?1))",
        [id],
    )?;
    Ok(())
}

fn insert_habit_evidence(
    tx: &rusqlite::Transaction<'_>,
    id: i64,
    evidence: &NewHabitEvidence<'_>,
) -> SqlResult<()> {
    type StoredEvidence = (
        i64,
        String,
        i64,
        String,
        String,
        String,
        String,
        String,
        String,
    );
    let previous: Option<StoredEvidence> = tx
        .query_row(
            "SELECT id, source_path, line_no, record_digest, episode, project, repository, status, observed_at \
             FROM persona_claim_evidence \
             WHERE claim_id = ?1 AND source = ?2 AND quote = ?3 AND relation = ?4",
            params![id, evidence.source, evidence.quote, evidence.relation],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()?;
    if let Some((
        evidence_id,
        old_path,
        old_line,
        old_digest,
        episode,
        project,
        repository,
        status,
        old_at,
    )) = previous
    {
        if status == "dismissed" {
            return Err(rusqlite::Error::InvalidParameterName(
                "dismissed habit citation cannot be restored by repeating cite".into(),
            ));
        }
        if episode != evidence.episode
            || project != evidence.project
            || repository != evidence.repository
        {
            return Err(rusqlite::Error::InvalidParameterName(
                "a citation cannot be moved to another episode or project".into(),
            ));
        }
        if old_path == evidence.source_path
            && old_line == evidence.line_no
            && old_digest == evidence.record_digest
        {
            return Ok(());
        }
        tx.execute(
            "INSERT INTO persona_evidence_rebind \
             (claim_id, evidence_id, old_source_path, old_line_no, old_record_digest, old_observed_at, \
              new_source_path, new_line_no, new_record_digest, new_observed_at, at_epoch) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, unixepoch())",
            params![
                id,
                evidence_id,
                old_path,
                old_line,
                old_digest,
                old_at,
                evidence.source_path,
                evidence.line_no,
                evidence.record_digest,
                evidence.at,
            ],
        )?;
        tx.execute(
            "UPDATE persona_claim_evidence \
             SET source_path = ?2, line_no = ?3, record_digest = ?4, observed_at = ?5 \
             WHERE id = ?1",
            params![
                evidence_id,
                evidence.source_path,
                evidence.line_no,
                evidence.record_digest,
                evidence.at,
            ],
        )?;
        invalidate_habit_observation(tx, id)?;
        return Ok(());
    }
    tx.execute(
        "INSERT INTO persona_claim_evidence \
         (claim_id, source, source_path, line_no, record_digest, episode, project, repository, quote, observed_at, attribution, relation) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'human_turn', ?11)",
        params![
            id,
            evidence.source,
            evidence.source_path,
            evidence.line_no,
            evidence.record_digest,
            evidence.episode,
            evidence.project,
            evidence.repository,
            evidence.quote,
            evidence.at,
            evidence.relation
        ],
    )?;
    invalidate_habit_observation(tx, id)?;
    Ok(())
}

fn validate_habit_project_scope(scope: &str, project: &str) -> SqlResult<()> {
    if let Some(expected) = scope.strip_prefix("project:") {
        if expected != project {
            return Err(rusqlite::Error::InvalidParameterName(
                "habit evidence belongs to another project".into(),
            ));
        }
    }
    Ok(())
}

/// A readable key whose identity also binds category and scope. The text prefix
/// retains the existing `feedback accept <prefix>` workflow, while the digest
/// keeps an identical statement in another scope distinct.
pub fn feedback_key(statement: &str, category: &str, scope: &str) -> String {
    let words = statement
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("-");
    let mut digest = Sha256::new();
    digest.update(b"mastermind-feedback-v2\0");
    for field in [category, scope, &words] {
        digest_str(&mut digest, field);
    }
    let suffix = crate::hex::encode(&digest.finalize());
    format!("{words}--{}", &suffix[..16])
}

/// Existing stores keyed feedback by statement alone. Re-key in one
/// transaction, preserving review status, dates, quotes, and source links.
fn migrate_feedback_keys(conn: &mut Connection) -> SqlResult<()> {
    let entries = {
        let mut stmt = conn.prepare("SELECT key, statement, category, scope FROM feedback")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.collect::<SqlResult<Vec<_>>>()?
    };
    let tx = conn.transaction()?;
    for (old, statement, category, scope) in entries {
        let new = feedback_key(&statement, &category, &scope);
        if old == new {
            continue;
        }
        tx.execute(
            "UPDATE feedback SET key = ?1 WHERE key = ?2",
            params![new, old],
        )?;
        tx.execute(
            "UPDATE feedback_source SET key = ?1 WHERE key = ?2",
            params![new, old],
        )?;
        tx.execute(
            "UPDATE feedback_evidence SET key = ?1 WHERE key = ?2",
            params![new, old],
        )?;
        tx.execute(
            "UPDATE feedback_review_event SET key = ?1 WHERE key = ?2",
            params![new, old],
        )?;
        tx.execute(
            "UPDATE persona_candidate_feedback SET feedback_key=?1 WHERE feedback_key=?2",
            params![new, old],
        )?;
        // A changed key is a changed review definition: retain the old pin so
        // it fails comparison until the newly keyed entry is reviewed again.
        tx.execute(
            "UPDATE feedback_acceptance SET key=?1 WHERE key=?2",
            params![new, old],
        )?;
        tx.execute(
            "UPDATE persona_feedback_dismissal SET feedback_key=?1 WHERE feedback_key=?2",
            params![new, old],
        )?;
        tx.execute(
            "UPDATE feedback_supersession SET old_key=?1 WHERE old_key=?2",
            params![new, old],
        )?;
        tx.execute(
            "UPDATE feedback_supersession SET new_key=?1 WHERE new_key=?2",
            params![new, old],
        )?;
    }
    collection::check_size(&tx)?;
    tx.commit()
}

fn delete_repo(transaction: &rusqlite::Transaction<'_>, repo_key: &str) -> SqlResult<()> {
    for table in ["commit_counter", "sampled_commit", "identity", "repo"] {
        transaction.execute(
            &format!("DELETE FROM {table} WHERE repo_key = ?1"),
            params![repo_key],
        )?;
    }
    Ok(())
}

fn verify_store_identity(
    root: &crate::bounded_fs::RootCapability,
    target: &Path,
    expected: StableFileIdentity,
    exact: bool,
) -> SqlResult<()> {
    root.verify()
        .map_err(|error| sqlite_path_error("verify style store parent", error))?;
    let opened = crate::bounded_fs::read_regular_file_with_capability(
        root,
        target,
        MAX_STYLE_STORE_SIZE,
        0,
        ReadControl::default(),
    )
    .map_err(|error| sqlite_path_error("verify style store identity", error))?;
    let matches = if exact {
        opened.identity == expected
    } else {
        opened.identity.same_object(expected)
    };
    if matches {
        Ok(())
    } else {
        Err(sqlite_snapshot_changed())
    }
}

fn sqlite_path_error(context: &str, error: BoundedReadError) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
        Some(format!("{context}: {error}")),
    )
}

fn sqlite_snapshot_changed() -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
        Some("style store changed while opening".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(pairs: &[(&str, i64)]) -> Counts {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn commit(pairs: &[(&str, i64)]) -> Vec<CommitEvidence> {
        vec![CommitEvidence {
            sha: "c".repeat(40),
            authored_at: "2026-01-01".into(),
            counts: counts(pairs),
        }]
    }

    fn prov(sha: Option<&str>, mined_at_epoch: i64) -> RepoProvenance {
        RepoProvenance {
            author: "me".into(),
            commits_total: 10,
            commits_sampled: 10,
            added_lines_sampled: 100,
            latest_sha: sha.map(String::from),
            latest_date: Some("2026-01-01".into()),
            mined_at_epoch,
            extractor: "test".into(),
        }
    }

    #[test]
    fn aggregate_sums_across_repos() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ProfileStore::open(&dir.path().join("style.db")).unwrap();
        s.upsert_repo(
            "/a",
            &prov(Some("aaa"), 0),
            &["a@x".into()],
            &commit(&[("indent.space", 30), ("indent.tab", 2)]),
            &[],
        )
        .unwrap();
        let mut second_commit = commit(&[("indent.space", 20), ("indent.tab", 8)]);
        second_commit[0].sha = "d".repeat(40);
        s.upsert_repo(
            "/b",
            &prov(Some("bbb"), 0),
            &["b@x".into()],
            &second_commit,
            &[],
        )
        .unwrap();

        let agg = s.aggregate().unwrap();
        assert_eq!(agg.repos, 2);
        assert_eq!(agg.commits_total, 20);
        assert_eq!(agg.counts["indent.space"], 50);
        assert_eq!(agg.counts["indent.tab"], 10);
        assert_eq!(agg.identities, vec!["a@x".to_string(), "b@x".to_string()]);
        assert_eq!(s.repo_latest_sha("/a").unwrap().as_deref(), Some("aaa"));
        assert_eq!(s.repo_latest_sha("/missing").unwrap(), None);
        assert_eq!(
            s.owner_signals().unwrap(),
            (
                vec!["me".to_string()],
                vec!["a@x".to_string(), "b@x".to_string()]
            )
        );
    }

    #[test]
    fn aggregate_counts_a_commit_seen_in_two_clones_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProfileStore::open(&dir.path().join("style.db")).unwrap();
        let sampled = commit(&[
            ("indent.space", 7),
            ("indent.tab", 1),
            ("diff.sampled", 1),
            ("diff.lines", 8),
        ]);
        store
            .upsert_repo("/clone-a", &prov(None, 0), &[], &sampled, &[])
            .unwrap();
        store
            .upsert_repo("/clone-b", &prov(None, 0), &[], &sampled, &[])
            .unwrap();
        let aggregate = store.aggregate().unwrap();
        assert_eq!(aggregate.repos, 2);
        assert_eq!(aggregate.commits_sampled, 1);
        assert_eq!(aggregate.added_lines_sampled, 8);
        assert_eq!(aggregate.commits.len(), 1);
        assert_eq!(aggregate.counts["indent.space"], 7);
    }

    #[test]
    fn duplicate_contexts_are_order_independent_and_never_mix_measurements() {
        let metadata = commit(&[("commit.total", 1)]).remove(0);
        let measured = commit(&[
            ("commit.total", 1),
            ("diff.sampled", 1),
            ("diff.lines", 8),
            ("indent.space", 8),
        ])
        .remove(0);
        for copies in [
            vec![metadata.clone(), measured.clone()],
            vec![measured.clone(), metadata.clone()],
            vec![measured.clone(), metadata.clone(), measured.clone()],
        ] {
            assert_eq!(reconcile_commit(copies).counts, measured.counts);
        }
        let conflicting = commit(&[
            ("commit.total", 1),
            ("diff.sampled", 1),
            ("diff.lines", 8),
            ("tool.indent.space", 8),
        ])
        .remove(0);
        for copies in [
            vec![measured.clone(), conflicting.clone(), metadata.clone()],
            vec![conflicting.clone(), metadata, measured.clone(), conflicting],
        ] {
            let merged = reconcile_commit(copies);
            assert_eq!(
                merged.counts,
                Counts::from([("evidence.context_conflict".into(), 1)])
            );
        }
    }

    #[test]
    fn profile_revision_is_deterministic_and_tracks_rendered_inputs() {
        let evidence = commit(&[("indent.space", 20), ("indent.tab", 1)]);
        let baseline = Aggregate {
            repos: 1,
            legacy_repos: 0,
            commits_total: 10,
            commits_sampled: 8,
            added_lines_sampled: 120,
            identities: vec!["private@example.test".into()],
            counts: evidence[0].counts.clone(),
            commits: evidence.clone(),
            feedback: Vec::new(),
            habits: Vec::new(),
        };
        let same_rendered_inputs = Aggregate {
            repos: baseline.repos,
            legacy_repos: 0,
            commits_total: baseline.commits_total,
            commits_sampled: baseline.commits_sampled,
            added_lines_sampled: baseline.added_lines_sampled,
            identities: vec!["another-private@example.test".into()],
            counts: baseline.counts.clone(),
            commits: evidence,
            feedback: Vec::new(),
            habits: Vec::new(),
        };
        assert_eq!(
            baseline.profile_revision(),
            same_rendered_inputs.profile_revision()
        );

        let changed = Aggregate {
            commits: commit(&[("indent.space", 21), ("indent.tab", 1)]),
            ..same_rendered_inputs
        };
        let changed_revision = changed.profile_revision();
        assert_ne!(baseline.profile_revision(), changed_revision);
        let legacy = Aggregate {
            legacy_repos: 1,
            ..changed
        };
        assert_ne!(legacy.profile_revision(), changed_revision);
    }

    fn stated<'a>(statement: &'a str, source: &'a str, at: &'a str) -> NewFeedback<'a> {
        NewFeedback {
            statement,
            category: "code",
            scope: "global",
            quote: "never unwrap in library code",
            at,
            source,
        }
    }

    fn habit<'a>(scope: &'a str) -> NewHabit<'a> {
        NewHabit {
            when: "a change crosses service boundaries",
            behavior: "checks the contract and rollout separately",
            outcome: "reports each unverified delivery stage",
            exception: "",
            scope,
            role: "auditor",
            workflow: "strict",
        }
    }

    fn habit_quote<'a>(
        source: &'a str,
        episode: &'a str,
        project: &'a str,
        relation: &'a str,
    ) -> NewHabitEvidence<'a> {
        NewHabitEvidence {
            source,
            source_path: "fixture.jsonl",
            line_no: 1,
            record_digest: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            episode,
            project,
            repository: project,
            quote: "check the contract and rollout separately",
            at: "2026-09-25",
            relation,
        }
    }

    #[test]
    fn habit_review_needs_independent_tasks_and_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProfileStore::open(&dir.path().join("style.db")).unwrap();
        let first = store
            .record_habit(
                &habit("project:alpha"),
                &habit_quote("session:a", "task-1", "alpha", "supports"),
            )
            .unwrap();
        assert_eq!(first.status, "candidate");
        assert!(store
            .review_habit(
                first.id,
                "observed",
                Some(&store.habit(first.id).unwrap().unwrap().review_revision())
            )
            .unwrap()
            .is_err());
        store
            .add_habit_evidence(
                first.id,
                &habit_quote("session:b", "task-1", "alpha", "supports"),
            )
            .unwrap();
        assert!(store
            .review_habit(
                first.id,
                "observed",
                Some(&store.habit(first.id).unwrap().unwrap().review_revision())
            )
            .unwrap()
            .is_err());
        store
            .add_habit_evidence(
                first.id,
                &habit_quote("session:c", "task-2", "alpha", "supports"),
            )
            .unwrap();
        let reviewed = store
            .review_habit(
                first.id,
                "observed",
                Some(&store.habit(first.id).unwrap().unwrap().review_revision()),
            )
            .unwrap()
            .unwrap();
        assert_eq!((reviewed.episodes, reviewed.sources), (2, 3));
        assert_eq!(reviewed.status, "observed");
        let before = store.aggregate().unwrap().profile_revision();
        store
            .add_habit_evidence(
                first.id,
                &habit_quote("session:d", "task-3", "alpha", "contradicts"),
            )
            .unwrap();
        let stale = store.habit(first.id).unwrap().unwrap();
        assert_eq!((stale.status.as_str(), stale.contradictions), ("stale", 1));
        assert_ne!(before, store.aggregate().unwrap().profile_revision());
        assert!(store
            .review_habit(
                first.id,
                "observed",
                Some(&store.habit(first.id).unwrap().unwrap().review_revision())
            )
            .unwrap()
            .is_err());
        let contradiction = store
            .habit_evidence(first.id)
            .unwrap()
            .into_iter()
            .find(|item| item.relation == "contradicts")
            .unwrap();
        assert!(store
            .dismiss_habit_evidence(first.id, contradiction.id)
            .unwrap());
        assert!(!store
            .dismiss_habit_evidence(first.id, contradiction.id)
            .unwrap());
        assert_eq!(
            store
                .habit_evidence(first.id)
                .unwrap()
                .into_iter()
                .find(|item| item.id == contradiction.id)
                .unwrap()
                .status,
            "dismissed"
        );
        assert_eq!(
            store
                .review_habit(
                    first.id,
                    "observed",
                    Some(&store.habit(first.id).unwrap().unwrap().review_revision())
                )
                .unwrap()
                .unwrap()
                .status,
            "observed"
        );
        let support = store
            .habit_evidence(first.id)
            .unwrap()
            .into_iter()
            .find(|item| item.relation == "supports")
            .unwrap();
        assert!(store.dismiss_habit_evidence(first.id, support.id).unwrap());
        assert_eq!(store.habit(first.id).unwrap().unwrap().status, "stale");
        assert_eq!(
            store
                .review_habit(
                    first.id,
                    "observed",
                    Some(&store.habit(first.id).unwrap().unwrap().review_revision())
                )
                .unwrap()
                .unwrap()
                .status,
            "observed"
        );
        store
            .add_habit_evidence(
                first.id,
                &habit_quote("session:e", "task-4", "alpha", "limits"),
            )
            .unwrap();
        assert_eq!(store.habit(first.id).unwrap().unwrap().status, "stale");
        assert!(store
            .review_habit(
                first.id,
                "observed",
                Some(&store.habit(first.id).unwrap().unwrap().review_revision())
            )
            .unwrap()
            .is_err());
    }

    #[test]
    fn relocated_habit_citation_stales_claim_and_retains_old_locator() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProfileStore::open(&dir.path().join("style.db")).unwrap();
        let claim = store
            .record_habit(
                &habit("project:alpha"),
                &habit_quote("session:a", "task-1", "alpha", "supports"),
            )
            .unwrap();
        store
            .add_habit_evidence(
                claim.id,
                &habit_quote("session:b", "task-2", "alpha", "supports"),
            )
            .unwrap();
        store
            .review_habit(
                claim.id,
                "observed",
                Some(&store.habit(claim.id).unwrap().unwrap().review_revision()),
            )
            .unwrap()
            .unwrap();
        let original_revision = store.aggregate().unwrap().profile_revision();
        let mut moved = habit_quote("session:a", "task-1", "alpha", "supports");
        moved.source_path = "relocated.jsonl";
        moved.line_no = 7;
        moved.record_digest = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        moved.at = "2026-09-26";
        let stale = store.add_habit_evidence(claim.id, &moved).unwrap().unwrap();
        assert_ne!(
            store.aggregate().unwrap().profile_revision(),
            original_revision
        );
        assert_eq!(
            (stale.status.as_str(), stale.sources, stale.episodes),
            ("stale", 2, 2)
        );
        assert_eq!(store.habit_rebinds(claim.id).unwrap().len(), 1);
        let current = store
            .habit_evidence(claim.id)
            .unwrap()
            .into_iter()
            .find(|item| item.source == "session:a")
            .unwrap();
        assert_eq!(
            (current.source_path.as_str(), current.line_no),
            ("relocated.jsonl", 7)
        );
        let old = &store.habit_rebinds(claim.id).unwrap()[0];
        assert_eq!(
            (old.old_source_path.as_str(), old.old_line_no),
            ("fixture.jsonl", 1)
        );
        assert_eq!(
            (old.old_at.as_str(), old.new_at.as_str()),
            ("2026-09-25", "2026-09-26")
        );
        store.add_habit_evidence(claim.id, &moved).unwrap();
        assert_eq!(store.habit_rebinds(claim.id).unwrap().len(), 1);
        moved.episode = "another-task";
        assert!(store.add_habit_evidence(claim.id, &moved).is_err());
        assert_eq!(store.habit(claim.id).unwrap().unwrap().status, "stale");
        store
            .review_habit(
                claim.id,
                "observed",
                Some(&store.habit(claim.id).unwrap().unwrap().review_revision()),
            )
            .unwrap()
            .unwrap();
        assert_ne!(
            store.aggregate().unwrap().profile_revision(),
            original_revision
        );
        store.reset().unwrap();
        assert!(store.habit_rebinds(claim.id).unwrap().is_empty());
    }

    #[test]
    fn cross_project_habit_needs_two_projects_and_repeated_proposal_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProfileStore::open(&dir.path().join("style.db")).unwrap();
        let first = store
            .record_habit(
                &habit("global"),
                &habit_quote("session:a", "task-1", "alpha", "supports"),
            )
            .unwrap();
        let repeated = store
            .record_habit(
                &habit("global"),
                &habit_quote("session:a", "task-1", "alpha", "supports"),
            )
            .unwrap();
        assert_eq!(first.id, repeated.id);
        assert_eq!((repeated.episodes, repeated.sources), (1, 1));
        store
            .add_habit_evidence(
                first.id,
                &habit_quote("session:b", "task-2", "alpha", "supports"),
            )
            .unwrap();
        assert!(store
            .review_habit(
                first.id,
                "observed",
                Some(&store.habit(first.id).unwrap().unwrap().review_revision())
            )
            .unwrap()
            .is_err());
        store
            .add_habit_evidence(
                first.id,
                &habit_quote("session:c", "task-3", "beta", "supports"),
            )
            .unwrap();
        let reviewed = store
            .review_habit(
                first.id,
                "observed",
                Some(&store.habit(first.id).unwrap().unwrap().review_revision()),
            )
            .unwrap()
            .unwrap();
        assert_eq!((reviewed.projects, reviewed.episodes), (2, 3));
        assert_eq!(store.habit_evidence(first.id).unwrap().len(), 3);
        store.reset().unwrap();
        assert!(store.habits().unwrap().is_empty());
    }

    #[test]
    fn project_scoped_habit_rejects_evidence_from_other_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProfileStore::open(&dir.path().join("style.db")).unwrap();
        let claim = store
            .record_habit(
                &habit("project:alpha"),
                &habit_quote("session:a", "task-1", "alpha", "supports"),
            )
            .unwrap();
        assert!(store
            .add_habit_evidence(
                claim.id,
                &habit_quote("session:b", "task-2", "beta", "supports"),
            )
            .is_err());
        assert_eq!(store.habit(claim.id).unwrap().unwrap().episodes, 1);
    }

    #[test]
    fn feedback_stays_a_candidate_until_the_author_decides() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ProfileStore::open(&dir.path().join("style.db")).unwrap();
        let first = s
            .record_feedback(&stated(
                "Never unwrap in library code.",
                "session:a",
                "2026-09-01",
            ))
            .unwrap();
        assert_eq!((first.status.as_str(), first.sources), ("candidate", 1));
        let evidence = s.feedback_evidence(&first.key).unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].attribution, "human_turn");
        assert_eq!(evidence[0].source, "session:a");
        let repeated = s
            .record_feedback(&stated(
                "Never  unwrap in library code.",
                "session:a",
                "2026-09-02",
            ))
            .unwrap();
        assert_eq!(repeated.sources, 1, "the same session is one source");
        let elsewhere = s
            .record_feedback(&stated(
                "Never unwrap in library code.",
                "session:b",
                "2026-08-20",
            ))
            .unwrap();
        assert_eq!(
            (elsewhere.status.as_str(), elsewhere.sources),
            ("candidate", 2),
            "repetition is evidence, not acceptance"
        );
        assert_eq!(s.feedback_evidence(&elsewhere.key).unwrap().len(), 2);
        assert_eq!(
            (elsewhere.first_at.as_str(), elsewhere.last_at.as_str()),
            ("2026-08-20", "2026-09-02")
        );

        assert_eq!(s.set_feedback_status("never-unwrap", "active").unwrap(), 1);
        assert_eq!(s.feedback().unwrap()[0].status, "active");
        assert_eq!(
            s.set_feedback_status("never-unwrap", "rejected").unwrap(),
            1
        );
        let rejected = s
            .record_feedback(&stated(
                "Never unwrap in library code.",
                "session:c",
                "2026-09-03",
            ))
            .unwrap();
        assert_eq!(
            rejected.status, "rejected",
            "a rejection is never overridden"
        );
        assert_eq!(s.set_feedback_status("zzz", "active").unwrap(), 0);
        assert_eq!(s.aggregate().unwrap().feedback.len(), 1);

        s.reset().unwrap();
        assert!(s.feedback().unwrap().is_empty());
    }

    #[test]
    fn feedback_identity_keeps_scope_and_category_separate() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProfileStore::open(&dir.path().join("style.db")).unwrap();
        let global = store
            .record_feedback(&stated("Prefer small PRs", "session:a", "2026-09-01"))
            .unwrap();
        let project = store
            .record_feedback(&NewFeedback {
                statement: "Prefer small PRs",
                category: "code",
                scope: "repo:checkout",
                quote: "Prefer small PRs",
                at: "2026-09-02",
                source: "session:b",
            })
            .unwrap();
        let process = store
            .record_feedback(&NewFeedback {
                statement: "Prefer small PRs",
                category: "process",
                scope: "global",
                quote: "Prefer small PRs",
                at: "2026-09-03",
                source: "session:c",
            })
            .unwrap();
        assert_ne!(global.key, project.key);
        assert_ne!(global.key, process.key);
        assert_ne!(project.key, process.key);
        assert_eq!(store.feedback().unwrap().len(), 3);
        assert_eq!(
            store
                .set_feedback_status("prefer-small-prs", "active")
                .unwrap(),
            3
        );
        assert!(store
            .feedback()
            .unwrap()
            .iter()
            .all(|item| item.status == "candidate"));
    }

    #[test]
    fn old_feedback_keys_migrate_without_losing_review_or_sources() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("style.db");
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch(
                "CREATE TABLE feedback (
                    key TEXT PRIMARY KEY, statement TEXT NOT NULL, category TEXT NOT NULL,
                    scope TEXT NOT NULL, quote TEXT NOT NULL, first_at TEXT NOT NULL,
                    last_at TEXT NOT NULL, status TEXT NOT NULL);
                 CREATE TABLE feedback_source (
                    key TEXT NOT NULL, source TEXT NOT NULL, PRIMARY KEY (key, source));
                 INSERT INTO feedback VALUES (
                    'prefer-small-prs', 'Prefer small PRs', 'process', 'repo:checkout',
                    'Prefer small PRs', '2026-09-01', '2026-09-02', 'active');
                 INSERT INTO feedback_source VALUES ('prefer-small-prs', 'session:a');",
            )
            .unwrap();
        }
        let store = ProfileStore::open(&path).unwrap();
        let entry = store.feedback().unwrap().remove(0);
        assert_eq!(
            entry.key,
            feedback_key("Prefer small PRs", "process", "repo:checkout")
        );
        assert_eq!(entry.status, "active");
        assert_eq!(entry.sources, 1);
        drop(store);
        let reopened = ProfileStore::open(&path).unwrap();
        assert_eq!(reopened.feedback().unwrap().len(), 1);
    }

    #[test]
    fn key_migration_rolls_back_before_exceeding_store_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("style.db");
        let mut db = ProfileStore::open(&path).unwrap();
        {
            let tx = db.conn.transaction().unwrap();
            for i in 0..500 {
                let key = format!("old-{i}");
                let statement = format!("Preference {i} {}", "words ".repeat(27));
                tx.execute("INSERT INTO feedback VALUES (?1,?2,'code','global','legacy quote','2026-09-01','2026-09-01','active')",params![key,statement]).unwrap();
                tx.execute(
                    "INSERT INTO feedback_source VALUES (?1,'session:legacy')",
                    [&key],
                )
                .unwrap();
                tx.execute("INSERT INTO feedback_evidence VALUES (?1,'session:legacy','legacy quote','2026-09-01','human_turn')",[&key]).unwrap();
            }
            tx.commit().unwrap();
        }
        db.conn
            .execute_batch("CREATE TABLE fixture_padding(value BLOB)")
            .unwrap();
        let page_size: u32 = db
            .conn
            .query_row("PRAGMA page_size", [], |r| r.get(0))
            .unwrap();
        db.conn
            .execute_batch(&format!(
                "PRAGMA max_page_count={}",
                MAX_STYLE_STORE_SIZE / u64::from(page_size)
            ))
            .unwrap();
        for size in [1024 * 1024, 4096] {
            loop {
                match db
                    .conn
                    .execute("INSERT INTO fixture_padding VALUES (zeroblob(?1))", [size])
                {
                    Ok(_) => {}
                    Err(rusqlite::Error::SqliteFailure(error, _))
                        if error.code == rusqlite::ErrorCode::DiskFull =>
                    {
                        break
                    }
                    Err(error) => panic!("unexpected padding error: {error}"),
                }
            }
        }
        // Let migration grow so the application size gate, not SQLite's page
        // ceiling, is what must protect the previous readable store.
        db.conn
            .execute_batch("PRAGMA max_page_count=2147483646")
            .unwrap();
        let error = migrate_feedback_keys(&mut db.conn).unwrap_err();
        assert!(error.to_string().contains("64 MiB"), "{error}");
        let legacy: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM feedback WHERE key LIKE 'old-%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(legacy, 500);
        drop(db);
        assert!(std::fs::metadata(&path).unwrap().len() <= MAX_STYLE_STORE_SIZE);
        assert_eq!(
            ProfileStore::open_read_only(&path)
                .unwrap()
                .feedback()
                .unwrap()
                .len(),
            500
        );
    }

    #[test]
    fn line_level_repositories_are_reported_as_legacy_not_counted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("style.db");
        {
            let legacy = Connection::open(&path).unwrap();
            legacy
                .execute_batch(
                    "CREATE TABLE repo (repo_key TEXT PRIMARY KEY, author TEXT NOT NULL,
                         commits_total INTEGER NOT NULL, commits_sampled INTEGER NOT NULL,
                         added_lines_sampled INTEGER NOT NULL, latest_sha TEXT,
                         latest_date TEXT, mined_at_epoch INTEGER NOT NULL DEFAULT 0);
                     CREATE TABLE counter (repo_key TEXT NOT NULL, key TEXT NOT NULL,
                         value INTEGER NOT NULL, PRIMARY KEY (repo_key, key));
                     INSERT INTO repo VALUES ('/old', 'me', 5, 5, 1293, NULL, NULL, 1);
                     INSERT INTO counter VALUES ('/old', 'indent.space', 1065);",
                )
                .unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let read_only = ProfileStore::open_read_only(&path).unwrap();
        let before_upgrade = read_only.aggregate().unwrap();
        assert_eq!((before_upgrade.repos, before_upgrade.legacy_repos), (0, 1));
        assert!(before_upgrade.feedback.is_empty());
        assert!(read_only.feedback().unwrap().is_empty());
        drop(read_only);

        let mut s = ProfileStore::open(&path).unwrap();
        let legacy = s.aggregate().unwrap();
        assert_eq!((legacy.repos, legacy.legacy_repos), (0, 1));
        assert_eq!(legacy.commits_total, 0);
        assert!(legacy.counts.is_empty());

        s.upsert_repo(
            "/new",
            &prov(None, 2),
            &[],
            &commit(&[("indent.tab", 3)]),
            &[],
        )
        .unwrap();
        let mixed = s.aggregate().unwrap();
        assert_eq!((mixed.repos, mixed.legacy_repos), (1, 1));
        assert_eq!(mixed.counts, counts(&[("indent.tab", 3)]));
        assert_eq!(mixed.commits.len(), 1);
    }

    #[test]
    fn reupsert_replaces_not_doubles() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ProfileStore::open(&dir.path().join("style.db")).unwrap();
        s.upsert_repo("/a", &prov(None, 0), &[], &commit(&[("x", 10)]), &[])
            .unwrap();
        s.upsert_repo("/a", &prov(None, 0), &[], &commit(&[("x", 99)]), &[])
            .unwrap();
        let agg = s.aggregate().unwrap();
        assert_eq!(agg.repos, 1);
        assert_eq!(agg.counts["x"], 99);
    }

    #[test]
    fn failed_alias_replacement_keeps_the_previous_contribution() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ProfileStore::open(&dir.path().join("style.db")).unwrap();
        s.upsert_repo(
            "/checkout",
            &prov(Some("old"), 0),
            &["author@example.test".into()],
            &commit(&[("x", 10)]),
            &[],
        )
        .unwrap();
        s.conn
            .execute_batch(
                "CREATE TRIGGER reject_new_key BEFORE INSERT ON repo
                 WHEN NEW.repo_key = '/canonical.git'
                 BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;",
            )
            .unwrap();

        assert!(s
            .upsert_repo(
                "/canonical.git",
                &prov(Some("new"), 1),
                &[],
                &commit(&[("x", 99)]),
                &["/checkout".into()],
            )
            .is_err());
        let aggregate = s.aggregate().unwrap();
        assert_eq!(aggregate.repos, 1);
        assert_eq!(aggregate.counts["x"], 10);
        assert_eq!(aggregate.identities, vec!["author@example.test"]);
        assert_eq!(
            s.repo_latest_sha("/checkout").unwrap().as_deref(),
            Some("old")
        );
    }

    #[test]
    fn failed_force_replacement_rolls_back_reset_and_pruning() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProfileStore::open(&dir.path().join("style.db")).unwrap();
        store
            .upsert_repo(
                "/old",
                &prov(Some("old"), 1),
                &["old@example.test".into()],
                &commit(&[("x", 10)]),
                &[],
            )
            .unwrap();
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_replacement BEFORE INSERT ON repo
                 WHEN NEW.repo_key = '/new'
                 BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;",
            )
            .unwrap();

        assert!(store
            .apply_mine(
                true,
                &["/old".into()],
                "/new",
                &prov(Some("new"), 2),
                &[],
                &commit(&[("x", 99)]),
            )
            .is_err());
        let aggregate = store.aggregate().unwrap();
        assert_eq!(aggregate.repos, 1);
        assert_eq!(aggregate.counts["x"], 10);
        assert_eq!(aggregate.identities, vec!["old@example.test"]);
        assert_eq!(
            store.repo_latest_sha("/old").unwrap().as_deref(),
            Some("old")
        );
    }

    #[test]
    fn reset_wipes_everything() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ProfileStore::open(&dir.path().join("style.db")).unwrap();
        s.upsert_repo("/a", &prov(None, 0), &[], &commit(&[("x", 10)]), &[])
            .unwrap();
        s.reset().unwrap();
        let agg = s.aggregate().unwrap();
        assert_eq!(agg.repos, 0);
        assert!(agg.counts.is_empty());
    }

    #[test]
    fn list_and_prune_retention() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ProfileStore::open(&dir.path().join("style.db")).unwrap();
        s.upsert_repo("/fresh", &prov(None, 1000), &[], &commit(&[("x", 10)]), &[])
            .unwrap();
        s.upsert_repo("/old", &prov(None, 1), &[], &commit(&[("x", 5)]), &[])
            .unwrap();

        let listed = s.list_repos().unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.contains(&("/old".to_string(), 1)));

        // Caller's retention policy: drop repos last mined before epoch 100.
        let drop: Vec<String> = listed
            .iter()
            .filter(|(_, epoch)| *epoch < 100)
            .map(|(key, _)| key.clone())
            .collect();
        assert_eq!(drop, vec!["/old".to_string()]);
        s.prune_repos(&drop).unwrap();

        let agg = s.aggregate().unwrap();
        assert_eq!(agg.repos, 1);
        assert_eq!(agg.counts["x"], 10); // only /fresh survived
    }

    #[test]
    fn open_creates_missing_private_parent_and_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/.mastermind/style.db");
        let store = ProfileStore::open(&path).unwrap();
        assert_eq!(store.aggregate().unwrap().repos, 0);
        assert!(path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn open_rejects_linked_store_and_parent() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let real_parent = dir.path().join("real");
        std::fs::create_dir(&real_parent).unwrap();
        let real_store = real_parent.join("style.db");
        drop(ProfileStore::open(&real_store).unwrap());

        let linked_store = dir.path().join("linked.db");
        symlink(&real_store, &linked_store).unwrap();
        assert!(ProfileStore::open(&linked_store).is_err());
        assert!(ProfileStore::open_read_only(&linked_store).is_err());

        let linked_parent = dir.path().join("linked-parent");
        symlink(&real_parent, &linked_parent).unwrap();
        assert!(ProfileStore::open(&linked_parent.join("other.db")).is_err());
        assert!(!real_parent.join("other.db").exists());
    }

    #[test]
    fn read_only_open_does_not_create_or_mutate_store_state() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing/style.db");
        assert!(ProfileStore::open_read_only(&missing).is_err());
        assert!(!missing.exists());
        assert!(!missing.parent().unwrap().exists());

        let path = dir.path().join("style.db");
        let mut writable = ProfileStore::open(&path).unwrap();
        writable
            .upsert_repo("/repo", &prov(None, 1), &[], &commit(&[("x", 1)]), &[])
            .unwrap();
        drop(writable);
        let read_only = ProfileStore::open_read_only(&path).unwrap();
        assert_eq!(read_only.aggregate().unwrap().counts["x"], 1);
        let query_only: i64 = read_only
            .conn
            .query_row("PRAGMA query_only", [], |row| row.get(0))
            .unwrap();
        assert_eq!(query_only, 1);
    }
}
