//! Filesystem watcher — keeps the index fresh as files change.
//!
//! Architecture:
//!   1. Initial full index of `root`
//!   2. Subscribe to recursive filesystem events under `root`
//!   3. Coalesce rapid-fire events per path with a 500ms debounce
//!   4. Re-index files on Modify/Create, purge on Remove
//!
//! Runs in the foreground until stdin closes or Ctrl-C.

use crate::indexer::{extractor_for_path, Indexer};
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

    // Initial pass — bring the index up to date before we start watching.
    // Incremental: only re-parses files whose mtime is newer than the stored index.
    let stats = indexer.index_all(&mut store, false)?;
    eprintln!(
        "[mmcg watch] initial: indexed {} (unchanged {}, purged {}) in {} ms",
        stats.files_indexed, stats.files_unchanged, stats.files_purged, stats.duration_ms
    );
    eprintln!("[mmcg watch] watching {}", root.display());

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(tx)?;
    watcher.watch(&root, RecursiveMode::Recursive)?;

    let mut pending_changes: HashMap<PathBuf, Instant> = HashMap::new();

    loop {
        // Drain incoming events with a short timeout so we can also flush pending changes
        match rx.recv_timeout(RX_TIMEOUT) {
            Ok(Ok(event)) => handle_event(event, &root, &mut pending_changes, &mut store),
            Ok(Err(e)) => eprintln!("[mmcg watch] notify error: {e}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                eprintln!("[mmcg watch] watcher channel closed");
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
    pending: &mut HashMap<PathBuf, Instant>,
    store: &mut Store,
) {
    match event.kind {
        EventKind::Modify(_) | EventKind::Create(_) => {
            for path in event.paths {
                if path.is_file() && extractor_for_path(&path).is_some() && !is_ignored(&path, root)
                {
                    pending.insert(path, Instant::now());
                }
            }
        }
        EventKind::Remove(_) => {
            for path in event.paths {
                if let Some(rel) = relative_path(&path, root) {
                    // Drop from pending (race: a quick rm + write would otherwise re-add)
                    pending.remove(&path);
                    if let Err(e) = store.purge_file(&rel) {
                        eprintln!("[mmcg watch] purge failed {rel}: {e}");
                    } else {
                        eprintln!("[mmcg watch] purged {rel}");
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
            Ok(()) => eprintln!("[mmcg watch] reindexed {}", path.display()),
            Err(e) => eprintln!("[mmcg watch] failed {}: {e}", path.display()),
        }
    }
}

fn relative_path(absolute: &Path, root: &Path) -> Option<String> {
    absolute
        .strip_prefix(root)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

/// Skip events for the index file itself and anything under skipped dirs.
fn is_ignored(path: &Path, root: &Path) -> bool {
    // Skip the index database
    if path.components().any(|c| {
        c.as_os_str() == ".mastermind" || c.as_os_str() == ".git" || c.as_os_str() == "target"
    }) {
        return true;
    }
    // Also skip if the relative prefix is missing (event outside root somehow)
    path.strip_prefix(root).is_err()
}
