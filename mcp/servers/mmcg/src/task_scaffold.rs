//! Race-safe allocation of canonical `.mastermind/tasks/<id>-<slug>/spec.md` files.

use crate::bounded_fs::{
    create_regular_file_with_capability, inspect_absent_path,
    open_locked_regular_file_with_capability, read_directory_names_with_capability,
    read_regular_file_with_capability, BoundedReadError, ReadControl, RootCapability,
};
use std::io::Write;
use std::path::{Path, PathBuf};

const TASK_ENTRY_LIMIT: usize = 100_000;

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
        let (mut file, _) =
            create_regular_file_with_capability(&root, &spec_path, false).map_err(into_io_error)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        drop(file);

        let observed = read_regular_file_with_capability(
            &root,
            &spec_path,
            u64::try_from(body.len()).unwrap_or(u64::MAX),
            u64::try_from(body.len()).unwrap_or(u64::MAX),
            ReadControl::default(),
        )
        .map_err(into_io_error)?;
        if observed.bytes != body.as_bytes() {
            return Err(std::io::Error::other(
                "task spec changed while it was being created",
            ));
        }
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
}
