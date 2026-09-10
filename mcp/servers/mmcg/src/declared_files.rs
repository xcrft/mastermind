//! Shared admission for declared files, without reading their contents.

use crate::bounded_fs::{self, BoundedReadError, ReadControl, RootCapability, StableFileIdentity};
use crate::spec::ParsedSpec;
use std::collections::BTreeMap;
use std::path::Path;

const MAX_DECLARATIONS: usize = 1024;
const MAX_WORK: usize = 16_384;
pub(crate) const MAX_PATH_BYTES: usize = 4096;
pub(crate) const MAX_PATH_COMPONENTS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Issue {
    pub file: Option<String>,
    pub reason: &'static str,
}

#[derive(Clone, Copy)]
enum Role {
    Touch,
    Document,
    Mention,
}

fn declarations(spec: &ParsedSpec) -> impl Iterator<Item = (&str, Role)> {
    let frontmatter = spec.frontmatter.as_ref().filter(|fm| fm.has_file_scope());
    frontmatter
        .into_iter()
        .flat_map(|fm| {
            fm.touches
                .iter()
                .map(|touch| (touch.file.as_str(), Role::Touch))
                .chain(
                    fm.expected_docs
                        .iter()
                        .map(|file| (file.as_str(), Role::Document)),
                )
        })
        .chain(
            spec.mentioned_files
                .iter()
                .filter(move |_| frontmatter.is_none())
                .map(|file| (file.as_str(), Role::Mention)),
        )
}

pub(crate) fn paths(spec: &ParsedSpec) -> impl Iterator<Item = &str> {
    declarations(spec).map(|(file, _)| file)
}

pub(crate) fn normalize(file: &str) -> Result<String, &'static str> {
    if file.len() > MAX_PATH_BYTES {
        return Err("path_limit_exceeded");
    }
    let file = crate::spec_symbols::normalize_file(file).map_err(|_| "target_path_invalid")?;
    if file.chars().any(char::is_control)
        || (file.as_bytes().get(1) == Some(&b':') && file.as_bytes()[0].is_ascii_alphabetic())
    {
        return Err("target_path_invalid");
    }
    let relative = crate::audit_bundle::normalize_relative_path(Path::new(&file))
        .map_err(|_| "target_path_invalid")?;
    if Path::new(&relative).components().count() > MAX_PATH_COMPONENTS {
        return Err("path_limit_exceeded");
    }
    Ok(relative)
}

struct Target {
    display: String,
    touched: bool,
    document: bool,
}

struct Receipt {
    relative: String,
    display: String,
    identity: Option<StableFileIdentity>,
}

struct Checker<'a> {
    root: Result<RootCapability, &'static str>,
    control: ReadControl<'a>,
    work: usize,
}

fn reason(error: BoundedReadError) -> &'static str {
    match error {
        BoundedReadError::InvalidPath | BoundedReadError::OutsideRoot => "target_path_invalid",
        BoundedReadError::NotRegular => "target_not_regular",
        BoundedReadError::SnapshotChanged => "target_changed",
        BoundedReadError::Interrupted => "interrupted",
        BoundedReadError::DeadlineExceeded => "deadline_exceeded",
        BoundedReadError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            "target_missing"
        }
        BoundedReadError::Io(_) => "target_unavailable",
        BoundedReadError::TooLarge { .. } => "target_unavailable",
    }
}

fn issue(file: &str, reason: &'static str) -> Issue {
    Issue {
        file: (!file.is_empty() && file.len() <= MAX_PATH_BYTES).then(|| file.into()),
        reason,
    }
}

pub(crate) fn check(
    spec: &ParsedSpec,
    root: &Path,
    control: ReadControl<'_>,
    accepts_deletion: impl Fn(&str) -> bool,
) -> Vec<Issue> {
    let mut targets: BTreeMap<String, Target> = BTreeMap::new();
    let mut issues = Vec::new();
    let mut count = 0;
    for (index, (file, role)) in declarations(spec).enumerate() {
        if index == MAX_DECLARATIONS {
            return vec![Issue {
                file: None,
                reason: "declaration_limit_exceeded",
            }];
        }
        count += 1;
        let normalized = control
            .check()
            .map_err(reason)
            .and_then(|()| normalize(file));
        match normalized {
            Ok(relative) => {
                let target = targets.entry(relative).or_insert_with(|| Target {
                    display: file.into(),
                    touched: false,
                    document: false,
                });
                target.touched |= matches!(role, Role::Touch);
                target.document |= matches!(role, Role::Document);
            }
            Err(reason) => issues.push(issue(file, reason)),
        }
    }
    if targets.is_empty() {
        return issues;
    }
    let mut checker = Checker {
        root: control
            .check()
            .map_err(reason)
            .and_then(|()| RootCapability::open(root).map_err(|_| "root_unavailable")),
        control,
        work: MAX_WORK - count,
    };
    let (receipts, failed) = checker.collect(targets, accepts_deletion);
    issues.extend(failed);
    issues.extend(checker.finish(receipts));
    issues
}

impl Checker<'_> {
    fn inspect(
        &mut self,
        relative: &str,
        expected: Option<StableFileIdentity>,
    ) -> Result<StableFileIdentity, &'static str> {
        self.control.check().map_err(reason)?;
        self.work = self
            .work
            .checked_sub(1 + Path::new(relative).components().count())
            .ok_or("work_budget_exhausted")?;
        let root = self.root.as_ref().map_err(|reason| *reason)?;
        // Zero retained bytes checks openability, type and stable metadata. Large
        // and binary assets do not inherit FIND's content or UTF-8 limits.
        bounded_fs::read_regular_file_expected(
            root,
            &root.requested_root().join(relative),
            u64::MAX,
            0,
            self.control,
            expected,
        )
        .map(|file| file.identity)
        .map_err(reason)
    }

    fn collect(
        &mut self,
        targets: BTreeMap<String, Target>,
        accepts_deletion: impl Fn(&str) -> bool,
    ) -> (Vec<Receipt>, Vec<Issue>) {
        let mut receipts = Vec::new();
        let mut issues = Vec::new();
        for (relative, target) in targets {
            let identity = match self.inspect(&relative, None) {
                Ok(identity) => Some(identity),
                Err("target_missing")
                    if target.touched && !target.document && accepts_deletion(&relative) =>
                {
                    None
                }
                Err(reason) => {
                    issues.push(issue(&target.display, reason));
                    continue;
                }
            };
            receipts.push(Receipt {
                relative,
                display: target.display,
                identity,
            });
        }
        (receipts, issues)
    }

    fn finish(&mut self, receipts: Vec<Receipt>) -> Vec<Issue> {
        let mut issues = Vec::new();
        for receipt in &receipts {
            let checked = match (
                receipt.identity,
                self.inspect(&receipt.relative, receipt.identity),
            ) {
                (Some(_), Ok(_)) | (None, Err("target_missing")) => Ok(()),
                (None, Ok(_)) | (Some(_), Err("target_missing")) => Err("target_changed"),
                (_, Err(reason)) => Err(reason),
            };
            if let Err(reason) = checked {
                issues.push(issue(&receipt.display, reason));
            }
        }
        let final_check = self.control.check().map_err(reason).and_then(|()| {
            self.root
                .as_ref()
                .map_err(|reason| *reason)?
                .verify()
                .map_err(|_| "root_changed")?;
            self.control.check().map_err(reason)
        });
        if let Err(reason) = final_check {
            return receipts
                .iter()
                .map(|receipt| issue(&receipt.display, reason))
                .collect();
        }
        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::time::Instant;

    fn spec(touches: &[&str], docs: &[&str]) -> ParsedSpec {
        crate::spec::parse_str("spec.md", &format!("---\n{}---\n## Goals\nUpdate declared files.\n",
            serde_norway::to_string(&serde_json::json!({
                "mode": "lite", "touches": touches.iter().map(|file| serde_json::json!({"file": file})).collect::<Vec<_>>(),
                "expected_docs": docs,
            })).unwrap()))
    }

    fn checker<'a>(root: &Path, control: ReadControl<'a>) -> Checker<'a> {
        Checker {
            root: RootCapability::open(root).map_err(|_| "root_unavailable"),
            control,
            work: MAX_WORK,
        }
    }

    fn targets(file: &str, document: bool) -> BTreeMap<String, Target> {
        BTreeMap::from([(
            file.into(),
            Target {
                display: file.into(),
                touched: true,
                document,
            },
        )])
    }

    #[test]
    fn declared_files_preserve_binary_size_and_scope_authority() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("asset.bin");
        std::fs::write(&file, [0xff, 0, 0xfe]).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&file)
            .unwrap()
            .set_len(crate::indexer::MAX_INDEXABLE_FILE_SIZE + 1)
            .unwrap();
        let mut parsed = spec(&["./asset.bin"], &["asset.bin"]);
        parsed.mentioned_files = vec!["missing.md".into()];
        assert!(check(&parsed, root.path(), ReadControl::default(), |_| false).is_empty());
        let mut inspector = checker(root.path(), ReadControl::default());
        assert!(inspector.inspect("asset.bin", None).is_ok());
        parsed.frontmatter = None;
        assert_eq!(
            check(&parsed, root.path(), ReadControl::default(), |_| true),
            [issue("missing.md", "target_missing")]
        );
        assert!(
            check(&spec(&[], &[]), root.path(), ReadControl::default(), |_| {
                false
            })
            .is_empty()
        );
    }

    #[test]
    fn declared_files_reject_invalid_paths_and_bound_diagnostics() {
        let root = tempfile::tempdir().unwrap();
        for path in [
            "",
            ".",
            "..",
            "../file",
            "/file",
            "C:/file",
            "C:file",
            "\\\\host\\file",
            "a//b",
            "a/./b",
            "a/../b",
            "a\n.txt",
            "a\0.txt",
        ] {
            assert_eq!(
                check(
                    &spec(&[], &[path]),
                    root.path(),
                    ReadControl::default(),
                    |_| true
                ),
                [issue(path, "target_path_invalid")],
                "{path:?}"
            );
        }
        let oversized = "a".repeat(1024 * 1024 + 1);
        assert_eq!(
            check(
                &spec(&[], &[&oversized]),
                root.path(),
                ReadControl::default(),
                |_| true
            ),
            [Issue {
                file: None,
                reason: "path_limit_exceeded"
            }]
        );
        let deep = format!("{}file", "a/".repeat(MAX_PATH_COMPONENTS));
        assert_eq!(normalize(&deep), Err("path_limit_exceeded"));
        assert_eq!(normalize("./docs\\guide.md"), Ok("docs/guide.md".into()));
        assert_eq!(
            check(
                &spec(&[], &["file"; MAX_DECLARATIONS + 1]),
                root.path(),
                ReadControl::default(),
                |_| true
            ),
            [Issue {
                file: None,
                reason: "declaration_limit_exceeded"
            }]
        );
    }

    #[test]
    fn declared_files_require_proved_deletion_and_documents_override_aliases() {
        let root = tempfile::tempdir().unwrap();
        for (touches, docs, expected) in [
            (vec!["./src\\service.py"], vec![], true),
            (vec!["./src\\service.py"], vec!["src/service.py"], false),
            (vec!["src/service.py"], vec!["./src\\service.py"], false),
            (vec![], vec!["src/service.py"], false),
        ] {
            let parsed = spec(&touches, &docs);
            let issues = check(&parsed, root.path(), ReadControl::default(), |file| {
                file == "src/service.py"
            });
            assert_eq!(issues.is_empty(), expected, "{issues:?}");
            assert!(!check(&parsed, root.path(), ReadControl::default(), |_| false).is_empty());
        }
        std::fs::create_dir(root.path().join("service.py")).unwrap();
        let issues = check(
            &spec(&["service.py"], &[]),
            root.path(),
            ReadControl::default(),
            |_| true,
        );
        assert_eq!(issues.len(), 1);
        assert_ne!(issues[0].reason, "target_missing");
    }

    #[test]
    fn declared_files_recheck_present_and_missing_receipts() {
        for mutation in ["removed", "replaced", "directory", "appeared", "metadata"] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("file");
            if mutation != "appeared" {
                std::fs::write(&path, b"before").unwrap();
            }
            let mut inspector = checker(root.path(), ReadControl::default());
            let (receipts, issues) = inspector.collect(targets("file", false), |_| true);
            assert!(issues.is_empty());
            assert_eq!(receipts.len(), 1);
            match mutation {
                "removed" => std::fs::remove_file(&path).unwrap(),
                "replaced" => {
                    std::fs::rename(&path, root.path().join("old")).unwrap();
                    std::fs::write(&path, b"before").unwrap();
                }
                "directory" => {
                    std::fs::remove_file(&path).unwrap();
                    std::fs::create_dir(&path).unwrap();
                }
                "appeared" | "metadata" => std::fs::write(&path, b"new contents").unwrap(),
                _ => unreachable!(),
            }
            assert_eq!(inspector.finish(receipts).len(), 1, "{mutation}");
        }
    }

    #[test]
    fn declared_files_rechecks_share_work_deadline_and_interruption() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), b"contents").unwrap();
        let parsed = spec(&["file"], &[]);
        assert_eq!(
            check(
                &parsed,
                root.path(),
                ReadControl {
                    deadline: Some(Instant::now()),
                    interrupted: None
                },
                |_| false
            ),
            [issue("file", "deadline_exceeded")]
        );
        let mut inspector = checker(root.path(), ReadControl::default());
        inspector.work = 3;
        let (receipts, issues) = inspector.collect(targets("file", false), |_| false);
        assert!(issues.is_empty());
        assert_eq!(
            inspector.finish(receipts),
            [issue("file", "work_budget_exhausted")]
        );
        let interrupted = Cell::new(false);
        let callback = || interrupted.get();
        let mut inspector = checker(
            root.path(),
            ReadControl {
                deadline: None,
                interrupted: Some(&callback),
            },
        );
        let (receipts, issues) = inspector.collect(targets("file", false), |_| false);
        assert!(issues.is_empty());
        interrupted.set(true);
        assert_eq!(inspector.finish(receipts), [issue("file", "interrupted")]);
    }

    #[cfg(unix)]
    #[test]
    fn declared_files_reject_links_special_files_and_root_changes_unix() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("root");
        let outside = directory.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("file"), b"contents").unwrap();
        symlink(outside.join("file"), root.join("link")).unwrap();
        symlink(outside.join("absent"), root.join("dangling")).unwrap();
        symlink(&outside, root.join("parent")).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(root.join("pipe"))
            .status()
            .unwrap()
            .success());
        for file in ["link", "dangling", "parent/file", "parent/absent", "pipe"] {
            assert!(
                !check(&spec(&[file], &[]), &root, ReadControl::default(), |_| true).is_empty(),
                "{file}"
            );
        }
        let unreadable = root.join("private");
        std::fs::write(&unreadable, b"contents").unwrap();
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&unreadable).is_err() {
            assert_eq!(
                check(
                    &spec(&["private"], &[]),
                    &root,
                    ReadControl::default(),
                    |_| true
                ),
                [issue("private", "target_unavailable")]
            );
        }
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut inspector = checker(&root, ReadControl::default());
        let (receipts, issues) = inspector.collect(targets("private", false), |_| false);
        assert!(issues.is_empty());
        std::fs::rename(&root, directory.path().join("moved")).unwrap();
        symlink(&outside, &root).unwrap();
        assert_eq!(
            inspector.finish(receipts),
            [issue("private", "root_changed")]
        );
    }

    #[cfg(windows)]
    #[test]
    fn declared_files_reject_existing_and_dangling_junctions_windows() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let destination = outside.path().join("target");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("file"), b"contents").unwrap();
        let junction = root.path().join("junction");
        assert!(std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&destination)
            .status()
            .unwrap()
            .success());
        let parsed = spec(&["junction/file"], &[]);
        assert!(!check(&parsed, root.path(), ReadControl::default(), |_| true).is_empty());
        std::fs::remove_file(destination.join("file")).unwrap();
        std::fs::remove_dir(&destination).unwrap();
        assert!(!check(&parsed, root.path(), ReadControl::default(), |_| true).is_empty());
        std::fs::remove_dir(&junction).unwrap();
    }
}
