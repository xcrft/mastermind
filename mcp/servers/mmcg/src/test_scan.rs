//! Advisory scans of conventional test locations, bounded across one report.

use crate::bounded_fs::{self, BoundedPathKind, ReadControl, RootCapability, StableFileIdentity};
use crate::verification::{test_command, TestArgument, TestRunner};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

const MAX_WORK: usize = 16_384;
const MAX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_DEPTH: usize = 12;
const MAX_SCOPE_COMPONENTS: usize = 64;

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Present,
    Absent,
    Unknown,
}

struct Plan {
    runner: TestRunner,
    path: PathBuf,
    manifest: Option<PathBuf>,
    recursive: bool,
}

impl Plan {
    fn parse(cmd: &str) -> Option<Self> {
        let command = test_command(cmd)?;
        let mut filters = Vec::new();
        let mut manifest = None;
        for argument in command.arguments {
            match argument {
                TestArgument::Filter(value) => filters.push(value),
                TestArgument::Value("--manifest-path", value) => {
                    if manifest.replace(PathBuf::from(value)).is_some() {
                        return None;
                    }
                }
                TestArgument::Switch("--workspace" | "--all" | "--doc")
                | TestArgument::Value("--package" | "-p" | "--exclude", _) => return None,
                _ => {}
            }
        }
        let (path, recursive) = match command.runner {
            TestRunner::Cargo => {
                let path = match &manifest {
                    Some(path) if path.file_name()? == "Cargo.toml" => path.parent()?.to_path_buf(),
                    Some(_) => return None,
                    None => PathBuf::from("."),
                };
                (path, true)
            }
            TestRunner::Go => match filters.as_slice() {
                [] | ["."] => (PathBuf::from("."), false),
                [path] if path.starts_with("./") => match path.strip_suffix("/...") {
                    Some(base) if !base.contains("...") => (PathBuf::from(base), true),
                    Some(_) => return None,
                    None if !path.contains("...") => (PathBuf::from(path), false),
                    None => return None,
                },
                _ => return None,
            },
            TestRunner::Pytest => match filters.as_slice() {
                [] => (PathBuf::from("."), true),
                [path] if !path.contains("::") => (PathBuf::from(path), true),
                _ => return None,
            },
            TestRunner::Javascript if filters.is_empty() => (PathBuf::from("."), true),
            TestRunner::Javascript => return None,
        };
        Some(Self {
            runner: command.runner,
            path,
            manifest,
            recursive,
        })
    }

    fn absence_reason(&self) -> String {
        let scope = if self.path.as_os_str().is_empty() {
            ".".to_string()
        } else {
            self.path.to_string_lossy().replace('\\', "/")
        };
        match self.runner {
            TestRunner::Cargo => format!(
                "no conventional Rust test attributes found in {scope}/src/ or {scope}/tests/ (or crate root when both are absent)"
            ),
            TestRunner::Go => format!("no conventional *_test.go files found in {scope}"),
            TestRunner::Pytest => {
                format!("no conventional test_*.py or *_test.py files found in {scope}")
            }
            TestRunner::Javascript => {
                format!("no conventional *.test.* or *.spec.* files found in {scope}")
            }
        }
    }
}

#[derive(Default)]
struct Snapshot {
    directories: Vec<(PathBuf, Vec<OsString>)>,
    kinds: Vec<(PathBuf, BoundedPathKind)>,
    files: Vec<(PathBuf, StableFileIdentity, String)>,
}

enum Collected {
    Present,
    Absent(Snapshot),
}

pub(crate) struct TestScanner<'a> {
    root: Option<RootCapability>,
    control: ReadControl<'a>,
    work: usize,
    bytes: u64,
}

impl<'a> TestScanner<'a> {
    pub(crate) fn new(root: &Path, control: ReadControl<'a>) -> Self {
        Self {
            root: control
                .check()
                .ok()
                .and_then(|()| RootCapability::open(root).ok()),
            control,
            work: MAX_WORK,
            bytes: MAX_BYTES,
        }
    }

    pub(crate) fn absence_reason(&mut self, cmd: &str) -> Option<String> {
        let plan = Plan::parse(cmd)?;
        (self.scan(&plan) == Outcome::Absent).then(|| plan.absence_reason())
    }

    fn charge(&mut self) -> Option<()> {
        self.control.check().ok()?;
        self.work = self.work.checked_sub(1)?;
        Some(())
    }

    fn resolve(&self, path: &Path) -> Option<PathBuf> {
        if path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        {
            return None;
        }
        let root = self.root.as_ref()?;
        let path = root.requested_root().join(path);
        // Keep cwd out of RootCapability's relative-input compatibility path.
        let relative = path
            .strip_prefix(root.requested_root())
            .or_else(|_| path.strip_prefix(root.canonical_root()))
            .ok()?;
        if relative.components().count() > MAX_SCOPE_COMPONENTS {
            return None;
        }
        let mut resolved = root.requested_root().to_path_buf();
        for component in relative.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(name) => resolved.push(name),
                _ => return None,
            }
        }
        Some(resolved)
    }

    fn kind(&mut self, path: &Path) -> Option<BoundedPathKind> {
        self.charge()?;
        let root = self.root.as_ref()?;
        if path == root.requested_root() {
            root.verify().ok()?;
            return Some(BoundedPathKind::Directory);
        }
        let kind = bounded_fs::inspect_path_kind_with_capability(root, path, self.control).ok()?;
        (kind != BoundedPathKind::Other).then_some(kind)
    }

    fn names(&mut self, path: &Path) -> Option<Vec<OsString>> {
        self.charge()?;
        let allowance = std::mem::take(&mut self.work);
        let names = bounded_fs::read_directory_names_with_capability(
            self.root.as_ref()?,
            path,
            allowance,
            self.control,
        )
        .ok()?;
        self.work = allowance.checked_sub(names.len())?;
        Some(names)
    }

    fn source(
        &mut self,
        path: &Path,
        expected: Option<StableFileIdentity>,
    ) -> Option<bounded_fs::BoundedFile> {
        self.charge()?;
        let reservation = self.bytes.min(MAX_FILE_BYTES);
        if reservation == 0 {
            return None;
        }
        self.bytes -= reservation;
        let file = bounded_fs::read_regular_file_expected(
            self.root.as_ref()?,
            path,
            reservation,
            reservation,
            self.control,
            expected,
        )
        .ok()?;
        if file.bytes.len() as u64 != file.declared_len {
            return None;
        }
        self.bytes += reservation - file.declared_len;
        Some(file)
    }

    fn scan(&mut self, plan: &Plan) -> Outcome {
        match self.collect(plan) {
            Some(Collected::Present) => Outcome::Present,
            Some(Collected::Absent(snapshot)) => {
                if self.verify(snapshot).is_some() {
                    Outcome::Absent
                } else {
                    Outcome::Unknown
                }
            }
            _ => Outcome::Unknown,
        }
    }

    fn collect(&mut self, plan: &Plan) -> Option<Collected> {
        let base = self.resolve(&plan.path)?;
        let mut snapshot = Snapshot::default();
        if let Some(manifest) = &plan.manifest {
            let manifest = self.resolve(manifest)?;
            if self.kind(&manifest)? != BoundedPathKind::RegularFile {
                return None;
            }
            snapshot
                .kinds
                .push((manifest, BoundedPathKind::RegularFile));
        }
        let kind = self.kind(&base)?;
        snapshot.kinds.push((base.clone(), kind));
        if kind == BoundedPathKind::RegularFile {
            // An explicit Python file need not follow default discovery names.
            return (plan.runner == TestRunner::Pytest && base.extension()? == "py")
                .then_some(Collected::Present);
        }
        let mut pending = Vec::new();
        if plan.runner == TestRunner::Cargo {
            let names = self.names(&base)?;
            for name in ["src", "tests"] {
                if names.iter().any(|entry| entry == name) {
                    let path = base.join(name);
                    let kind = self.kind(&path)?;
                    snapshot.kinds.push((path.clone(), kind));
                    if kind != BoundedPathKind::Directory {
                        return None;
                    }
                    pending.push((path, 0));
                }
            }
            if pending.is_empty() {
                // Re-enumeration is charged like every other read.
                pending.push((base.clone(), 0));
            }
            snapshot.directories.push((base, names));
        } else {
            pending.push((base, 0));
        }
        while let Some((directory, depth)) = pending.pop() {
            let names = self.names(&directory)?;
            for name in &names {
                // Git metadata is not a conventional test location.
                if name == ".git" {
                    continue;
                }
                let path = directory.join(name);
                let kind = self.kind(&path)?;
                snapshot.kinds.push((path.clone(), kind));
                match kind {
                    BoundedPathKind::Directory if plan.recursive => {
                        if depth == MAX_DEPTH {
                            return None;
                        }
                        pending.push((path, depth + 1));
                    }
                    BoundedPathKind::RegularFile if plan.runner == TestRunner::Cargo => {
                        if path.extension().is_some_and(|extension| extension == "rs") {
                            let source = self.source(&path, None)?;
                            let text = std::str::from_utf8(&source.bytes).ok()?;
                            if file_has_test_attr(text) {
                                return Some(Collected::Present);
                            }
                            snapshot.files.push((
                                path,
                                source.identity,
                                crate::hex::encode(&Sha256::digest(&source.bytes)),
                            ));
                        }
                    }
                    BoundedPathKind::RegularFile => {
                        let name = name.to_str()?;
                        let matches = match plan.runner {
                            TestRunner::Go => name.ends_with("_test.go"),
                            TestRunner::Pytest => {
                                (name.starts_with("test_") || name.ends_with("_test.py"))
                                    && name.ends_with(".py")
                            }
                            TestRunner::Javascript => {
                                name.contains(".test.") || name.contains(".spec.")
                            }
                            TestRunner::Cargo => unreachable!(),
                        };
                        if matches {
                            return Some(Collected::Present);
                        }
                    }
                    _ => {}
                }
            }
            snapshot.directories.push((directory, names));
        }
        Some(Collected::Absent(snapshot))
    }

    fn verify(&mut self, snapshot: Snapshot) -> Option<()> {
        for (path, kind) in snapshot.kinds {
            if self.kind(&path)? != kind {
                return None;
            }
        }
        for (path, identity, hash) in snapshot.files {
            if crate::hex::encode(&Sha256::digest(&self.source(&path, Some(identity))?.bytes))
                != hash
            {
                return None;
            }
        }
        for (path, names) in snapshot.directories {
            if self.names(&path)? != names {
                return None;
            }
        }
        self.control.check().ok()?;
        self.root.as_ref()?.verify().ok()?;
        Some(())
    }
}

fn file_has_test_attr(text: &str) -> bool {
    text.contains("#[test]") || text.contains("::test]") || text.contains("#[rstest")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn write(root: &Path, path: &str, bytes: &[u8]) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn scanner(root: &Path) -> TestScanner<'static> {
        TestScanner::new(
            root,
            ReadControl {
                deadline: Some(Instant::now() + Duration::from_secs(20)),
                interrupted: None,
            },
        )
    }

    #[test]
    fn file_has_test_attr_recognises_common_spellings() {
        for text in [
            "#[test]\nfn t() {}",
            "#[tokio::test]\nasync fn t() {}",
            "#[async_std::test]\nasync fn t() {}",
            "#[rstest]\nfn t() {}",
            "#[rstest(input, case(1))]\nfn t(input: u8) {}",
        ] {
            assert!(file_has_test_attr(text));
        }
        assert!(!file_has_test_attr("fn helper() {}\n// no tests here"));
    }

    #[test]
    fn cargo_test_not_vacuous_with_only_integration_tests() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "src/lib.rs", b"pub fn add() {}\n");
        write(root.path(), "tests/check.rs", b"#[test]\nfn check() {}\n");
        assert!(scanner(root.path()).absence_reason("cargo test").is_none());
        std::fs::remove_file(root.path().join("tests/check.rs")).unwrap();
        assert!(scanner(root.path()).absence_reason("cargo test").is_some());
    }

    #[test]
    fn cargo_test_manifest_path_scopes_scan_to_the_crate() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "tests/fixture.py",
            b"def test_fixture(): pass\n",
        );
        write(
            root.path(),
            "crates/app/Cargo.toml",
            b"[package]\nname = \"app\"\n",
        );
        write(
            root.path(),
            "crates/app/src/lib.rs",
            b"#[test]\nfn check() {}\n",
        );
        for cmd in [
            "cargo test --manifest-path crates/app/Cargo.toml --locked check",
            "cargo test --manifest-path=./crates/app/Cargo.toml",
        ] {
            assert!(scanner(root.path()).absence_reason(cmd).is_none());
        }
        assert!(scanner(root.path()).absence_reason("cargo test").is_some());
        assert!(scanner(root.path())
            .absence_reason("cargo test --manifest-path absent/Cargo.toml")
            .is_none());
        write(root.path(), "tests/decoy.rs", b"#[test]\nfn decoy() {}\n");
        write(
            root.path(),
            "crates/app/src/lib.rs",
            b"pub fn helper() {}\n",
        );
        let reason = scanner(root.path())
            .absence_reason("cargo test --manifest-path=crates/app/Cargo.toml")
            .unwrap();
        assert!(reason.contains("crates/app/src/"), "{reason}");
    }

    #[test]
    fn test_scan_scope_parsing_keeps_option_values_out_of_targets() {
        for (cmd, path, recursive) in [
            ("go test -run ./decoy ./pkg", "./pkg", false),
            ("go test -count=1 ./pkg/...", "./pkg", true),
            ("go test ./...", ".", true),
            ("pytest -k decoy ./suite", "./suite", true),
            ("python3 -m pytest -m marker suite", "suite", true),
            (
                "cargo test --features test --manifest-path crates/app/Cargo.toml filter",
                "crates/app",
                true,
            ),
        ] {
            let plan = Plan::parse(cmd).unwrap();
            assert_eq!(plan.path, Path::new(path), "{cmd}");
            assert_eq!(plan.recursive, recursive, "{cmd}");
        }
        for cmd in [
            "go test std",
            "go test ./one ./two",
            "go test ./.../pkg/...",
            "pytest one two",
            "pytest tests/test_app.py::test_app",
            "jest src",
            "vitest run src",
            "cargo test --workspace",
            "cargo test -p app",
            "cargo test --doc",
            "cargo test --manifest-path crates/app",
            "cargo test --manifest-path Cargo.toml --manifest-path other/Cargo.toml",
        ] {
            assert!(Plan::parse(cmd).is_none(), "{cmd}");
        }
    }

    #[test]
    fn test_scan_resolve_bounds_starting_paths_and_root_aliases() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let scan = scanner(root.path());
        let root = scan.root.as_ref().unwrap().requested_root();
        assert_eq!(scan.resolve(Path::new("./.")), Some(root.to_path_buf()));
        assert_eq!(scan.resolve(root), Some(root.to_path_buf()));
        assert_eq!(scan.resolve(&root.join("src")), Some(root.join("src")));
        assert!(scan.resolve(outside.path()).is_none());
        assert!(scan.resolve(Path::new("../outside")).is_none());
        assert!(scan.resolve(Path::new("src/../src")).is_none());
        assert!(scan.resolve(Path::new(&"nested/".repeat(80))).is_none());
    }

    #[test]
    fn test_scan_limits_charge_failed_enumerations_and_repeated_rows() {
        let root = tempfile::tempdir().unwrap();
        for i in 0..10 {
            write(root.path(), &format!("helper{i}.py"), b"pass\n");
        }
        let plan = Plan::parse("pytest").unwrap();
        let mut limited = scanner(root.path());
        limited.work = 4;
        assert_eq!(limited.scan(&plan), Outcome::Unknown);
        assert_eq!(limited.work, 0);
        assert_eq!(limited.scan(&plan), Outcome::Unknown);

        let empty = tempfile::tempdir().unwrap();
        let mut limited = scanner(empty.path());
        limited.work = 64;
        assert_eq!(limited.scan(&plan), Outcome::Absent);
        let mut exhausted = false;
        for _ in 0..64 {
            if limited.scan(&plan) == Outcome::Unknown {
                exhausted = true;
                break;
            }
        }
        assert!(
            exhausted,
            "empty-directory rows must not reset the operation budget"
        );
    }

    #[test]
    fn test_scan_byte_limits_include_revalidation_and_failed_reads() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "src/lib.rs", b"// empty\n");
        let plan = Plan::parse("cargo test").unwrap();
        let mut limited = scanner(root.path());
        limited.bytes = 12;
        assert_eq!(limited.scan(&plan), Outcome::Unknown);
        assert_eq!(limited.bytes, 0);
        assert_eq!(limited.scan(&plan), Outcome::Unknown);
        let mut enough = scanner(root.path());
        enough.bytes = 18;
        assert_eq!(enough.scan(&plan), Outcome::Absent);
        assert_eq!(enough.bytes, 0);
        assert_eq!(enough.scan(&plan), Outcome::Unknown);

        let path = root.path().join("src/lib.rs");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        drop(file);
        let mut limited = scanner(root.path());
        let before = limited.bytes;
        assert_eq!(limited.scan(&plan), Outcome::Unknown);
        assert_eq!(before - limited.bytes, MAX_FILE_BYTES);
        write(root.path(), "src/lib.rs", b"\xff");
        assert_eq!(scanner(root.path()).scan(&plan), Outcome::Unknown);
    }

    #[test]
    fn test_scan_depth_deadline_and_cancellation_are_unknown() {
        let root = tempfile::tempdir().unwrap();
        let nested = format!("{}test_nested.py", "nested/".repeat(20));
        write(root.path(), &nested, b"def test_nested(): pass\n");
        let plan = Plan::parse("pytest").unwrap();
        assert_eq!(scanner(root.path()).scan(&plan), Outcome::Unknown);
        let mut expired = TestScanner::new(
            root.path(),
            ReadControl {
                deadline: Some(Instant::now()),
                interrupted: None,
            },
        );
        assert_eq!(expired.scan(&plan), Outcome::Unknown);
        let cancelled = std::cell::Cell::new(false);
        let interrupt = || cancelled.get();
        let mut scan = TestScanner::new(
            root.path(),
            ReadControl {
                deadline: None,
                interrupted: Some(&interrupt),
            },
        );
        cancelled.set(true);
        assert_eq!(scan.scan(&plan), Outcome::Unknown);
    }

    #[test]
    fn test_scan_rechecks_directory_kinds_and_source_receipts() {
        for mutation in ["add", "delete", "change", "replace_kind", "same_metadata"] {
            let root = tempfile::tempdir().unwrap();
            write(root.path(), "src/lib.rs", b"fn helper() {}\n");
            write(root.path(), "src/plain", b"plain\n");
            let mut scan = scanner(root.path());
            let Collected::Absent(snapshot) =
                scan.collect(&Plan::parse("cargo test").unwrap()).unwrap()
            else {
                panic!("fixture must be a complete conventional absence");
            };
            let file = root.path().join("src/lib.rs");
            match mutation {
                "add" => write(root.path(), "src/new.rs", b"#[test]\nfn t() {}\n"),
                "delete" => std::fs::remove_file(&file).unwrap(),
                "change" => std::fs::write(&file, b"#[test]\nfn t() {}\n").unwrap(),
                "replace_kind" => {
                    std::fs::remove_file(root.path().join("src/plain")).unwrap();
                    std::fs::create_dir(root.path().join("src/plain")).unwrap();
                }
                "same_metadata" => {
                    let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
                    std::fs::write(&file, b"fn change() {}\n").unwrap();
                    std::fs::File::options()
                        .write(true)
                        .open(&file)
                        .unwrap()
                        .set_times(std::fs::FileTimes::new().set_modified(modified))
                        .unwrap();
                }
                _ => unreachable!(),
            }
            assert!(scan.verify(snapshot).is_none(), "{mutation}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_scan_root_replacement_and_special_files_are_unknown_unix() {
        use std::os::unix::fs::symlink;
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("repo");
        let outside = parent.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        write(&root, "src/lib.rs", b"fn helper() {}\n");
        let plan = Plan::parse("cargo test").unwrap();
        let mut scan = scanner(&root);
        let Collected::Absent(snapshot) = scan.collect(&plan).unwrap() else {
            panic!("expected absence")
        };
        std::fs::rename(&root, parent.path().join("moved")).unwrap();
        symlink(&outside, &root).unwrap();
        assert!(scan.verify(snapshot).is_none());
        let root = parent.path().join("special");
        std::fs::create_dir(&root).unwrap();
        let fifo = root.join("pipe.rs");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        assert_eq!(scanner(&root).scan(&plan), Outcome::Unknown);
    }
}
