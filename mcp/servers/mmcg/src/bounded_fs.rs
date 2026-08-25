//! Capability-scoped, bounded reads for repository-owned regular files.
//!
//! Callers pass an already selected repository root plus a repository-relative
//! path. Every directory component and the final file are opened without
//! following links. The descriptor identity is checked before and after the
//! read and then re-opened through the same root capability, closing the usual
//! check/open and rename/swap races.

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Instant, SystemTime};

const READ_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Default)]
pub(crate) struct ReadControl<'a> {
    pub deadline: Option<Instant>,
    pub interrupted: Option<&'a dyn Fn() -> bool>,
}

impl<'a> ReadControl<'a> {
    pub(crate) fn check(self) -> Result<(), BoundedReadError> {
        if self.interrupted.is_some_and(|check| check()) {
            return Err(BoundedReadError::Interrupted);
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(BoundedReadError::DeadlineExceeded);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) enum BoundedReadError {
    InvalidPath,
    OutsideRoot,
    NotRegular,
    TooLarge { size: u64, limit: u64 },
    SnapshotChanged,
    Interrupted,
    DeadlineExceeded,
    Io(std::io::Error),
}

impl std::fmt::Display for BoundedReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPath => formatter.write_str("path is not a safe repository-relative path"),
            Self::OutsideRoot => formatter.write_str("path escapes the repository root"),
            Self::NotRegular => formatter.write_str("path is not a regular no-follow file"),
            Self::TooLarge { size, limit } => {
                write!(formatter, "file has {size} bytes, limit is {limit}")
            }
            Self::SnapshotChanged => formatter.write_str("file identity changed during read"),
            Self::Interrupted => formatter.write_str("file read was cancelled"),
            Self::DeadlineExceeded => formatter.write_str("file read deadline exceeded"),
            Self::Io(error) => write!(formatter, "file read failed: {error}"),
        }
    }
}

impl std::error::Error for BoundedReadError {}

#[derive(Debug)]
pub(crate) struct BoundedFile {
    pub bytes: Vec<u8>,
    pub declared_len: u64,
    pub modified_millis: i64,
    pub identity: StableFileIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StableFileIdentity {
    volume: u64,
    index: u64,
    length: u64,
    modified_seconds: i64,
    modified_fraction: i64,
    attributes: u64,
}

impl StableFileIdentity {
    pub(crate) fn same_object(self, other: Self) -> bool {
        self.volume == other.volume && self.index == other.index
    }
}

pub(crate) struct RootCapability {
    requested_root: PathBuf,
    canonical_root: PathBuf,
    directory: Dir,
    identity: StableFileIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoundedPathKind {
    RegularFile,
    Directory,
    Other,
}

#[cfg(unix)]
fn stable_file_identity(file: &std::fs::File) -> std::io::Result<StableFileIdentity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(StableFileIdentity {
        volume: metadata.dev(),
        index: metadata.ino(),
        length: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_fraction: metadata.mtime_nsec(),
        attributes: metadata.mode() as u64,
    })
}

#[cfg(windows)]
fn stable_file_identity(file: &std::fs::File) -> std::io::Result<StableFileIdentity> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    let success = unsafe {
        GetFileInformationByHandle(file.as_raw_handle(), std::ptr::addr_of_mut!(information))
    };
    if success == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(StableFileIdentity {
        volume: information.dwVolumeSerialNumber as u64,
        index: ((information.nFileIndexHigh as u64) << 32) | information.nFileIndexLow as u64,
        length: ((information.nFileSizeHigh as u64) << 32) | information.nFileSizeLow as u64,
        modified_seconds: information.ftLastWriteTime.dwHighDateTime as i64,
        modified_fraction: information.ftLastWriteTime.dwLowDateTime as i64,
        attributes: information.dwFileAttributes as u64,
    })
}

#[cfg(not(any(unix, windows)))]
fn stable_file_identity(_file: &std::fs::File) -> std::io::Result<StableFileIdentity> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "stable file identity is unavailable on this platform",
    ))
}

fn relative_path(root: &Path, path: &Path) -> Result<PathBuf, BoundedReadError> {
    let relative = if path.is_absolute() {
        path.strip_prefix(root)
            .map_err(|_| BoundedReadError::OutsideRoot)?
    } else {
        path
    };
    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => normalized.push(value),
            _ => return Err(BoundedReadError::InvalidPath),
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(BoundedReadError::InvalidPath);
    }
    Ok(normalized)
}

#[cfg(unix)]
fn open_absolute_directory_nofollow(path: &Path) -> Result<Dir, BoundedReadError> {
    if !path.is_absolute() {
        return Err(BoundedReadError::InvalidPath);
    }
    let mut directory =
        Dir::open_ambient_dir(Path::new("/"), ambient_authority()).map_err(BoundedReadError::Io)?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(component) => {
                directory = directory
                    .open_dir_nofollow(component)
                    .map_err(BoundedReadError::Io)?;
            }
            _ => return Err(BoundedReadError::InvalidPath),
        }
    }
    Ok(directory)
}

#[cfg(windows)]
fn open_absolute_directory_nofollow(path: &Path) -> Result<Dir, BoundedReadError> {
    if !path.is_absolute() {
        return Err(BoundedReadError::InvalidPath);
    }
    let mut components = path.components();
    let Component::Prefix(prefix) = components.next().ok_or(BoundedReadError::InvalidPath)? else {
        return Err(BoundedReadError::InvalidPath);
    };
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(BoundedReadError::InvalidPath);
    }
    let mut anchor = PathBuf::from(prefix.as_os_str());
    anchor.push(Path::new(r"\"));
    let mut directory =
        Dir::open_ambient_dir(anchor, ambient_authority()).map_err(BoundedReadError::Io)?;
    for component in components {
        let Component::Normal(component) = component else {
            return Err(BoundedReadError::InvalidPath);
        };
        directory = directory
            .open_dir_nofollow(component)
            .map_err(BoundedReadError::Io)?;
    }
    Ok(directory)
}

#[cfg(not(any(unix, windows)))]
fn open_absolute_directory_nofollow(_path: &Path) -> Result<Dir, BoundedReadError> {
    Err(BoundedReadError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "root capabilities are unavailable on this platform",
    )))
}

fn directory_identity(directory: &Dir) -> Result<StableFileIdentity, BoundedReadError> {
    stable_file_identity(
        &directory
            .try_clone()
            .map_err(BoundedReadError::Io)?
            .into_std_file(),
    )
    .map_err(BoundedReadError::Io)
}

impl RootCapability {
    pub(crate) fn open(root: &Path) -> Result<Self, BoundedReadError> {
        let requested_root = std::path::absolute(root).map_err(BoundedReadError::Io)?;
        let metadata = std::fs::symlink_metadata(&requested_root).map_err(BoundedReadError::Io)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
            return Err(BoundedReadError::NotRegular);
        }
        let canonical_root = requested_root
            .canonicalize()
            .map_err(BoundedReadError::Io)?;
        let directory = open_absolute_directory_nofollow(&canonical_root)?;
        let identity = directory_identity(&directory)?;
        let capability = Self {
            requested_root,
            canonical_root,
            directory,
            identity,
        };
        capability.verify()?;
        Ok(capability)
    }

    pub(crate) fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    pub(crate) fn requested_root(&self) -> &Path {
        &self.requested_root
    }

    fn relative(&self, path: &Path) -> Result<PathBuf, BoundedReadError> {
        if path.is_absolute() {
            relative_path(&self.canonical_root, path)
                .or_else(|_| relative_path(&self.requested_root, path))
        } else {
            // Callers may pass either `src/lib.rs` or a path produced by
            // `relative_root.join("src/lib.rs")`. Recognize the latter before
            // treating the input as repository-relative, otherwise a relative
            // root named `repo` would resolve `repo/src` as `repo/repo/src`.
            let absolute = std::path::absolute(path).map_err(BoundedReadError::Io)?;
            relative_path(&self.requested_root, &absolute)
                .or_else(|_| relative_path(&self.canonical_root, &absolute))
                .or_else(|_| relative_path(&self.requested_root, path))
        }
    }

    pub(crate) fn verify(&self) -> Result<(), BoundedReadError> {
        let metadata = std::fs::symlink_metadata(&self.requested_root)
            .map_err(|_| BoundedReadError::SnapshotChanged)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
            return Err(BoundedReadError::SnapshotChanged);
        }
        let current_canonical = self
            .requested_root
            .canonicalize()
            .map_err(|_| BoundedReadError::SnapshotChanged)?;
        if current_canonical != self.canonical_root {
            return Err(BoundedReadError::SnapshotChanged);
        }
        let current = open_absolute_directory_nofollow(&self.canonical_root)
            .map_err(|_| BoundedReadError::SnapshotChanged)?;
        if !directory_identity(&current)
            .map_err(|_| BoundedReadError::SnapshotChanged)?
            .same_object(self.identity)
        {
            return Err(BoundedReadError::SnapshotChanged);
        }
        Ok(())
    }

    pub(crate) fn ensure_directory(&self, relative: &Path) -> Result<(), BoundedReadError> {
        self.verify()?;
        let relative = relative_path(&self.requested_root, relative)?;
        let mut directory = self.directory.try_clone().map_err(BoundedReadError::Io)?;
        for component in relative.components() {
            let Component::Normal(component) = component else {
                return Err(BoundedReadError::InvalidPath);
            };
            match directory.open_dir_nofollow(component) {
                Ok(next) => directory = next,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    directory
                        .create_dir(component)
                        .map_err(BoundedReadError::Io)?;
                    directory = directory
                        .open_dir_nofollow(component)
                        .map_err(BoundedReadError::Io)?;
                }
                Err(error) => return Err(BoundedReadError::Io(error)),
            }
        }
        self.verify()
    }
}

fn open_relative_nofollow(root: &Dir, relative: &Path) -> Result<std::fs::File, BoundedReadError> {
    let components = relative
        .components()
        .map(|component| match component {
            Component::Normal(value) => Ok(value.to_os_string()),
            _ => Err(BoundedReadError::InvalidPath),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (name, parents) = components
        .split_last()
        .ok_or(BoundedReadError::InvalidPath)?;

    let mut parent = root.try_clone().map_err(BoundedReadError::Io)?;
    for component in parents {
        parent = parent
            .open_dir_nofollow(component)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotADirectory => BoundedReadError::NotRegular,
                _ => BoundedReadError::Io(error),
            })?;
    }

    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    parent
        .open_with(name, &options)
        .map(cap_std::fs::File::into_std)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotADirectory => {
                BoundedReadError::NotRegular
            }
            _ => BoundedReadError::Io(error),
        })
}

pub(crate) fn create_regular_file_with_capability(
    root: &RootCapability,
    path: &Path,
) -> Result<(std::fs::File, StableFileIdentity), BoundedReadError> {
    root.verify()?;
    let relative = root.relative(path)?;
    let components = relative
        .components()
        .map(|component| match component {
            Component::Normal(value) => Ok(value.to_os_string()),
            _ => Err(BoundedReadError::InvalidPath),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (name, parents) = components
        .split_last()
        .ok_or(BoundedReadError::InvalidPath)?;
    let mut parent = root.directory.try_clone().map_err(BoundedReadError::Io)?;
    for component in parents {
        parent = parent
            .open_dir_nofollow(component)
            .map_err(BoundedReadError::Io)?;
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    let file = parent
        .open_with(name, &options)
        .map(cap_std::fs::File::into_std)
        .map_err(BoundedReadError::Io)?;
    let identity = stable_file_identity(&file).map_err(BoundedReadError::Io)?;
    root.verify()?;
    Ok((file, identity))
}

fn open_relative_directory_nofollow(root: &Dir, relative: &Path) -> Result<Dir, BoundedReadError> {
    let mut directory = root.try_clone().map_err(BoundedReadError::Io)?;
    if relative.as_os_str().is_empty() {
        return Ok(directory);
    }
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(BoundedReadError::InvalidPath);
        };
        directory = directory
            .open_dir_nofollow(component)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotADirectory => BoundedReadError::NotRegular,
                _ => BoundedReadError::Io(error),
            })?;
    }
    Ok(directory)
}

pub(crate) fn inspect_path_kind_with_capability(
    root: &RootCapability,
    path: &Path,
    control: ReadControl<'_>,
) -> Result<BoundedPathKind, BoundedReadError> {
    control.check()?;
    root.verify()?;
    let relative = root.relative(path)?;
    if open_relative_directory_nofollow(&root.directory, &relative).is_ok() {
        root.verify()?;
        return Ok(BoundedPathKind::Directory);
    }
    let file = match open_relative_nofollow(&root.directory, &relative) {
        Ok(file) => file,
        Err(BoundedReadError::NotRegular) => return Ok(BoundedPathKind::Other),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata().map_err(BoundedReadError::Io)?;
    root.verify()?;
    Ok(if metadata.file_type().is_file() {
        BoundedPathKind::RegularFile
    } else {
        BoundedPathKind::Other
    })
}

fn modified_millis(metadata: &std::fs::Metadata) -> i64 {
    // Second precision misses edits made within the same index-run second.
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// Read at most `read_limit` bytes while rejecting a file whose declared or
/// observed length exceeds `max_bytes`. `read_limit == max_bytes` is a complete
/// bounded read; a smaller value is useful for binary sniffing without a second
/// unbounded admission path.
pub(crate) fn read_regular_file(
    root: &Path,
    path: &Path,
    max_bytes: u64,
    read_limit: u64,
    control: ReadControl<'_>,
) -> Result<BoundedFile, BoundedReadError> {
    let root = RootCapability::open(root)?;
    read_regular_file_with_capability(&root, path, max_bytes, read_limit, control)
}

pub(crate) fn read_regular_file_with_capability(
    root: &RootCapability,
    path: &Path,
    max_bytes: u64,
    read_limit: u64,
    control: ReadControl<'_>,
) -> Result<BoundedFile, BoundedReadError> {
    read_regular_file_expected(root, path, max_bytes, read_limit, control, None)
}

pub(crate) fn read_regular_file_expected(
    root: &RootCapability,
    path: &Path,
    max_bytes: u64,
    read_limit: u64,
    control: ReadControl<'_>,
    expected_identity: Option<StableFileIdentity>,
) -> Result<BoundedFile, BoundedReadError> {
    control.check()?;
    root.verify()?;
    let relative = root.relative(path)?;
    let mut file = open_relative_nofollow(&root.directory, &relative)?;
    let before = stable_file_identity(&file).map_err(BoundedReadError::Io)?;
    if expected_identity.is_some_and(|expected| expected != before) {
        return Err(BoundedReadError::SnapshotChanged);
    }
    let metadata = file.metadata().map_err(BoundedReadError::Io)?;
    if !metadata.file_type().is_file() {
        return Err(BoundedReadError::NotRegular);
    }
    if metadata.len() > max_bytes {
        return Err(BoundedReadError::TooLarge {
            size: metadata.len(),
            limit: max_bytes,
        });
    }

    let retained_limit = read_limit.min(max_bytes);
    let capacity = usize::try_from(metadata.len().min(retained_limit)).unwrap_or(0);
    let mut bytes = Vec::with_capacity(capacity);
    let mut buffer = [0_u8; READ_CHUNK_BYTES];
    while (bytes.len() as u64) < retained_limit {
        control.check()?;
        let remaining = usize::try_from(retained_limit - bytes.len() as u64)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let count = file
            .read(&mut buffer[..remaining])
            .map_err(BoundedReadError::Io)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    control.check()?;
    let after = stable_file_identity(&file).map_err(BoundedReadError::Io)?;
    let current = open_relative_nofollow(&root.directory, &relative)?;
    let current_identity = stable_file_identity(&current).map_err(BoundedReadError::Io)?;
    root.verify()?;
    if before != after || after != current_identity {
        return Err(BoundedReadError::SnapshotChanged);
    }
    Ok(BoundedFile {
        bytes,
        declared_len: metadata.len(),
        modified_millis: modified_millis(&metadata),
        identity: after,
    })
}

pub(crate) fn copy_regular_file_with_capability(
    root: &RootCapability,
    path: &Path,
    max_bytes: u64,
    control: ReadControl<'_>,
    expected_identity: Option<StableFileIdentity>,
    destination: &mut dyn Write,
) -> Result<BoundedFile, BoundedReadError> {
    control.check()?;
    root.verify()?;
    let relative = root.relative(path)?;
    let mut file = open_relative_nofollow(&root.directory, &relative)?;
    let before = stable_file_identity(&file).map_err(BoundedReadError::Io)?;
    if expected_identity.is_some_and(|expected| expected != before) {
        return Err(BoundedReadError::SnapshotChanged);
    }
    let metadata = file.metadata().map_err(BoundedReadError::Io)?;
    if !metadata.file_type().is_file() {
        return Err(BoundedReadError::NotRegular);
    }
    if metadata.len() > max_bytes {
        return Err(BoundedReadError::TooLarge {
            size: metadata.len(),
            limit: max_bytes,
        });
    }
    let mut copied = 0_u64;
    let mut buffer = [0_u8; READ_CHUNK_BYTES];
    loop {
        control.check()?;
        let count = file.read(&mut buffer).map_err(BoundedReadError::Io)?;
        if count == 0 {
            break;
        }
        copied = copied
            .checked_add(count as u64)
            .ok_or(BoundedReadError::TooLarge {
                size: u64::MAX,
                limit: max_bytes,
            })?;
        if copied > max_bytes {
            return Err(BoundedReadError::TooLarge {
                size: copied,
                limit: max_bytes,
            });
        }
        destination
            .write_all(&buffer[..count])
            .map_err(BoundedReadError::Io)?;
    }
    if copied != metadata.len() {
        return Err(BoundedReadError::SnapshotChanged);
    }
    control.check()?;
    let after = stable_file_identity(&file).map_err(BoundedReadError::Io)?;
    let current = open_relative_nofollow(&root.directory, &relative)?;
    let current_identity = stable_file_identity(&current).map_err(BoundedReadError::Io)?;
    root.verify()?;
    if before != after || after != current_identity {
        return Err(BoundedReadError::SnapshotChanged);
    }
    Ok(BoundedFile {
        bytes: Vec::new(),
        declared_len: copied,
        modified_millis: modified_millis(&metadata),
        identity: after,
    })
}

/// Enumerate one repository-owned directory without following any path
/// component. The caller supplies the aggregate entry cap so a walk can share
/// one deterministic work limit across directories.
pub(crate) fn read_directory_names(
    root: &Path,
    path: &Path,
    remaining_entries: usize,
    control: ReadControl<'_>,
) -> Result<Vec<std::ffi::OsString>, BoundedReadError> {
    let root = RootCapability::open(root)?;
    read_directory_names_with_capability(&root, path, remaining_entries, control)
}

pub(crate) fn read_directory_names_with_capability(
    root: &RootCapability,
    path: &Path,
    remaining_entries: usize,
    control: ReadControl<'_>,
) -> Result<Vec<std::ffi::OsString>, BoundedReadError> {
    control.check()?;
    root.verify()?;
    let relative = if path == root.requested_root || path == root.canonical_root {
        PathBuf::new()
    } else {
        root.relative(path)?
    };
    let directory = open_relative_directory_nofollow(&root.directory, &relative)?;
    let before = stable_file_identity(
        &directory
            .try_clone()
            .map_err(BoundedReadError::Io)?
            .into_std_file(),
    )
    .map_err(BoundedReadError::Io)?;
    let mut names = Vec::new();
    for entry in directory.entries().map_err(BoundedReadError::Io)? {
        control.check()?;
        if names.len() >= remaining_entries {
            return Err(BoundedReadError::TooLarge {
                size: names.len().saturating_add(1) as u64,
                limit: remaining_entries as u64,
            });
        }
        names.push(entry.map_err(BoundedReadError::Io)?.file_name());
    }
    names.sort();
    control.check()?;
    let after = stable_file_identity(
        &directory
            .try_clone()
            .map_err(BoundedReadError::Io)?
            .into_std_file(),
    )
    .map_err(BoundedReadError::Io)?;
    let current = open_relative_directory_nofollow(&root.directory, &relative)?;
    let current_identity = stable_file_identity(
        &current
            .try_clone()
            .map_err(BoundedReadError::Io)?
            .into_std_file(),
    )
    .map_err(BoundedReadError::Io)?;
    root.verify()?;
    if before != after || after != current_identity {
        return Err(BoundedReadError::SnapshotChanged);
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_reader_reads_regular_relative_file() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/lib.rs"), b"fn main() {}\n").unwrap();
        let read = read_regular_file(
            root.path(),
            Path::new("src/lib.rs"),
            1024,
            1024,
            ReadControl::default(),
        )
        .unwrap();
        assert_eq!(read.bytes, b"fn main() {}\n");
    }

    #[test]
    fn relative_root_joined_path_is_not_prefixed_twice() {
        let current = std::env::current_dir().unwrap();
        let root = tempfile::tempdir_in(&current).unwrap();
        let relative_root = root.path().strip_prefix(&current).unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/lib.rs"), b"fn main() {}\n").unwrap();

        let capability = RootCapability::open(relative_root).unwrap();
        let read = read_regular_file_with_capability(
            &capability,
            &relative_root.join("src/lib.rs"),
            1024,
            1024,
            ReadControl::default(),
        )
        .unwrap();
        assert_eq!(read.bytes, b"fn main() {}\n");
    }

    #[cfg(unix)]
    #[test]
    fn root_replacement_with_symlink_fails_closed() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("root");
        let moved = parent.path().join("moved");
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("safe.rs"), b"safe").unwrap();
        std::fs::write(outside.path().join("safe.rs"), b"outside").unwrap();
        let capability = RootCapability::open(&root).unwrap();

        std::fs::rename(&root, &moved).unwrap();
        symlink(outside.path(), &root).unwrap();
        assert!(matches!(
            read_regular_file_with_capability(
                &capability,
                &root.join("safe.rs"),
                1024,
                1024,
                ReadControl::default(),
            ),
            Err(BoundedReadError::SnapshotChanged)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn bounded_reader_rejects_symlink_components_and_special_files() {
        use std::os::unix::fs::{symlink, FileTypeExt};

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.rs"), b"secret").unwrap();
        symlink(outside.path(), root.path().join("linked")).unwrap();
        assert!(read_regular_file(
            root.path(),
            Path::new("linked/secret.rs"),
            1024,
            1024,
            ReadControl::default(),
        )
        .is_err());

        let fifo = root.path().join("pipe.rs");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(std::fs::symlink_metadata(&fifo)
            .unwrap()
            .file_type()
            .is_fifo());
        assert!(matches!(
            read_regular_file(
                root.path(),
                Path::new("pipe.rs"),
                1024,
                1024,
                ReadControl::default(),
            ),
            Err(BoundedReadError::NotRegular)
        ));
    }
}
