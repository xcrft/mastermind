//! `~/.mastermind/style.db` — user-global SQLite store that accumulates per-repo
//! style counts so the author profile enriches across every repo they mine.
//!
//! The codegraph DB (`.mastermind/mmcg.db`) is per-project; this one is global on
//! purpose — "write like me" is one fingerprint summed over all the person's
//! repos. Each mine upserts its repo's contribution (idempotent — re-mining the
//! same repo replaces, never doubles); the rendered profile is the SUM over repos.

use crate::bounded_fs::{BoundedReadError, ReadControl, StableFileIdentity};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Result as SqlResult};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
}

/// Sum across all mined repos — what the rendered profile is built from.
pub struct Aggregate {
    pub repos: usize,
    pub commits_total: i64,
    pub commits_sampled: i64,
    pub added_lines_sampled: i64,
    pub identities: Vec<String>,
    pub counts: Counts,
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
            u64::MAX,
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
        let conn = Connection::open_with_flags(
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
                 mined_at_epoch      INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS counter (
                 repo_key TEXT NOT NULL,
                 key      TEXT NOT NULL,
                 value    INTEGER NOT NULL,
                 PRIMARY KEY (repo_key, key)
             );
             CREATE TABLE IF NOT EXISTS identity (
                 repo_key TEXT NOT NULL,
                 email    TEXT NOT NULL,
                 PRIMARY KEY (repo_key, email)
             );",
        )?;
        verify_store_identity(&root, &target, expected_identity, false)?;
        Ok(Self { conn })
    }

    /// Replace `repo_key`'s contribution and remove its legacy checkout aliases
    /// in one transaction, so re-mining never sums the old and new identities.
    pub fn upsert_repo(
        &mut self,
        repo_key: &str,
        prov: &RepoProvenance,
        identities: &[String],
        counts: &Counts,
        aliases: &[String],
    ) -> SqlResult<()> {
        let tx = self.conn.transaction()?;
        for alias in aliases {
            tx.execute("DELETE FROM counter WHERE repo_key = ?1", params![alias])?;
            tx.execute("DELETE FROM identity WHERE repo_key = ?1", params![alias])?;
            tx.execute("DELETE FROM repo WHERE repo_key = ?1", params![alias])?;
        }
        tx.execute("DELETE FROM counter WHERE repo_key = ?1", params![repo_key])?;
        tx.execute(
            "DELETE FROM identity WHERE repo_key = ?1",
            params![repo_key],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO repo (repo_key, author, commits_total, commits_sampled, \
             added_lines_sampled, latest_sha, latest_date, mined_at_epoch) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                repo_key,
                prov.author,
                prov.commits_total,
                prov.commits_sampled,
                prov.added_lines_sampled,
                prov.latest_sha,
                prov.latest_date,
                prov.mined_at_epoch,
            ],
        )?;
        for (key, value) in counts {
            tx.execute(
                "INSERT INTO counter (repo_key, key, value) VALUES (?1, ?2, ?3)",
                params![repo_key, key, value],
            )?;
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
        self.conn
            .execute_batch("DELETE FROM counter; DELETE FROM identity; DELETE FROM repo;")
    }

    /// Sum counts and provenance across every mined repo.
    pub fn aggregate(&self) -> SqlResult<Aggregate> {
        let mut counts = Counts::new();
        let mut stmt = self
            .conn
            .prepare("SELECT key, SUM(value) FROM counter GROUP BY key")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (k, v) = row?;
            counts.insert(k, v);
        }

        let (repos, commits_total, commits_sampled, added_lines_sampled) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(commits_total), 0), COALESCE(SUM(commits_sampled), 0), \
             COALESCE(SUM(added_lines_sampled), 0) FROM repo",
            [],
            |r| {
                Ok((
                    r.get::<_, i64>(0)? as usize,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )?;

        let mut idstmt = self
            .conn
            .prepare("SELECT DISTINCT email FROM identity ORDER BY email")?;
        let identities: Vec<String> = idstmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<SqlResult<_>>()?;

        Ok(Aggregate {
            repos,
            commits_total,
            commits_sampled,
            added_lines_sampled,
            identities,
            counts,
        })
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
            tx.execute("DELETE FROM counter WHERE repo_key = ?1", params![key])?;
            tx.execute("DELETE FROM identity WHERE repo_key = ?1", params![key])?;
            tx.execute("DELETE FROM repo WHERE repo_key = ?1", params![key])?;
        }
        tx.commit()
    }
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
        u64::MAX,
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

    fn prov(sha: Option<&str>, mined_at_epoch: i64) -> RepoProvenance {
        RepoProvenance {
            author: "me".into(),
            commits_total: 10,
            commits_sampled: 10,
            added_lines_sampled: 100,
            latest_sha: sha.map(String::from),
            latest_date: Some("2026-01-01".into()),
            mined_at_epoch,
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
            &counts(&[("indent.space", 30), ("indent.tab", 2)]),
            &[],
        )
        .unwrap();
        s.upsert_repo(
            "/b",
            &prov(Some("bbb"), 0),
            &["b@x".into()],
            &counts(&[("indent.space", 20), ("indent.tab", 8)]),
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
    fn reupsert_replaces_not_doubles() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ProfileStore::open(&dir.path().join("style.db")).unwrap();
        s.upsert_repo("/a", &prov(None, 0), &[], &counts(&[("x", 10)]), &[])
            .unwrap();
        s.upsert_repo("/a", &prov(None, 0), &[], &counts(&[("x", 99)]), &[])
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
            &counts(&[("x", 10)]),
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
                &counts(&[("x", 99)]),
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
    fn reset_wipes_everything() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ProfileStore::open(&dir.path().join("style.db")).unwrap();
        s.upsert_repo("/a", &prov(None, 0), &[], &counts(&[("x", 10)]), &[])
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
        s.upsert_repo("/fresh", &prov(None, 1000), &[], &counts(&[("x", 10)]), &[])
            .unwrap();
        s.upsert_repo("/old", &prov(None, 1), &[], &counts(&[("x", 5)]), &[])
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

        let linked_parent = dir.path().join("linked-parent");
        symlink(&real_parent, &linked_parent).unwrap();
        assert!(ProfileStore::open(&linked_parent.join("other.db")).is_err());
        assert!(!real_parent.join("other.db").exists());
    }
}
