//! Durable admission markers, independent of the SQLite writer lock.
//!
//! A marker contains no event text and deliberately survives errors and Drop.
//! Only a committed delivery or an explicit generation recovery removes it.

use crate::bounded_fs::{self, BoundedReadError, ReadControl, RootCapability, StableFileIdentity};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const MAX_FILES: usize = 1024;
// Concurrent admissions can cross MAX_FILES after their individual pre-scans.
// Reads then fail closed; explicit recovery has a separate bounded repair cap.
const MAX_RECOVERY_FILES: usize = 8192;
const SUFFIX: &str = ".pending";

pub(super) struct Fence {
    root: RootCapability,
    path: PathBuf,
    identity: StableFileIdentity,
}

fn directory() -> Result<PathBuf> {
    Ok(std::env::home_dir()
        .ok_or("could not resolve capture fence home")?
        .join(".mastermind/.persona-capture"))
}

fn prefix(client: &str, project: &Path) -> Result<String> {
    if !matches!(client, "claude" | "codex") {
        return Err("capture fence client must be claude or codex".into());
    }
    let project = project.canonicalize()?;
    if !project.is_dir() {
        return Err("capture fence project must be a directory".into());
    }
    let project = project
        .to_str()
        .ok_or("capture fence project is not UTF-8")?;
    let mut digest = Sha256::new();
    digest.update(b"mastermind-persona-capture-grant-v1\0");
    digest.update(project.as_bytes());
    Ok(format!(
        "{client}-{}-",
        crate::hex::encode(&digest.finalize())
    ))
}

pub(super) fn begin(client: &str, root: &Path) -> Result<Fence> {
    begin_at(&directory()?, &prefix(client, root)?)
}

pub(super) fn pending(client: &str, root: &Path) -> Result<bool> {
    pending_at(&directory()?, &prefix(client, root)?)
}

pub(super) fn recover(client: &str, root: &Path) -> Result<()> {
    recover_at(&directory()?, &prefix(client, root)?)
}

impl Fence {
    pub(super) fn current(&self) -> Result<bool> {
        match bounded_fs::read_regular_file_expected(
            &self.root,
            &self.path,
            0,
            0,
            ReadControl::default(),
            Some(self.identity),
        ) {
            Ok(_) => Ok(true),
            Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(false)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn clear(self) -> Result<()> {
        bounded_fs::remove_regular_file_expected_with_capability(
            &self.root,
            &self.path,
            self.identity,
        )?;
        Ok(())
    }
}

fn existing(directory: &Path) -> Result<Option<RootCapability>> {
    match RootCapability::open(directory) {
        Ok(root) => Ok(Some(root)),
        Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

fn valid_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(SUFFIX) else {
        return false;
    };
    let mut parts = stem.split('-');
    matches!(parts.next(), Some("claude" | "codex"))
        && parts.next().is_some_and(|part| lower_hex(part, 64))
        && parts.next().is_some_and(|part| lower_hex(part, 32))
        && parts.next().is_none()
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

// Validate the complete bounded directory before treating it as absence or
// starting recovery. Foreign, oversized or special files are never ignored.
fn markers(root: &RootCapability, limit: usize) -> Result<Vec<(PathBuf, StableFileIdentity)>> {
    let names = bounded_fs::read_directory_names_with_capability(
        root,
        root.canonical_root(),
        limit,
        ReadControl::default(),
    )?;
    marker_entries(root, names, false)
}

fn marker_entries(
    root: &RootCapability,
    names: Vec<std::ffi::OsString>,
    allow_disappeared: bool,
) -> Result<Vec<(PathBuf, StableFileIdentity)>> {
    let mut result = Vec::with_capacity(names.len());
    for name in names {
        let name = name.to_str().ok_or("invalid capture fence filename")?;
        if !valid_name(name) {
            return Err("unexpected capture fence directory entry".into());
        }
        let path = root.canonical_root().join(name);
        let file = match bounded_fs::read_regular_file_with_capability(
            root,
            &path,
            0,
            0,
            ReadControl::default(),
        ) {
            Ok(file) => file,
            Err(BoundedReadError::Io(error))
                if allow_disappeared && error.kind() == std::io::ErrorKind::NotFound =>
            {
                continue;
            }
            Err(BoundedReadError::SnapshotChanged) if allow_disappeared => {
                // A different delivery can finish after this entry was listed
                // or opened. A replacement is not the same as its absence.
                root.verify()?;
                if bounded_fs::inspect_absent_path(root, &path, ReadControl::default())?.is_some() {
                    continue;
                }
                return Err(BoundedReadError::SnapshotChanged.into());
            }
            Err(error) => return Err(error.into()),
        };
        result.push((path, file.identity));
    }
    Ok(result)
}

fn check_admission(root: &RootCapability, path: &Path, identity: StableFileIdentity) -> Result<()> {
    match bounded_fs::read_directory_names_with_capability(
        root,
        root.canonical_root(),
        MAX_FILES,
        ReadControl::default(),
    ) {
        Ok(names) => {
            marker_entries(root, names, true)?;
        }
        // Other valid captures may create/clear their entries while this one
        // is being admitted. The marker is already durable, so this benign
        // directory race must not drop the event or abandon its live fence.
        Err(BoundedReadError::SnapshotChanged) => root.verify()?,
        Err(error) => return Err(error.into()),
    }
    bounded_fs::read_regular_file_expected(
        root,
        path,
        0,
        0,
        ReadControl::default(),
        Some(identity),
    )?;
    Ok(())
}

fn begin_at(directory: &Path, prefix: &str) -> Result<Fence> {
    // This path is used only to prepare the directory, never written.
    let (root, _) = bounded_fs::prepare_file_target(&directory.join(".capture"))?;
    root.set_root_directory_mode(0o700)?;
    match bounded_fs::read_directory_names_with_capability(
        &root,
        root.canonical_root(),
        MAX_FILES,
        ReadControl::default(),
    ) {
        Ok(names) if names.len() >= MAX_FILES => {
            return Err(
                "capture fence limit reached; inspect pending deliveries and recover explicitly"
                    .into(),
            );
        }
        Ok(_) | Err(BoundedReadError::SnapshotChanged) => {}
        Err(error) => return Err(error.into()),
    }
    for _ in 0..8 {
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).map_err(|error| {
            std::io::Error::other(format!("capture fence nonce unavailable: {error}"))
        })?;
        let path = root
            .canonical_root()
            .join(format!("{prefix}{}{SUFFIX}", crate::hex::encode(&nonce)));
        match bounded_fs::create_regular_file_with_capability(&root, &path, true) {
            Ok((file, identity)) => {
                // Empty is the complete marker payload. It is already visible
                // as pending if the process fails before either fsync finishes.
                file.sync_all()?;
                root.sync()?;
                // Also persist freshly created .persona-capture/.mastermind
                // directory entries. Existing ancestors are not modified.
                for parent in root.canonical_root().ancestors().skip(1).take(2) {
                    RootCapability::open(parent)?.sync()?;
                }
                // Overcapacity and invalid entries retain the marker on error;
                // ordinary concurrent completions do not drop this delivery.
                check_admission(&root, &path, identity)?;
                return Ok(Fence {
                    root,
                    path,
                    identity,
                });
            }
            Err(BoundedReadError::Io(error))
                if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err("could not allocate a unique capture fence".into())
}

fn pending_at(directory: &Path, prefix: &str) -> Result<bool> {
    let Some(root) = existing(directory)? else {
        return Ok(false);
    };
    Ok(markers(&root, MAX_FILES)?.iter().any(|(path, _)| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(prefix))
    }))
}

fn recover_at(directory: &Path, prefix: &str) -> Result<()> {
    let Some(root) = existing(directory)? else {
        return Ok(());
    };
    for (path, identity) in markers(&root, MAX_RECOVERY_FILES)? {
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(prefix))
        {
            bounded_fs::remove_regular_file_expected_with_capability(&root, &path, identity)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _temp: tempfile::TempDir,
        directory: PathBuf,
        project: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let project = temp.path().join("project");
            std::fs::create_dir(&project).unwrap();
            Self {
                directory: temp.path().join(".mastermind/.persona-capture"),
                project,
                _temp: temp,
            }
        }
        fn key(&self, client: &str) -> String {
            prefix(client, &self.project).unwrap()
        }
    }

    #[test]
    fn reads_do_not_create_state_and_drop_preserves_a_pending_delivery() {
        let f = Fixture::new();
        let key = f.key("codex");
        assert!(!pending_at(&f.directory, &key).unwrap());
        recover_at(&f.directory, &key).unwrap();
        assert!(!f.directory.exists());
        let fence = begin_at(&f.directory, &key).unwrap();
        assert!(fence.current().unwrap());
        assert!(pending_at(&f.directory, &key).unwrap());
        assert_eq!(std::fs::read(&fence.path).unwrap(), b"");
        drop(fence);
        assert!(pending_at(&f.directory, &key).unwrap());
        recover_at(&f.directory, &key).unwrap();
        assert!(!pending_at(&f.directory, &key).unwrap());
    }

    #[test]
    fn independent_deliveries_clear_only_their_own_marker() {
        let f = Fixture::new();
        let key = f.key("codex");
        let first = begin_at(&f.directory, &key).unwrap();
        let second = begin_at(&f.directory, &key).unwrap();
        assert_ne!(first.path, second.path);
        first.clear().unwrap();
        assert!(second.current().unwrap());
        assert!(pending_at(&f.directory, &key).unwrap());
        second.clear().unwrap();
        assert!(!pending_at(&f.directory, &key).unwrap());
    }

    #[test]
    fn recovery_revokes_old_handles_and_preserves_other_grants() {
        let f = Fixture::new();
        let key = f.key("codex");
        let old = begin_at(&f.directory, &key).unwrap();
        let other_client = begin_at(&f.directory, &f.key("claude")).unwrap();
        let other_project = f._temp.path().join("other-project");
        std::fs::create_dir(&other_project).unwrap();
        let other_key = prefix("codex", &other_project).unwrap();
        let other = begin_at(&f.directory, &other_key).unwrap();
        recover_at(&f.directory, &key).unwrap();
        assert!(!old.current().unwrap());
        assert!(old.clear().is_err());
        assert!(other.current().unwrap());
        assert!(other_client.current().unwrap());
        let fresh = begin_at(&f.directory, &key).unwrap();
        assert!(fresh.current().unwrap());
        fresh.clear().unwrap();
        other.clear().unwrap();
        other_client.clear().unwrap();
    }

    #[test]
    fn changed_identity_and_nonempty_markers_fail_closed() {
        let f = Fixture::new();
        let key = f.key("codex");
        let fence = begin_at(&f.directory, &key).unwrap();
        let saved = f._temp.path().join("saved-marker");
        std::fs::rename(&fence.path, saved).unwrap();
        std::fs::write(&fence.path, b"").unwrap();
        assert!(fence.current().is_err());
        assert!(fence.clear().is_err());
        assert!(pending_at(&f.directory, &key).unwrap());
        let path = std::fs::read_dir(&f.directory)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(path, b"unexpected bytes").unwrap();
        assert!(pending_at(&f.directory, &key).is_err());
        assert!(recover_at(&f.directory, &key).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn markers_are_private_and_symlinks_never_count_as_absence() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let f = Fixture::new();
        let key = f.key("codex");
        let fence = begin_at(&f.directory, &key).unwrap();
        assert_eq!(
            std::fs::metadata(&fence.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&f.directory)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let path = fence.path.clone();
        fence.clear().unwrap();
        symlink(f._temp.path().join("missing"), &path).unwrap();
        assert!(pending_at(&f.directory, &key).is_err());
        assert!(recover_at(&f.directory, &key).is_err());
        assert!(begin_at(&f.directory, &key).is_err());
        assert!(std::fs::symlink_metadata(path)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn directory_entry_count_is_bounded_and_recovery_frees_capacity() {
        let f = Fixture::new();
        let key = f.key("codex");
        begin_at(&f.directory, &key).unwrap().clear().unwrap();
        for index in 0..MAX_FILES {
            std::fs::write(f.directory.join(format!("{key}{index:032x}{SUFFIX}")), b"").unwrap();
        }
        assert!(begin_at(&f.directory, &key).is_err());
        assert_eq!(std::fs::read_dir(&f.directory).unwrap().count(), MAX_FILES);
        assert!(pending_at(&f.directory, &key).unwrap());
        recover_at(&f.directory, &key).unwrap();
        assert_eq!(std::fs::read_dir(&f.directory).unwrap().count(), 0);
        begin_at(&f.directory, &key).unwrap().clear().unwrap();
        for index in 0..MAX_FILES + 1 {
            std::fs::write(f.directory.join(format!("{key}{index:032x}{SUFFIX}")), b"").unwrap();
        }
        assert!(pending_at(&f.directory, &key).is_err());
        recover_at(&f.directory, &key).unwrap();
        assert!(!pending_at(&f.directory, &key).unwrap());
    }

    #[test]
    fn parallel_begins_leave_unique_durable_markers_even_on_scan_errors() {
        let f = Fixture::new();
        let key = f.key("codex");
        begin_at(&f.directory, &key).unwrap().clear().unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let workers = (0..2)
            .map(|_| {
                let directory = f.directory.clone();
                let key = key.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    begin_at(&directory, &key).is_ok()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        for worker in workers {
            let _ = worker.join().unwrap();
        }
        assert_eq!(std::fs::read_dir(&f.directory).unwrap().count(), 2);
        assert!(pending_at(&f.directory, &key).unwrap());
        recover_at(&f.directory, &key).unwrap();
    }

    #[test]
    fn parallel_begin_and_clear_do_not_abandon_markers() {
        let f = Fixture::new();
        let key = f.key("codex");
        begin_at(&f.directory, &key).unwrap().clear().unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let workers = (0..2)
            .map(|_| {
                let directory = f.directory.clone();
                let key = key.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..40 {
                        let fence = begin_at(&directory, &key).unwrap();
                        assert!(fence.current().unwrap());
                        fence.clear().unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(std::fs::read_dir(&f.directory).unwrap().count(), 0);
        assert!(!pending_at(&f.directory, &key).unwrap());
    }
}
