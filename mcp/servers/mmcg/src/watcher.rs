//! Filesystem watcher — keeps the index fresh as files change.
//!
//! Architecture:
//!   1. Initial full index of `root`
//!   2. Subscribe to recursive filesystem events under `root`
//!   3. Coalesce rapid-fire events per path with a 500ms debounce
//!   4. Re-index files on Modify/Create, purge on Remove
//!
//! Runs in the foreground until stdin closes or Ctrl-C.

use crate::indexer::{extractor_for_path, IndexError, Indexer, SourceMatcher};
use crate::store::Store;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(500);
const RX_TIMEOUT: Duration = Duration::from_millis(100);

pub fn run(root: PathBuf, mut store: Store) -> Result<(), Box<dyn std::error::Error>> {
    let root = root.canonicalize()?;
    let indexer = Indexer::new(&root);
    let mut source_matcher = SourceMatcher::new(&root);

    // Initial pass — bring the index current before watching. Incremental:
    // only re-parses files whose mtime is newer than the stored index.
    let stats = indexer.index_all(&mut store, false)?;
    eprintln!(
        "[mastermind watch] initial: indexed {} (unchanged {}, purged {}) in {} ms",
        stats.files_indexed, stats.files_unchanged, stats.files_purged, stats.duration_ms
    );
    eprintln!("[mastermind watch] watching {}", root.display());

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(tx)?;
    watcher.watch(&root, RecursiveMode::Recursive)?;

    let mut pending_changes: HashMap<PathBuf, Instant> = HashMap::new();

    loop {
        // Short timeout so we can also flush pending changes between events.
        match rx.recv_timeout(RX_TIMEOUT) {
            Ok(Ok(event)) => handle_event(
                event,
                &root,
                &indexer,
                &mut source_matcher,
                &mut pending_changes,
                &mut store,
            ),
            Ok(Err(e)) => eprintln!("[mastermind watch] notify error: {e}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                eprintln!("[mastermind watch] watcher channel closed");
                break;
            }
        }

        flush_settled(&indexer, &mut store, &mut pending_changes);
    }

    Ok(())
}

fn handle_event(
    event: notify::Event,
    root: &Path,
    indexer: &Indexer,
    source_matcher: &mut SourceMatcher,
    pending: &mut HashMap<PathBuf, Instant>,
    store: &mut Store,
) {
    if event.paths.iter().any(|path| is_ignore_config(path, root)) {
        *source_matcher = SourceMatcher::new(root);
        pending.clear();
        match indexer.index_all(store, false) {
            Ok(stats) => eprintln!(
                "[mastermind watch] ignore rules changed: indexed {} (unchanged {}, purged {})",
                stats.files_indexed, stats.files_unchanged, stats.files_purged
            ),
            Err(error) => eprintln!("[mastermind watch] ignore-rule refresh failed: {error}"),
        }
        return;
    }

    match event.kind {
        EventKind::Modify(_) | EventKind::Create(_) => {
            for path in event.paths {
                if path.is_file()
                    && extractor_for_path(&path).is_some()
                    && !source_matcher.is_ignored(&path, false)
                {
                    pending.insert(path, Instant::now());
                }
            }
        }
        EventKind::Remove(_) => {
            for path in event.paths {
                if let Some(rel) = relative_path(&path, root) {
                    // Drop from pending — a quick rm + write would otherwise re-add it.
                    pending.remove(&path);
                    if let Err(e) = store.purge_file(&rel) {
                        eprintln!("[mastermind watch] purge failed {rel}: {e}");
                    } else {
                        eprintln!("[mastermind watch] purged {rel}");
                    }
                }
            }
        }
        _ => {}
    }
}

fn flush_settled(indexer: &Indexer, store: &mut Store, pending: &mut HashMap<PathBuf, Instant>) {
    let now = Instant::now();
    let ready: Vec<PathBuf> = pending
        .iter()
        .filter(|(_, t)| now.duration_since(**t) >= DEBOUNCE)
        .map(|(p, _)| p.clone())
        .collect();

    for path in ready {
        pending.remove(&path);
        if !path.is_file() {
            continue;
        }
        match indexer.index_one(store, &path) {
            Ok(()) => eprintln!("[mastermind watch] reindexed {}", path.display()),
            Err(IndexError::Skipped(reason)) => {
                eprintln!("[mastermind watch] skipped {}: {reason:?}", path.display())
            }
            Err(e) => eprintln!("[mastermind watch] failed {}: {e}", path.display()),
        }
    }
}

fn relative_path(absolute: &Path, root: &Path) -> Option<String> {
    absolute
        .strip_prefix(root)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::ModifyKind;

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
        let mut pending = HashMap::new();
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
        );

        assert!(pending.is_empty());
        assert!(matcher.is_ignored(&source, false));
        assert!(store.indexed_paths().unwrap().is_empty());
    }
}
