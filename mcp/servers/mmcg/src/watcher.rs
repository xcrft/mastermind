//! Filesystem watcher — keeps the index fresh as files change.
//!
//! Architecture:
//!   1. Subscribe to recursive filesystem events under `root`
//!   2. Initial full index of `root`
//!   3. Coalesce rapid-fire events per path with a 500ms debounce
//!   4. Re-index files on Modify/Create, purge on Remove
//!
//! Runs in the foreground until stdin closes or Ctrl-C.

use crate::indexer::{
    extractor_for_path, IndexError, IndexStats, Indexer, SourceMatcher, PROJECT_DECISION_DIRS,
};
use crate::store::Store;
use notify::event::{ModifyKind, RenameMode};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(500);
const RX_TIMEOUT: Duration = Duration::from_millis(100);
const RETRY_DELAY: Duration = Duration::from_secs(1);
const MAX_INDEX_ATTEMPTS: u8 = 3;
const MAX_QUEUED_EVENTS: usize = 4096;
const MAX_PENDING_PATHS: usize = 4096;

#[derive(Default)]
struct PendingChanges {
    paths: HashMap<PathBuf, Instant>,
    full_refresh: Option<PendingRefresh>,
}

struct PendingRefresh {
    ready_at: Instant,
    failures: u8,
}

pub fn run(root: PathBuf, mut store: Store) -> Result<(), Box<dyn std::error::Error>> {
    let root = root.canonicalize()?;
    let indexer = Indexer::new(&root);

    let (tx, rx) = mpsc::sync_channel::<notify::Result<notify::Event>>(MAX_QUEUED_EVENTS);
    let overflow = Arc::new(AtomicBool::new(false));
    let callback_overflow = Arc::clone(&overflow);
    let database_path = store.db_path().canonicalize()?;
    let index_files: Vec<PathBuf> = ["", "-wal", "-shm", "-journal"]
        .into_iter()
        .map(|suffix| {
            let mut path = database_path.as_os_str().to_os_string();
            path.push(suffix);
            PathBuf::from(path)
        })
        .collect();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |event| {
        queue_event(event, &index_files, &tx, &callback_overflow);
    })?;
    watcher.watch(&root, RecursiveMode::Recursive)?;
    let mut source_matcher = SourceMatcher::new(&root);

    let mut pending_changes = PendingChanges::default();
    match refresh_index(&indexer, &mut store) {
        Ok(stats) => eprintln!(
            "[mastermind watch] initial: indexed {} (unchanged {}, purged {}) in {} ms",
            stats.files_indexed, stats.files_unchanged, stats.files_purged, stats.duration_ms
        ),
        Err(error) => record_failure(&mut pending_changes, Instant::now(), error)?,
    }
    eprintln!("[mastermind watch] watching {}", root.display());

    loop {
        if overflow.load(Ordering::Relaxed) {
            return Err(std::io::Error::other(
                "[mastermind watch] event queue overflow, index may be stale; run mastermind index and restart watch",
            )
            .into());
        }
        // Short timeout so we can also flush pending changes between events.
        match rx.recv_timeout(RX_TIMEOUT) {
            Ok(Ok(event)) => handle_event(
                event,
                &root,
                &indexer,
                &mut source_matcher,
                &mut pending_changes,
                &mut store,
            )?,
            Ok(Err(error)) => {
                return Err(std::io::Error::other(format!(
                    "[mastermind watch] notify error: {error}; index may be stale, run mastermind index and restart watch"
                )).into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                eprintln!("[mastermind watch] watcher channel closed");
                break;
            }
        }

        flush_settled(&indexer, &mut store, &mut pending_changes, Instant::now())?;
    }

    Ok(())
}

fn queue_event(
    mut event: notify::Result<notify::Event>,
    index_files: &[PathBuf],
    tx: &mpsc::SyncSender<notify::Result<notify::Event>>,
    overflow: &AtomicBool,
) {
    if let Ok(event) = &mut event {
        if matches!(event.kind, EventKind::Access(_)) && !event.need_rescan() {
            return;
        }
        event.paths.retain(|path| !index_files.contains(path));
        if event.paths.is_empty() && !event.need_rescan() {
            return;
        }
        if event.paths.len() > MAX_PENDING_PATHS {
            overflow.store(true, Ordering::Relaxed);
            return;
        }
    }
    if matches!(tx.try_send(event), Err(mpsc::TrySendError::Full(_))) {
        overflow.store(true, Ordering::Relaxed);
    }
}

fn handle_event(
    event: notify::Event,
    root: &Path,
    indexer: &Indexer,
    source_matcher: &mut SourceMatcher,
    pending: &mut PendingChanges,
    store: &mut Store,
) -> std::io::Result<()> {
    if event.need_rescan() {
        return Err(std::io::Error::other(
            "[mastermind watch] filesystem events were lost; index may be stale, run mastermind index and restart watch",
        ));
    }
    if matches!(event.kind, EventKind::Access(_)) {
        return Ok(());
    }
    let may_replace_directory = matches!(
        event.kind,
        EventKind::Create(notify::event::CreateKind::Folder)
            | EventKind::Create(notify::event::CreateKind::Any)
            | EventKind::Remove(_)
            | EventKind::Modify(ModifyKind::Name(_))
            | EventKind::Any
    );
    if pending.full_refresh.is_none()
        && event.paths.iter().any(|path| {
            is_project_history_path(path, root)
                || (may_replace_directory && is_project_history_container(path, root))
        })
    {
        match indexer.index_project_history(store) {
            Ok(stats) => eprintln!(
                "[mastermind watch] refreshed {} history entries (skipped {}, truncated {})",
                stats.indexed, stats.skipped, stats.truncated
            ),
            Err(error) => record_failure(pending, Instant::now(), error)?,
        }
    }

    if event.paths.iter().any(|path| is_ignore_config(path, root)) {
        *source_matcher = SourceMatcher::new(root);
        if pending.full_refresh.is_none() {
            refresh_all(indexer, store, pending, Instant::now())?;
        }
        return Ok(());
    }
    if pending.full_refresh.is_some() {
        return Ok(());
    }

    match event.kind {
        EventKind::Modify(ModifyKind::Name(rename_mode)) => {
            let paths = event.paths;
            let indexed_paths = match store.indexed_paths() {
                Ok(paths) => paths,
                Err(error) => {
                    return record_failure(pending, Instant::now(), error);
                }
            };
            let mut purge_paths = Vec::new();
            let mut full_refresh = indexed_paths.is_empty()
                || matches!(
                    rename_mode,
                    RenameMode::To | RenameMode::Any | RenameMode::Other
                );

            for (position, path) in paths.iter().enumerate() {
                pending.paths.remove(path);
                if path.is_dir() {
                    full_refresh = true;
                }
                if path.exists() && !rename_event_path_is_old(&rename_mode, position) {
                    continue;
                }
                if let Some(rel) = relative_path(path, root) {
                    let descendant_prefix = format!("{rel}/");
                    let mut matched = false;
                    for indexed in &indexed_paths {
                        if indexed == &rel || indexed.starts_with(&descendant_prefix) {
                            if indexed.starts_with(&descendant_prefix) {
                                full_refresh = true;
                            }
                            purge_paths.push(indexed.clone());
                            matched = true;
                        }
                    }
                    if !matched {
                        purge_paths.push(rel);
                    }
                }
            }
            purge_paths.sort();
            purge_paths.dedup();
            for rel in purge_paths {
                if let Err(error) = store.purge_file(&rel) {
                    return record_failure(
                        pending,
                        Instant::now(),
                        format!("rename purge failed {rel}: {error}"),
                    );
                } else {
                    eprintln!("[mastermind watch] rename purged {rel}");
                }
            }

            if full_refresh {
                return refresh_all(indexer, store, pending, Instant::now());
            }

            for (position, path) in paths.into_iter().enumerate() {
                if rename_event_path_is_old(&rename_mode, position) {
                    continue;
                }
                if path.is_file()
                    && extractor_for_path(&path).is_some()
                    && !source_matcher.is_ignored(&path, false)
                {
                    queue_change(pending, path, Instant::now())?;
                }
            }
        }
        EventKind::Modify(_) | EventKind::Create(_) => {
            for path in event.paths {
                if path.is_file()
                    && extractor_for_path(&path).is_some()
                    && !source_matcher.is_ignored(&path, false)
                {
                    queue_change(pending, path, Instant::now())?;
                }
            }
        }
        EventKind::Remove(_) => {
            let indexed_paths = match store.indexed_paths() {
                Ok(paths) => paths,
                Err(error) => {
                    return record_failure(pending, Instant::now(), error);
                }
            };
            let mut purge_paths = Vec::new();
            for path in event.paths {
                // Drop descendants too — a directory remove otherwise leaves
                // debounced child paths able to re-enter the index.
                pending
                    .paths
                    .retain(|candidate, _| !candidate.starts_with(&path));
                if let Some(rel) = relative_path(&path, root) {
                    let descendant_prefix = format!("{rel}/");
                    let mut matched = false;
                    for indexed in &indexed_paths {
                        if indexed == &rel || indexed.starts_with(&descendant_prefix) {
                            purge_paths.push(indexed.clone());
                            matched = true;
                        }
                    }
                    if !matched {
                        purge_paths.push(rel);
                    }
                }
            }
            purge_paths.sort();
            purge_paths.dedup();
            for rel in purge_paths {
                if let Err(error) = store.purge_file(&rel) {
                    return record_failure(
                        pending,
                        Instant::now(),
                        format!("purge failed {rel}: {error}"),
                    );
                } else {
                    eprintln!("[mastermind watch] purged {rel}");
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn queue_change(pending: &mut PendingChanges, path: PathBuf, now: Instant) -> std::io::Result<()> {
    if pending.full_refresh.is_some() {
        return Ok(());
    }
    if pending.paths.len() >= MAX_PENDING_PATHS && !pending.paths.contains_key(&path) {
        return Err(std::io::Error::other(
            "[mastermind watch] pending path limit exceeded, index may be stale; run mastermind index and restart watch",
        ));
    }
    pending.paths.insert(path, now + DEBOUNCE);
    Ok(())
}

fn refresh_index(indexer: &Indexer, store: &mut Store) -> Result<IndexStats, IndexError> {
    let stats = indexer.index_all(store, false)?;
    if stats.files_failed > 0 {
        return Err(IndexError::Other(format!(
            "refresh failed for {} file(s)",
            stats.files_failed
        )));
    }
    if !store
        .concept_contract_current()
        .map_err(|error| IndexError::Other(error.to_string()))?
    {
        return Err(IndexError::Other(
            "refreshed corpus is still stale".to_string(),
        ));
    }
    Ok(stats)
}

fn refresh_all(
    indexer: &Indexer,
    store: &mut Store,
    pending: &mut PendingChanges,
    now: Instant,
) -> std::io::Result<()> {
    match refresh_index(indexer, store) {
        Ok(stats) => {
            pending.paths.clear();
            pending.full_refresh = None;
            eprintln!(
                "[mastermind watch] refreshed: indexed {} (unchanged {}, purged {})",
                stats.files_indexed, stats.files_unchanged, stats.files_purged
            );
            Ok(())
        }
        Err(error) => record_failure(pending, now, error),
    }
}

fn record_failure(
    pending: &mut PendingChanges,
    now: Instant,
    error: impl std::fmt::Display,
) -> std::io::Result<()> {
    // A full refresh subsumes file retries and restores contracts dirtied by failed writes.
    pending.paths.clear();
    let refresh = pending.full_refresh.get_or_insert(PendingRefresh {
        ready_at: now,
        failures: 0,
    });
    refresh.failures += 1;
    if refresh.failures >= MAX_INDEX_ATTEMPTS {
        pending.full_refresh = None;
        return Err(std::io::Error::other(format!(
            "[mastermind watch] retry limit exhausted after {MAX_INDEX_ATTEMPTS} attempts: {error}; run mastermind index and restart watch",
        )));
    }
    refresh.ready_at = now + RETRY_DELAY;
    eprintln!(
        "[mastermind watch] failed (attempt {}/{MAX_INDEX_ATTEMPTS}), retrying full refresh: {error}",
        refresh.failures
    );
    Ok(())
}

fn flush_settled(
    indexer: &Indexer,
    store: &mut Store,
    pending: &mut PendingChanges,
    now: Instant,
) -> std::io::Result<()> {
    if let Some(refresh) = &pending.full_refresh {
        return if refresh.ready_at <= now {
            refresh_all(indexer, store, pending, now)
        } else {
            Ok(())
        };
    }
    let ready: Vec<PathBuf> = pending
        .paths
        .iter()
        .filter(|(_, ready_at)| **ready_at <= now)
        .map(|(path, _)| path.clone())
        .collect();

    for path in ready {
        if !path.is_file() {
            return refresh_all(indexer, store, pending, now);
        }
        match indexer.index_one(store, &path) {
            Ok(()) => {
                pending.paths.remove(&path);
                eprintln!("[mastermind watch] reindexed {}", path.display());
            }
            Err(IndexError::Skipped(reason)) => {
                pending.paths.remove(&path);
                eprintln!("[mastermind watch] skipped {}: {reason:?}", path.display());
            }
            Err(error) => {
                return record_failure(pending, now, format!("{}: {error}", path.display()));
            }
        }
    }
    Ok(())
}

fn relative_path(absolute: &Path, root: &Path) -> Option<String> {
    crate::indexer::repository_relative_path(root, absolute).ok()
}

fn rename_event_path_is_old(mode: &RenameMode, position: usize) -> bool {
    matches!(mode, &RenameMode::From) || matches!(mode, &RenameMode::Both) && position == 0
}

fn is_ignore_config(path: &Path, root: &Path) -> bool {
    if path
        .file_name()
        .is_some_and(|name| name == ".gitignore" || name == ".ignore")
    {
        return path.starts_with(root);
    }
    path == root.join(".git/info/exclude")
}

fn is_project_history_path(path: &Path, root: &Path) -> bool {
    let is_context = path == root.join("CONTEXT.md")
        || (path.parent() == Some(root)
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("CONTEXT-archive-") && name.ends_with(".md")));
    is_context
        || path.starts_with(root.join(".mastermind").join("tasks"))
        || path.starts_with(root.join(".mastermind").join("releases"))
        || PROJECT_DECISION_DIRS
            .iter()
            .any(|relative| path.starts_with(root.join(relative)))
}

fn is_project_history_container(path: &Path, root: &Path) -> bool {
    path != root
        && path.starts_with(root)
        && PROJECT_DECISION_DIRS
            .iter()
            .copied()
            .chain([".mastermind/tasks", ".mastermind/releases"])
            .any(|relative| root.join(relative).starts_with(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_write_failure_is_retried_without_another_event() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("changing.rs");
        let database = root.join("index.db");
        std::fs::write(&source, "pub fn before_failure() {}\n").unwrap();
        let mut store = Store::open(&database).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();
        std::fs::write(&source, "pub fn after_recovery() {}\n").unwrap();
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_watched_update BEFORE INSERT ON symbols BEGIN
                 SELECT RAISE(ABORT, 'injected transient write failure');
             END;",
            )
            .unwrap();
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = PendingChanges::default();
        handle_event(
            notify::Event::new(EventKind::Modify(ModifyKind::Any)).add_path(source.clone()),
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        )
        .unwrap();
        let settled = pending.paths[&source];
        flush_settled(&indexer, &mut store, &mut pending, settled).unwrap();
        assert!(pending.paths.is_empty());
        assert!(pending.full_refresh.is_some());
        assert_eq!(pending.full_refresh.as_ref().unwrap().failures, 1);
        assert_eq!(
            store
                .search_symbols("before_failure", None, None)
                .unwrap()
                .len(),
            1
        );
        assert!(store
            .search_symbols("after_recovery", None, None)
            .unwrap()
            .is_empty());

        connection
            .execute_batch("DROP TRIGGER reject_watched_update")
            .unwrap();
        flush_settled(
            &indexer,
            &mut store,
            &mut pending,
            settled + RETRY_DELAY - Duration::from_nanos(1),
        )
        .unwrap();
        assert!(pending.paths.is_empty());
        assert!(pending.full_refresh.is_some());
        assert!(store
            .search_symbols("after_recovery", None, None)
            .unwrap()
            .is_empty());
        flush_settled(&indexer, &mut store, &mut pending, settled + RETRY_DELAY).unwrap();
        assert!(pending.paths.is_empty() && pending.full_refresh.is_none());
        assert!(store
            .search_symbols("before_failure", None, None)
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .search_symbols("after_recovery", None, None)
                .unwrap()
                .len(),
            1
        );
        assert!(store.concept_contract_current().unwrap());
    }

    #[test]
    fn persistent_write_failure_exhausts_the_retry_budget() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("changing.rs");
        let database = root.join("index.db");
        std::fs::write(&source, "pub fn before_failure() {}\n").unwrap();
        let mut store = Store::open(&database).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();
        std::fs::write(&source, "pub fn after_recovery() {}\n").unwrap();
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_watched_update BEFORE INSERT ON symbols BEGIN
                 SELECT RAISE(ABORT, 'injected persistent write failure');
             END;",
            )
            .unwrap();
        let mut pending = PendingChanges::default();
        let now = Instant::now();
        queue_change(&mut pending, source.clone(), now).unwrap();
        for attempt in 1..MAX_INDEX_ATTEMPTS {
            flush_settled(
                &indexer,
                &mut store,
                &mut pending,
                now + DEBOUNCE + RETRY_DELAY * u32::from(attempt - 1),
            )
            .unwrap();
            assert_eq!(pending.full_refresh.as_ref().unwrap().failures, attempt);
        }
        let exhausted = now + DEBOUNCE + RETRY_DELAY * u32::from(MAX_INDEX_ATTEMPTS - 1);
        let error = flush_settled(&indexer, &mut store, &mut pending, exhausted).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("retry limit exhausted"));
        assert!(message.contains("after 3 attempts"));
        assert!(message.contains("refresh failed for 1 file(s)"));
        assert!(pending.paths.is_empty() && pending.full_refresh.is_none());
        connection
            .execute_batch("DROP TRIGGER reject_watched_update")
            .unwrap();
        flush_settled(&indexer, &mut store, &mut pending, exhausted + RETRY_DELAY).unwrap();
        assert_eq!(
            store
                .search_symbols("before_failure", None, None)
                .unwrap()
                .len(),
            1
        );
        assert!(store
            .search_symbols("after_recovery", None, None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rename_and_ignore_events_preserve_a_failed_update_until_full_recovery() {
        for rename in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let source = root.join("changing.rs");
            let excluded = root.join("excluded.rs");
            let database = root.join("index.db");
            std::fs::write(&source, "pub fn before_failure() {}\n").unwrap();
            std::fs::write(&excluded, "pub fn excluded_symbol() {}\n").unwrap();
            let mut store = Store::open(&database).unwrap();
            let indexer = Indexer::new(&root);
            indexer.index_all(&mut store, false).unwrap();
            std::fs::write(&source, "pub fn after_recovery() {}\n").unwrap();
            let connection = rusqlite::Connection::open(&database).unwrap();
            connection
                .execute_batch(
                    "CREATE TRIGGER reject_watched_update BEFORE INSERT ON symbols BEGIN
                     SELECT RAISE(ABORT, 'injected transient write failure');
                 END;",
                )
                .unwrap();
            let mut matcher = SourceMatcher::new(&root);
            let mut pending = PendingChanges::default();
            let now = Instant::now();
            queue_change(&mut pending, source.clone(), now).unwrap();
            flush_settled(&indexer, &mut store, &mut pending, now + DEBOUNCE).unwrap();
            let retry_at = pending.full_refresh.as_ref().unwrap().ready_at;
            let event = if rename {
                let renamed = root.join("renamed.rs");
                std::fs::rename(&source, &renamed).unwrap();
                notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
                    .add_path(source.clone())
                    .add_path(renamed)
            } else {
                let ignore = root.join(".gitignore");
                std::fs::write(&ignore, "excluded.rs\n").unwrap();
                notify::Event::new(EventKind::Modify(ModifyKind::Any)).add_path(ignore)
            };
            handle_event(
                event,
                &root,
                &indexer,
                &mut matcher,
                &mut pending,
                &mut store,
            )
            .unwrap();
            assert_eq!(pending.full_refresh.as_ref().unwrap().ready_at, retry_at);
            assert_eq!(pending.full_refresh.as_ref().unwrap().failures, 1);
            connection
                .execute_batch("DROP TRIGGER reject_watched_update")
                .unwrap();
            flush_settled(&indexer, &mut store, &mut pending, retry_at).unwrap();
            assert!(pending.paths.is_empty() && pending.full_refresh.is_none());
            assert!(store
                .search_symbols("before_failure", None, None)
                .unwrap()
                .is_empty());
            let recovered = store.search_symbols("after_recovery", None, None).unwrap();
            assert_eq!(recovered.len(), 1);
            if rename {
                assert_eq!(recovered[0].file_path, "renamed.rs");
                assert!(store.symbols_in_file("changing.rs").unwrap().is_empty());
            } else {
                assert!(store.symbols_in_file("excluded.rs").unwrap().is_empty());
                assert!(matcher.is_ignored(&excluded, false));
            }
            assert!(store.concept_contract_current().unwrap());
        }
    }

    #[test]
    fn failed_removal_is_purged_by_the_retained_full_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("removed.rs");
        let database = root.join("index.db");
        std::fs::write(&source, "pub fn removed_handler() {}\n").unwrap();
        let mut store = Store::open(&database).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_watched_removal BEFORE DELETE ON symbols BEGIN
                 SELECT RAISE(ABORT, 'injected transient purge failure');
             END;",
            )
            .unwrap();
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = PendingChanges::default();
        queue_change(&mut pending, source.clone(), Instant::now()).unwrap();
        std::fs::remove_file(&source).unwrap();
        handle_event(
            notify::Event::new(EventKind::Remove(notify::event::RemoveKind::File)).add_path(source),
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        )
        .unwrap();
        assert_eq!(pending.full_refresh.as_ref().unwrap().failures, 1);
        assert!(!store.symbols_in_file("removed.rs").unwrap().is_empty());
        connection
            .execute_batch("DROP TRIGGER reject_watched_removal")
            .unwrap();
        let retry_at = pending.full_refresh.as_ref().unwrap().ready_at;
        flush_settled(&indexer, &mut store, &mut pending, retry_at).unwrap();
        assert!(pending.paths.is_empty() && pending.full_refresh.is_none());
        assert!(store.symbols_in_file("removed.rs").unwrap().is_empty());
        assert!(store.search_concepts("\"removed\"", 10).unwrap().is_empty());
        assert!(store.concept_contract_current().unwrap());
    }

    #[test]
    fn history_write_failure_uses_the_same_retained_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let database = root.join("index.db");
        let context = root.join("CONTEXT.md");
        let body = |decision: &str| {
            format!(
            "# Context\n## Decision log\n### Storage\n- **Decision:** {decision}\n- **Status:** active\n"
        )
        };
        std::fs::write(&context, body("Keep ephemeral storage.")).unwrap();
        let mut store = Store::open(&database).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_history_update BEFORE INSERT ON project_claim BEGIN
                 SELECT RAISE(ABORT, 'injected transient history failure');
             END;",
            )
            .unwrap();
        std::fs::write(&context, body("Keep durable storage.")).unwrap();
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = PendingChanges::default();
        handle_event(
            notify::Event::new(EventKind::Modify(ModifyKind::Any)).add_path(context),
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        )
        .unwrap();
        assert_eq!(pending.full_refresh.as_ref().unwrap().failures, 1);
        connection
            .execute_batch("DROP TRIGGER reject_history_update")
            .unwrap();
        let retry_at = pending.full_refresh.as_ref().unwrap().ready_at;
        flush_settled(&indexer, &mut store, &mut pending, retry_at).unwrap();
        assert!(pending.full_refresh.is_none());
        assert!(store
            .search_project_history("ephemeral", Some("context"), 10)
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .search_project_history("durable", Some("context"), 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn empty_rescan_event_survives_queue_filtering_and_refuses_silent_continuation() {
        let (tx, rx) = mpsc::sync_channel(1);
        let overflow = AtomicBool::new(false);
        queue_event(
            Ok(notify::Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan)),
            &[],
            &tx,
            &overflow,
        );
        let event = rx.try_recv().unwrap().unwrap();
        assert!(event.need_rescan());
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let indexer = Indexer::new(&root);
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = PendingChanges::default();
        let mut store = Store::open(root.join("index.db")).unwrap();
        let error = handle_event(
            event,
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        )
        .unwrap_err();
        assert!(error.to_string().contains("filesystem events were lost"));
        assert!(error.to_string().contains("restart watch"));
    }

    #[test]
    fn pending_path_capacity_allows_updates_but_rejects_new_paths() {
        let now = Instant::now();
        let mut pending = PendingChanges::default();
        for position in 0..MAX_PENDING_PATHS {
            queue_change(&mut pending, PathBuf::from(format!("{position}.rs")), now).unwrap();
        }
        queue_change(&mut pending, PathBuf::from("0.rs"), now + DEBOUNCE).unwrap();
        let error = queue_change(&mut pending, PathBuf::from("overflow.rs"), now).unwrap_err();
        assert!(error.to_string().contains("pending path limit exceeded"));
        assert_eq!(pending.paths.len(), MAX_PENDING_PATHS);
        assert!(!pending.paths.contains_key(Path::new("overflow.rs")));
    }

    #[test]
    fn index_database_events_are_filtered_before_bounded_queue_admission() {
        let index_files: Vec<PathBuf> = [
            "index.db",
            "index.db-wal",
            "index.db-shm",
            "index.db-journal",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        let (tx, rx) = mpsc::sync_channel(1);
        let overflow = AtomicBool::new(false);
        for _ in 0..MAX_QUEUED_EVENTS {
            queue_event(
                Ok(notify::Event::new(EventKind::Modify(ModifyKind::Any))
                    .add_path(index_files[1].clone())),
                &index_files,
                &tx,
                &overflow,
            );
        }
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert!(!overflow.load(Ordering::Relaxed));
        let history = PathBuf::from(".mastermind/tasks/001-task/spec.md");
        queue_event(
            Ok(notify::Event::new(EventKind::Modify(ModifyKind::Any))
                .add_path(index_files[0].clone())
                .add_path(history.clone())),
            &index_files,
            &tx,
            &overflow,
        );
        assert_eq!(rx.try_recv().unwrap().unwrap().paths, vec![history]);
        for _ in 0..2 {
            queue_event(
                Ok(notify::Event::new(EventKind::Modify(ModifyKind::Any))
                    .add_path(PathBuf::from("source.rs"))),
                &index_files,
                &tx,
                &overflow,
            );
        }
        assert!(overflow.load(Ordering::Relaxed));
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn removal_paths_do_not_alias_literal_backslashes() {
        let root = Path::new("/repo");
        assert_eq!(
            relative_path(Path::new("/repo/src/pay.rs"), root).as_deref(),
            Some("src/pay.rs")
        );
        assert_eq!(relative_path(Path::new("/repo/src\\pay.rs"), root), None);
    }

    #[test]
    fn changing_ignore_rules_rebuilds_matcher_and_purges_newly_ignored_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("ignored.rs");
        std::fs::write(&source, "pub fn indexed() {}\n").unwrap();
        std::fs::create_dir_all(root.join(".mastermind")).unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();
        assert_eq!(store.indexed_paths().unwrap(), vec!["ignored.rs"]);

        let mut matcher = SourceMatcher::new(&root);
        let mut pending = PendingChanges::default();
        let ignore_file = root.join(".gitignore");
        std::fs::write(&ignore_file, "ignored.rs\n").unwrap();
        let event = notify::Event::new(EventKind::Modify(ModifyKind::Any)).add_path(ignore_file);

        handle_event(
            event,
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        )
        .unwrap();

        assert!(pending.paths.is_empty() && pending.full_refresh.is_none());
        assert!(matcher.is_ignored(&source, false));
        assert!(store.indexed_paths().unwrap().is_empty());
    }

    #[test]
    fn history_markdown_events_refresh_the_derived_corpus() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".mastermind/tasks")).unwrap();
        let context = root.join("CONTEXT.md");
        std::fs::write(&context, "# Context\n\nInitial decision.\n").unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();

        std::fs::write(&context, "# Context\n\nUse durable idempotency keys.\n").unwrap();
        let event = notify::Event::new(EventKind::Modify(ModifyKind::Any)).add_path(context);
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = PendingChanges::default();
        handle_event(
            event,
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        )
        .unwrap();

        assert_eq!(
            store
                .search_project_history("idempotency", Some("context"), 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn nested_decision_create_edit_rename_and_remove_refresh_history() {
        for relative in PROJECT_DECISION_DIRS {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            std::fs::create_dir_all(root.join(".mastermind")).unwrap();
            let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
            let indexer = Indexer::new(&root);
            indexer.index_all(&mut store, false).unwrap();
            let mut matcher = SourceMatcher::new(&root);
            let mut pending = PendingChanges::default();
            let path = root.join(relative).join("nested/001-storage.md");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "# Storage\n\nUse ephemeral state.\n").unwrap();
            handle_event(
                notify::Event::new(EventKind::Create(notify::event::CreateKind::File))
                    .add_path(path.clone()),
                &root,
                &indexer,
                &mut matcher,
                &mut pending,
                &mut store,
            )
            .unwrap();
            assert_eq!(
                store
                    .search_project_history("ephemeral", Some("architecture_decision"), 10)
                    .unwrap()
                    .len(),
                1,
                "create under {relative}"
            );

            std::fs::write(&path, "# Storage\n\nUse durable state.\n").unwrap();
            handle_event(
                notify::Event::new(EventKind::Modify(ModifyKind::Any)).add_path(path.clone()),
                &root,
                &indexer,
                &mut matcher,
                &mut pending,
                &mut store,
            )
            .unwrap();
            assert!(store
                .search_project_history("ephemeral", Some("architecture_decision"), 10)
                .unwrap()
                .is_empty());
            let hits = store
                .search_project_history("durable", Some("architecture_decision"), 10)
                .unwrap();
            assert_eq!(hits.len(), 1, "edit under {relative}");

            let renamed = path.with_file_name("002-storage.md");
            std::fs::rename(&path, &renamed).unwrap();
            handle_event(
                notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
                    .add_path(path)
                    .add_path(renamed.clone()),
                &root,
                &indexer,
                &mut matcher,
                &mut pending,
                &mut store,
            )
            .unwrap();
            let hits = store
                .search_project_history("durable", Some("architecture_decision"), 10)
                .unwrap();
            assert_eq!(hits.len(), 1, "rename under {relative}");
            assert_eq!(hits[0].path, format!("{relative}/nested/002-storage.md"));

            std::fs::remove_file(&renamed).unwrap();
            handle_event(
                notify::Event::new(EventKind::Remove(notify::event::RemoveKind::File))
                    .add_path(renamed),
                &root,
                &indexer,
                &mut matcher,
                &mut pending,
                &mut store,
            )
            .unwrap();
            assert!(store
                .search_project_history("durable", Some("architecture_decision"), 10)
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn removing_decision_parent_directory_purges_history() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".mastermind")).unwrap();
        std::fs::create_dir_all(root.join("docs/adr")).unwrap();
        std::fs::write(root.join("docs/adr/001.md"), "# Durable state\n").unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();
        assert_eq!(
            store
                .search_project_history("durable", Some("architecture_decision"), 10)
                .unwrap()
                .len(),
            1
        );

        std::fs::remove_dir_all(root.join("docs")).unwrap();
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = PendingChanges::default();
        handle_event(
            notify::Event::new(EventKind::Remove(notify::event::RemoveKind::Folder))
                .add_path(root.join("docs")),
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        )
        .unwrap();
        assert!(store
            .search_project_history("durable", Some("architecture_decision"), 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn reading_history_does_not_trigger_another_index_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".mastermind")).unwrap();
        let path = root.join("CONTEXT.md");
        std::fs::write(&path, "# Initial context\n").unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();
        std::fs::write(&path, "# Changed context\n").unwrap();
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = PendingChanges::default();
        handle_event(
            notify::Event::new(EventKind::Access(notify::event::AccessKind::Read)).add_path(path),
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        )
        .unwrap();
        assert_eq!(
            store
                .search_project_history("initial", Some("context"), 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn rename_event_purges_old_graph_and_concept_path_before_reindexing_new_path() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".mastermind")).unwrap();
        let old_path = root.join("old_handler.rs");
        let new_path = root.join("new_handler.rs");
        std::fs::write(&old_path, "pub fn renamed_handler() {}\n").unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();
        assert_eq!(store.search_concepts("\"old\"", 10).unwrap().len(), 2);

        std::fs::rename(&old_path, &new_path).unwrap();
        let event = notify::Event::new(EventKind::Modify(ModifyKind::Name(
            notify::event::RenameMode::Both,
        )))
        .add_path(old_path.clone())
        .add_path(new_path.clone());
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = PendingChanges::default();
        handle_event(
            event,
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        )
        .unwrap();

        assert!(store.symbols_in_file("old_handler.rs").unwrap().is_empty());
        assert!(store.search_concepts("\"old\"", 10).unwrap().is_empty());
        assert!(pending.paths.contains_key(&new_path));
        let now = Instant::now();
        queue_change(&mut pending, new_path, now).unwrap();
        flush_settled(&indexer, &mut store, &mut pending, now + DEBOUNCE).unwrap();

        assert_eq!(store.symbols_in_file("new_handler.rs").unwrap().len(), 2);
        assert_eq!(store.search_concepts("\"new\"", 10).unwrap().len(), 2);
        assert!(store.concept_contract_current().unwrap());
    }

    #[test]
    fn directory_rename_purges_all_old_paths_and_indexes_the_new_tree() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("old/nested")).unwrap();
        std::fs::create_dir_all(root.join(".mastermind")).unwrap();
        std::fs::write(root.join("old/first.rs"), "pub fn first_handler() {}\n").unwrap();
        std::fs::write(
            root.join("old/nested/second.py"),
            "def second_handler(): pass\n",
        )
        .unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();

        let old_path = root.join("old");
        let new_path = root.join("new");
        std::fs::rename(&old_path, &new_path).unwrap();
        let event = notify::Event::new(EventKind::Modify(ModifyKind::Name(
            notify::event::RenameMode::Both,
        )))
        .add_path(old_path)
        .add_path(new_path);
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = PendingChanges::default();
        handle_event(
            event,
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        )
        .unwrap();

        assert!(store.symbols_in_file("old/first.rs").unwrap().is_empty());
        assert!(store
            .symbols_in_file("old/nested/second.py")
            .unwrap()
            .is_empty());
        assert_eq!(store.symbols_in_file("new/first.rs").unwrap().len(), 2);
        assert_eq!(
            store.symbols_in_file("new/nested/second.py").unwrap().len(),
            2
        );
        assert!(pending.paths.is_empty() && pending.full_refresh.is_none());
        assert!(store.concept_contract_current().unwrap());
    }

    #[test]
    fn rename_to_event_runs_inventory_refresh_when_the_old_path_is_absent() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".mastermind")).unwrap();
        let old_path = root.join("old.rs");
        let new_path = root.join("new.rs");
        std::fs::write(&old_path, "pub fn moved_handler() {}\n").unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();

        std::fs::rename(&old_path, &new_path).unwrap();
        let event = notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::To)))
            .add_path(new_path);
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = PendingChanges::default();
        handle_event(
            event,
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        )
        .unwrap();

        assert!(store.symbols_in_file("old.rs").unwrap().is_empty());
        assert_eq!(store.symbols_in_file("new.rs").unwrap().len(), 2);
        assert!(pending.paths.is_empty() && pending.full_refresh.is_none());
        assert!(store.concept_contract_current().unwrap());
    }

    #[test]
    fn rename_mode_positions_identify_case_only_old_paths_without_stat() {
        assert!(rename_event_path_is_old(&RenameMode::From, 0));
        assert!(rename_event_path_is_old(&RenameMode::Both, 0));
        assert!(!rename_event_path_is_old(&RenameMode::Both, 1));
        assert!(!rename_event_path_is_old(&RenameMode::To, 0));
    }

    #[test]
    fn case_only_rename_never_requeues_the_old_spelling_when_it_still_stats() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".mastermind")).unwrap();
        let old_path = root.join("Handler.rs");
        let new_path = root.join("handler.rs");
        std::fs::write(&old_path, "pub fn renamed_handler() {}\n").unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();

        // Keep both spellings present to reproduce the observable stat result
        // of a case-insensitive filesystem without making the test platform-specific.
        std::fs::write(&new_path, "pub fn renamed_handler() {}\n").unwrap();
        let event = notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(old_path.clone())
            .add_path(new_path.clone());
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = PendingChanges::default();
        handle_event(
            event,
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        )
        .unwrap();

        assert!(old_path.is_file());
        assert!(!pending.paths.contains_key(&old_path));
        assert!(pending.paths.contains_key(&new_path));
        let now = Instant::now();
        queue_change(&mut pending, new_path, now).unwrap();
        flush_settled(&indexer, &mut store, &mut pending, now + DEBOUNCE).unwrap();
        assert!(store.symbols_in_file("Handler.rs").unwrap().is_empty());
        assert_eq!(store.symbols_in_file("handler.rs").unwrap().len(), 2);
        assert!(store.concept_contract_current().unwrap());
    }

    #[test]
    fn directory_remove_event_purges_descendant_graph_and_concept_rows() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let removed = root.join("removed");
        std::fs::create_dir_all(removed.join("nested")).unwrap();
        std::fs::create_dir_all(root.join(".mastermind")).unwrap();
        std::fs::write(removed.join("first.rs"), "pub fn first_handler() {}\n").unwrap();
        std::fs::write(
            removed.join("nested/second.py"),
            "def second_handler(): pass\n",
        )
        .unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();
        assert_eq!(store.concept_count().unwrap(), 4);

        std::fs::remove_dir_all(&removed).unwrap();
        let event = notify::Event::new(EventKind::Remove(notify::event::RemoveKind::Folder))
            .add_path(removed.clone());
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = PendingChanges::default();
        queue_change(
            &mut pending,
            removed.join("nested/second.py"),
            Instant::now(),
        )
        .unwrap();
        handle_event(
            event,
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        )
        .unwrap();

        assert!(store.indexed_paths().unwrap().is_empty());
        assert_eq!(store.concept_count().unwrap(), 0);
        assert!(pending.paths.is_empty() && pending.full_refresh.is_none());
        assert!(store.concept_contract_current().unwrap());
    }

    #[test]
    fn release_and_context_archive_are_history_paths() {
        let root = Path::new("/repo");
        assert!(is_project_history_path(
            Path::new("/repo/CONTEXT-archive-2025.md"),
            root
        ));
        assert!(is_project_history_path(
            Path::new("/repo/.mastermind/releases/001-task.md"),
            root
        ));
        assert!(!is_project_history_path(
            Path::new("/repo/notes/CONTEXT-archive-2025.md"),
            root
        ));
        for relative in PROJECT_DECISION_DIRS {
            assert!(is_project_history_path(
                &root.join(relative).join("nested/001.MD"),
                root
            ));
        }
        for path in [
            "docs/adr-backup/001.md",
            "docs/other.md",
            "other/adr/001.md",
        ] {
            assert!(!is_project_history_path(&root.join(path), root));
        }
        assert!(is_project_history_container(&root.join("docs"), root));
        assert!(is_project_history_container(
            &root.join(".mastermind"),
            root
        ));
        assert!(!is_project_history_container(root, root));
        assert!(!is_project_history_container(Path::new("/"), root));
        assert!(!is_project_history_container(
            &root.join("docs/other"),
            root
        ));
    }
}
