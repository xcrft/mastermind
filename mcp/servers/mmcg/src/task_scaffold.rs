//! Race-safe allocation of canonical `.mastermind/tasks/<id>-<slug>/spec.md` files.

use crate::bounded_fs::{
    create_regular_file_with_capability, inspect_absent_path, inspect_path_kind_with_capability,
    open_locked_regular_file_with_capability, read_directory_names_with_capability,
    read_regular_file_with_capability, write_atomic_regular_file_expected_with_capability,
    AtomicWriteExpectation, BoundedPathKind, BoundedReadError, ReadControl, RootCapability,
};
use std::io::Write;
use std::path::{Path, PathBuf};

const TASK_ENTRY_LIMIT: usize = 100_000;
const SCAFFOLD_FILE_LIMIT: u64 = 4 * 1024 * 1024;
const BACKUP_ATTEMPTS: u32 = 10_000;

#[derive(Debug, Eq, PartialEq)]
pub struct ScaffoldFileOutcome {
    pub written: bool,
    pub backup: Option<PathBuf>,
}

fn into_io_error(error: BoundedReadError) -> std::io::Error {
    match error {
        BoundedReadError::Io(error) => error,
        error => std::io::Error::other(error),
    }
}

fn validate_slug(slug: &str) -> std::io::Result<()> {
    let valid = !slug.is_empty()
        && slug.len() <= 40
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "task slug must contain 1-40 lowercase ASCII letters, digits, or interior hyphens",
        ))
    }
}

fn next_task_number(names: &[std::ffi::OsString]) -> std::io::Result<u32> {
    let current = names
        .iter()
        .filter_map(|name| name.to_str())
        .filter_map(|name| name.split('-').next())
        .filter_map(|prefix| prefix.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    current.checked_add(1).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "task number space is exhausted",
        )
    })
}

fn create_file_no_clobber(root: &RootCapability, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let (mut file, _) =
        create_regular_file_with_capability(root, path, false).map_err(into_io_error)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);

    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let observed =
        read_regular_file_with_capability(root, path, size, size, ReadControl::default())
            .map_err(into_io_error)?;
    if observed.bytes == bytes {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "created file changed while it was being published",
        ))
    }
}

fn create_backup(root: &RootCapability, path: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("scaffold");
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "scaffold path has no parent",
        )
    })?;
    for suffix in 0..BACKUP_ATTEMPTS {
        let name = if suffix == 0 {
            format!("{file_name}.mastermind-backup")
        } else {
            format!("{file_name}.mastermind-backup.{suffix}")
        };
        let candidate = parent.join(name);
        match create_file_no_clobber(root, &candidate, bytes) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("cannot allocate a backup for {}", path.display()),
    ))
}

/// Create a repository directory without following any path component.
pub fn ensure_repository_directory(repo_root: &Path, path: &Path) -> std::io::Result<bool> {
    let root = RootCapability::open(repo_root).map_err(into_io_error)?;
    let missing = inspect_absent_path(&root, path, ReadControl::default())
        .map_err(into_io_error)?
        .is_some();
    root.ensure_directory(path).map_err(into_io_error)?;
    Ok(missing)
}

/// Enumerate canonical task contracts without following repository links or
/// retaining an unbounded directory listing. `None` means the task directory
/// is absent; an existing linked or non-directory path is an error.
pub fn discover_canonical_task_specs(repo_root: &Path) -> std::io::Result<Option<Vec<PathBuf>>> {
    let root = RootCapability::open(repo_root).map_err(into_io_error)?;
    let tasks_dir = root.requested_root().join(".mastermind/tasks");
    match inspect_path_kind_with_capability(&root, &tasks_dir, ReadControl::default()) {
        Ok(BoundedPathKind::Directory) => {}
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                ".mastermind/tasks must be a real directory",
            ));
        }
        Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(into_io_error(error)),
    }
    let names = read_directory_names_with_capability(
        &root,
        &tasks_dir,
        TASK_ENTRY_LIMIT,
        ReadControl::default(),
    )
    .map_err(into_io_error)?;
    let mut specs = Vec::new();
    for name in names {
        let task_dir = tasks_dir.join(name);
        match inspect_path_kind_with_capability(&root, &task_dir, ReadControl::default()) {
            Ok(BoundedPathKind::Directory) => {}
            Ok(BoundedPathKind::RegularFile) => continue,
            Ok(BoundedPathKind::Other) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "task entry must be a real directory or regular file: {}",
                        task_dir.display()
                    ),
                ));
            }
            Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(error) => return Err(into_io_error(error)),
        }
        let spec = task_dir.join("spec.md");
        match inspect_path_kind_with_capability(&root, &spec, ReadControl::default()) {
            Ok(BoundedPathKind::RegularFile) => specs.push(spec),
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("task spec must be a real regular file: {}", spec.display()),
                ));
            }
            Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(into_io_error(error)),
        }
    }
    specs.sort();
    Ok(Some(specs))
}

/// Return legacy flat task specs for migration diagnostics through the same
/// bounded, no-follow task-directory capability.
#[doc(hidden)]
pub fn discover_legacy_flat_specs(repo_root: &Path) -> std::io::Result<Vec<String>> {
    let root = RootCapability::open(repo_root).map_err(into_io_error)?;
    let tasks_dir = root.requested_root().join(".mastermind/tasks");
    let names = read_directory_names_with_capability(
        &root,
        &tasks_dir,
        TASK_ENTRY_LIMIT,
        ReadControl::default(),
    )
    .map_err(into_io_error)?;
    let mut specs = Vec::new();
    for name in names {
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.ends_with(".md") || name.starts_with('_') || name.starts_with('.') {
            continue;
        }
        let path = tasks_dir.join(name);
        match inspect_path_kind_with_capability(&root, &path, ReadControl::default()) {
            Ok(BoundedPathKind::RegularFile) => specs.push(name.to_string()),
            Ok(_) => {}
            Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(into_io_error(error)),
        }
    }
    specs.sort();
    Ok(specs)
}

/// Write a generated project file under the shared scaffold lock. Existing
/// files are preserved unless `force` is set; forced replacements first copy
/// the exact observed version into a no-clobber backup and then conditionally
/// replace only that version.
pub fn write_project_file(
    repo_root: &Path,
    path: &Path,
    contents: &str,
    force: bool,
) -> std::io::Result<ScaffoldFileOutcome> {
    let root = RootCapability::open(repo_root).map_err(into_io_error)?;
    let mastermind_dir = root.requested_root().join(".mastermind");
    root.ensure_directory(&mastermind_dir)
        .map_err(into_io_error)?;
    let lock_path = mastermind_dir.join(".scaffold.lock");
    let lock =
        open_locked_regular_file_with_capability(&root, &lock_path).map_err(into_io_error)?;
    let result = (|| {
        if contents.len() as u64 > SCAFFOLD_FILE_LIMIT {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "generated scaffold has {} bytes, limit is {SCAFFOLD_FILE_LIMIT}",
                    contents.len()
                ),
            ));
        }
        let relative = root.repository_relative(path).map_err(into_io_error)?;
        if let Some(parent) = relative
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            root.ensure_directory(&root.requested_root().join(parent))
                .map_err(into_io_error)?;
        }
        let existing = match inspect_path_kind_with_capability(&root, path, ReadControl::default())
        {
            Ok(BoundedPathKind::RegularFile) if !force => {
                return Ok(ScaffoldFileOutcome {
                    written: false,
                    backup: None,
                })
            }
            Ok(BoundedPathKind::RegularFile) => Some(
                read_regular_file_with_capability(
                    &root,
                    path,
                    SCAFFOLD_FILE_LIMIT,
                    SCAFFOLD_FILE_LIMIT,
                    ReadControl::default(),
                )
                .map_err(into_io_error)?,
            ),
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("scaffold target is not a regular file: {}", path.display()),
                ))
            }
            Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                None
            }
            Err(error) => return Err(into_io_error(error)),
        };
        let Some(existing) = existing else {
            create_file_no_clobber(&root, path, contents.as_bytes())?;
            return Ok(ScaffoldFileOutcome {
                written: true,
                backup: None,
            });
        };
        let backup = create_backup(&root, path, &existing.bytes)?;
        write_atomic_regular_file_expected_with_capability(
            &root,
            path,
            contents.as_bytes(),
            false,
            AtomicWriteExpectation::File(existing.identity),
        )
        .map_err(into_io_error)?;
        Ok(ScaffoldFileOutcome {
            written: true,
            backup: Some(backup),
        })
    })();
    let unlock = lock.unlock();
    match (result, unlock) {
        (Err(error), _) => Err(error),
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Ok(_), Err(error)) => Err(error),
    }
}

/// Allocate one task number while holding the repository's scaffold lock, then
/// publish its spec through a no-follow `create_new` handle. Concurrent
/// `new-spec` processes cannot reuse an id or overwrite an existing spec.
pub fn create_numbered_spec(
    repo_root: &Path,
    slug: &str,
    render: impl FnOnce(u32) -> String,
) -> std::io::Result<PathBuf> {
    validate_slug(slug)?;
    let root = RootCapability::open(repo_root).map_err(into_io_error)?;
    let mastermind_dir = root.requested_root().join(".mastermind");
    let tasks_dir = mastermind_dir.join("tasks");
    root.ensure_directory(&tasks_dir).map_err(into_io_error)?;

    let lock_path = mastermind_dir.join(".new-spec.lock");
    let lock =
        open_locked_regular_file_with_capability(&root, &lock_path).map_err(into_io_error)?;
    let result = (|| {
        let names = read_directory_names_with_capability(
            &root,
            &tasks_dir,
            TASK_ENTRY_LIMIT,
            ReadControl::default(),
        )
        .map_err(into_io_error)?;
        let number = next_task_number(&names)?;
        let task_dir = tasks_dir.join(format!("{number:03}-{slug}"));
        if inspect_absent_path(&root, &task_dir, ReadControl::default())
            .map_err(into_io_error)?
            .is_none()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("task path already exists: {}", task_dir.display()),
            ));
        }
        root.ensure_directory(&task_dir).map_err(into_io_error)?;

        let spec_path = task_dir.join("spec.md");
        let body = render(number);
        create_file_no_clobber(&root, &spec_path, body.as_bytes())?;
        Ok(spec_path)
    })();
    let unlock = lock.unlock();
    match (result, unlock) {
        (Err(error), _) => Err(error),
        (Ok(path), Ok(())) => Ok(path),
        (Ok(_), Err(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Barrier};

    #[test]
    fn task_discovery_returns_only_real_canonical_and_legacy_specs() {
        let root = tempfile::tempdir().unwrap();
        let tasks = root.path().join(".mastermind/tasks");
        std::fs::create_dir_all(tasks.join("002-real")).unwrap();
        std::fs::create_dir_all(tasks.join("001-no-spec")).unwrap();
        std::fs::write(tasks.join("002-real/spec.md"), "# Real\n").unwrap();
        std::fs::write(tasks.join("003-legacy.md"), "# Legacy\n").unwrap();
        std::fs::write(tasks.join("_ignored.md"), "# Ignored\n").unwrap();
        std::fs::write(tasks.join("004-not-a-directory"), "file\n").unwrap();

        assert_eq!(
            discover_canonical_task_specs(root.path()).unwrap(),
            Some(vec![tasks.join("002-real/spec.md")])
        );
        assert_eq!(
            discover_legacy_flat_specs(root.path()).unwrap(),
            vec!["003-legacy.md"]
        );
    }

    #[test]
    fn repeated_allocations_get_distinct_numbers_and_preserve_specs() {
        let root = tempfile::tempdir().unwrap();
        let first = create_numbered_spec(root.path(), "first", |number| {
            format!("id: {number:03}\nfirst\n")
        })
        .unwrap();
        let second = create_numbered_spec(root.path(), "second", |number| {
            format!("id: {number:03}\nsecond\n")
        })
        .unwrap();

        assert!(first.ends_with("001-first/spec.md"));
        assert!(second.ends_with("002-second/spec.md"));
        assert_eq!(std::fs::read_to_string(first).unwrap(), "id: 001\nfirst\n");
        assert_eq!(
            std::fs::read_to_string(second).unwrap(),
            "id: 002\nsecond\n"
        );
    }

    #[test]
    fn concurrent_allocations_never_reuse_a_task_number() {
        const WORKERS: usize = 8;
        let root = tempfile::tempdir().unwrap();
        let root = Arc::new(root.path().to_path_buf());
        let barrier = Arc::new(Barrier::new(WORKERS));
        let workers = (0..WORKERS)
            .map(|worker| {
                let root = Arc::clone(&root);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    create_numbered_spec(&root, &format!("worker-{worker}"), |number| {
                        format!("id: {number:03}\n")
                    })
                    .unwrap()
                })
            })
            .collect::<Vec<_>>();
        let paths = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        let numbers = paths
            .iter()
            .map(|path| {
                path.parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()[..3]
                    .to_string()
            })
            .collect::<BTreeSet<_>>();

        assert_eq!(numbers.len(), WORKERS);
        assert_eq!(numbers.first().map(String::as_str), Some("001"));
        assert_eq!(numbers.last().map(String::as_str), Some("008"));
    }

    #[test]
    fn forced_project_writes_preserve_each_observed_version() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("CONTEXT.md");
        std::fs::write(&path, "first\n").unwrap();

        let first = write_project_file(root.path(), &path, "second\n", true).unwrap();
        let second = write_project_file(root.path(), &path, "third\n", true).unwrap();

        assert!(first.written);
        assert!(second.written);
        assert_ne!(first.backup, second.backup);
        assert_eq!(
            std::fs::read_to_string(first.backup.unwrap()).unwrap(),
            "first\n"
        );
        assert_eq!(
            std::fs::read_to_string(second.backup.unwrap()).unwrap(),
            "second\n"
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "third\n");
    }

    #[test]
    fn non_forced_project_writes_never_replace_existing_content() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("CLAUDE.md");
        std::fs::write(&path, "owned\n").unwrap();

        let outcome = write_project_file(root.path(), &path, "generated\n", false).unwrap();

        assert!(!outcome.written);
        assert!(outcome.backup.is_none());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "owned\n");
    }

    #[test]
    fn non_forced_project_writes_skip_oversized_existing_files_without_reading() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("CONTEXT.md");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(SCAFFOLD_FILE_LIMIT + 1).unwrap();
        drop(file);

        let outcome = write_project_file(root.path(), &path, "generated\n", false).unwrap();

        assert!(!outcome.written);
        assert!(outcome.backup.is_none());
        assert_eq!(
            std::fs::metadata(path).unwrap().len(),
            SCAFFOLD_FILE_LIMIT + 1
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_mastermind_directory_cannot_redirect_a_spec() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), root.path().join(".mastermind")).unwrap();

        assert!(create_numbered_spec(root.path(), "escaped", |_| "outside\n".into()).is_err());
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn task_discovery_rejects_a_symlinked_tasks_directory() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".mastermind")).unwrap();
        symlink(outside.path(), root.path().join(".mastermind/tasks")).unwrap();

        assert!(discover_canonical_task_specs(root.path()).is_err());
        assert!(discover_legacy_flat_specs(root.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn project_writes_do_not_follow_a_symlinked_target() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim.md");
        std::fs::write(&victim, "outside\n").unwrap();
        symlink(&victim, root.path().join("CONTEXT.md")).unwrap();

        assert!(write_project_file(
            root.path(),
            &root.path().join("CONTEXT.md"),
            "replacement\n",
            true,
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(victim).unwrap(), "outside\n");
    }
}
