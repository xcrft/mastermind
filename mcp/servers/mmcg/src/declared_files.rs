//! Shared admission for declared files, without reading their contents.

use crate::bounded_fs::{self, BoundedReadError, ReadControl, RootCapability, StableFileIdentity};
use crate::spec::ParsedSpec;
use std::collections::{BTreeMap, BTreeSet};
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
    Create,
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
                .chain(fm.creates.iter().map(|file| (file.as_str(), Role::Create)))
                .chain(
                    fm.expected_docs
                        .iter()
                        .map(|file| (file.as_str(), Role::Document)),
                )
        })
        .chain(
            spec.mentioned_files
                .iter()
                .filter(move |_| frontmatter.is_none() && spec.frontmatter_error.is_none())
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
    created: bool,
    document: bool,
}

struct Receipt {
    relative: String,
    display: String,
    identity: Option<StableFileIdentity>,
    absence: Option<bounded_fs::AbsentPath>,
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

#[derive(Clone, Copy)]
enum Phase<'a> {
    Preflight,
    Postflight {
        baseline: &'a str,
        files: &'a [crate::diff::WorkingTreeChangedFile],
    },
}

pub(crate) fn preflight(spec: &ParsedSpec, root: &Path, control: ReadControl<'_>) -> Vec<Issue> {
    check(spec, root, control, Phase::Preflight, |_| false)
}

pub(crate) fn postflight(
    spec: &ParsedSpec,
    root: &Path,
    control: ReadControl<'_>,
    baseline: &str,
    files: &[crate::diff::WorkingTreeChangedFile],
    accepts_deletion: impl Fn(&str) -> bool,
) -> Vec<Issue> {
    check(
        spec,
        root,
        control,
        Phase::Postflight { baseline, files },
        accepts_deletion,
    )
}

fn check(
    spec: &ParsedSpec,
    root: &Path,
    control: ReadControl<'_>,
    phase: Phase<'_>,
    accepts_deletion: impl Fn(&str) -> bool,
) -> Vec<Issue> {
    if let Some(error) = &spec.frontmatter_error {
        return vec![issue(
            "",
            if error == "frontmatter_unterminated" {
                "frontmatter_unterminated"
            } else {
                "frontmatter_invalid"
            },
        )];
    }
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
                    created: false,
                    document: false,
                });
                target.touched |= matches!(role, Role::Touch);
                target.created |= matches!(role, Role::Create);
                target.document |= matches!(role, Role::Document);
            }
            Err(reason) => issues.push(issue(file, reason)),
        }
    }
    for (relative, target) in &targets {
        if target.created && target.touched {
            issues.push(issue(&target.display, "creation_touch_conflict"));
        }
        // Every declaration names a regular file, so it cannot also be a
        // directory needed by another declared target.
        for (index, _) in relative.match_indices('/') {
            if targets.contains_key(&relative[..index]) {
                issues.push(issue(&target.display, "declaration_ancestor_conflict"));
                break;
            }
        }
    }
    if !issues.is_empty() {
        return issues;
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
    if let Phase::Postflight { baseline, files } = phase {
        issues.extend(checker.creation_issues(&targets, baseline, files));
    }
    let (receipts, failed) =
        checker.collect(targets, matches!(phase, Phase::Preflight), accepts_deletion);
    issues.extend(failed);
    issues.extend(checker.finish(receipts));
    issues
}

impl Checker<'_> {
    fn charge(&mut self, work: usize) -> Result<(), &'static str> {
        self.control.check().map_err(reason)?;
        self.work = self.work.checked_sub(work).ok_or("work_budget_exhausted")?;
        Ok(())
    }

    fn absent(&mut self, relative: &str) -> Result<Option<bounded_fs::AbsentPath>, &'static str> {
        self.charge(1 + Path::new(relative).components().count())?;
        let root = self.root.as_ref().map_err(|reason| *reason)?;
        bounded_fs::inspect_absent_path(root, &root.requested_root().join(relative), self.control)
            .map_err(reason)
    }

    fn inspect(
        &mut self,
        relative: &str,
        expected: Option<StableFileIdentity>,
    ) -> Result<StableFileIdentity, &'static str> {
        self.charge(1 + Path::new(relative).components().count())?;
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
        allow_creates: bool,
        accepts_deletion: impl Fn(&str) -> bool,
    ) -> (Vec<Receipt>, Vec<Issue>) {
        let mut receipts = Vec::new();
        let mut issues = Vec::new();
        for (relative, target) in targets {
            let mut absence = None;
            let identity = match self.inspect(&relative, None) {
                Ok(identity) => Some(identity),
                Err("target_missing")
                    if (allow_creates && target.created)
                        || (target.touched
                            && !target.document
                            && !target.created
                            && accepts_deletion(&relative)) =>
                {
                    match self.absent(&relative) {
                        Ok(Some(proof)) => absence = Some(proof),
                        result => {
                            issues.push(issue(
                                &target.display,
                                result.err().unwrap_or("target_changed"),
                            ));
                            continue;
                        }
                    }
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
                absence,
            });
        }
        (receipts, issues)
    }

    fn finish(&mut self, receipts: Vec<Receipt>) -> Vec<Issue> {
        let mut issues = Vec::new();
        for receipt in &receipts {
            if let Some(expected) = &receipt.absence {
                let checked = match self.absent(&receipt.relative) {
                    Ok(Some(current)) if expected.matches(&current) => Ok(()),
                    Ok(_) => Err("target_changed"),
                    Err(reason) => Err(reason),
                };
                if let Err(reason) = checked {
                    issues.push(issue(&receipt.display, reason));
                }
                continue;
            }
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

    fn creation_issues(
        &mut self,
        targets: &BTreeMap<String, Target>,
        baseline: &str,
        files: &[crate::diff::WorkingTreeChangedFile],
    ) -> Vec<Issue> {
        let created = targets
            .iter()
            .filter(|(_, target)| target.created)
            .collect::<Vec<_>>();
        if created.is_empty() {
            return Vec::new();
        }
        let paths = created
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>();
        let existing = self.baseline_entries(&paths, baseline);
        let additions = files
            .iter()
            .filter(|file| matches!(file.status.as_str(), "added" | "untracked"))
            .map(|file| file.path.as_str())
            .collect::<BTreeSet<_>>();
        created
            .into_iter()
            .filter_map(|(path, target)| {
                let error = match &existing {
                    Err(reason) => Some(*reason),
                    Ok(existing) if existing.contains(path.as_str()) => {
                        Some("creation_exists_in_baseline")
                    }
                    Ok(_) if !additions.contains(path.as_str()) => Some("creation_not_added"),
                    Ok(_) => None,
                };
                error.map(|reason| issue(&target.display, reason))
            })
            .collect()
    }

    fn baseline_entries(
        &mut self,
        paths: &[&str],
        baseline: &str,
    ) -> Result<BTreeSet<String>, &'static str> {
        if !crate::diff::is_full_git_oid(baseline) {
            return Err("creation_baseline_unavailable");
        }
        let mut found = BTreeSet::new();
        let mut offset = 0;
        while offset < paths.len() {
            let mut end = offset;
            let mut bytes = 0;
            while end < paths.len() && end - offset < 64 {
                let next = paths[end].len() + 1;
                if end > offset && bytes + next > 8 * 1024 {
                    break;
                }
                bytes += next;
                end += 1;
            }
            self.charge(1 + end - offset)?;
            let root = self.root.as_ref().map_err(|reason| *reason)?;
            root.verify().map_err(reason)?;
            let mut args = vec![
                "--literal-pathspecs",
                "ls-tree",
                "-z",
                "--name-only",
                "--full-tree",
                baseline,
                "--",
            ];
            args.extend_from_slice(&paths[offset..end]);
            let output = crate::diff::run_bounded_git_with_control(
                root.requested_root(),
                &args,
                None,
                32 * 1024,
                self.control.deadline,
                self.control.interrupted,
            )
            .map_err(|_| "creation_baseline_unavailable")?;
            self.control.check().map_err(reason)?;
            root.verify().map_err(reason)?;
            if !output.success || (!output.stdout.is_empty() && output.stdout.last() != Some(&0)) {
                return Err("creation_baseline_unavailable");
            }
            for entry in output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|entry| !entry.is_empty())
            {
                let path =
                    std::str::from_utf8(entry).map_err(|_| "creation_baseline_unavailable")?;
                if !paths[offset..end].contains(&path) || !found.insert(path.to_string()) {
                    return Err("creation_baseline_unavailable");
                }
            }
            offset = end;
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::time::Instant;

    fn check(
        spec: &ParsedSpec,
        root: &Path,
        control: ReadControl<'_>,
        accepts_deletion: impl Fn(&str) -> bool,
    ) -> Vec<Issue> {
        postflight(spec, root, control, "", &[], accepts_deletion)
    }

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
                created: false,
                document,
            },
        )])
    }

    fn creation_spec(touches: &[&str], creates: &[&str], docs: &[&str]) -> ParsedSpec {
        let mut parsed = spec(touches, docs);
        parsed.frontmatter.as_mut().unwrap().creates =
            creates.iter().map(|file| file.to_string()).collect();
        parsed
    }

    #[test]
    fn creates_preflight_admits_safe_absence_and_regular_drafts() {
        let root = tempfile::tempdir().unwrap();
        let mut parsed = creation_spec(&[], &["./new\\api.py", "asset.bin"], &["new/api.py"]);
        parsed.mentioned_files = vec!["unrelated.md".into()];
        std::fs::write(root.path().join("asset.bin"), [0xff, 0, 0xfe]).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(root.path().join("asset.bin"))
            .unwrap()
            .set_len(crate::indexer::MAX_INDEXABLE_FILE_SIZE + 1)
            .unwrap();
        assert!(preflight(&parsed, root.path(), ReadControl::default()).is_empty());
        assert!(!root.path().join("new").exists());
        std::fs::create_dir(root.path().join("new")).unwrap();
        std::fs::write(root.path().join("new/api.py"), "def added(): pass\n").unwrap();
        assert!(preflight(&parsed, root.path(), ReadControl::default()).is_empty());
        parsed.frontmatter.as_mut().unwrap().creates.clear();
        assert!(preflight(&parsed, root.path(), ReadControl::default()).is_empty());
        std::fs::remove_file(root.path().join("new/api.py")).unwrap();
        assert_eq!(
            preflight(&parsed, root.path(), ReadControl::default()),
            [issue("new/api.py", "target_missing")]
        );
    }

    #[test]
    fn creates_reject_role_ancestor_and_path_conflicts_with_shared_limits() {
        let root = tempfile::tempdir().unwrap();
        for (touches, creates, docs, reason) in [
            (
                vec!["./new\\api.py"],
                vec!["new/api.py"],
                vec![],
                "creation_touch_conflict",
            ),
            (
                vec![],
                vec!["new", "new/api.py"],
                vec![],
                "declaration_ancestor_conflict",
            ),
            (
                vec![],
                vec!["new"],
                vec!["new/doc.md"],
                "declaration_ancestor_conflict",
            ),
            (
                vec!["new"],
                vec!["new/api.py"],
                vec![],
                "declaration_ancestor_conflict",
            ),
        ] {
            let issues = preflight(
                &creation_spec(&touches, &creates, &docs),
                root.path(),
                ReadControl::default(),
            );
            assert!(
                issues.iter().any(|issue| issue.reason == reason),
                "{issues:?}"
            );
        }
        for path in [
            "", ".", "../file", "/file", "C:file", "C:/file", "a//b", "a/../b", "a\nfile",
        ] {
            let issues = preflight(
                &creation_spec(&[], &[path], &[]),
                root.path(),
                ReadControl::default(),
            );
            assert_eq!(issues, [issue(path, "target_path_invalid")], "{path:?}");
        }
        let capped = preflight(
            &creation_spec(&[], &["new"; MAX_DECLARATIONS + 1], &[]),
            root.path(),
            ReadControl::default(),
        );
        assert_eq!(capped, [issue("", "declaration_limit_exceeded")]);
        let valid = creation_spec(&[], &["new/file"], &[]);
        assert_eq!(
            preflight(
                &valid,
                root.path(),
                ReadControl {
                    deadline: Some(Instant::now()),
                    interrupted: None
                }
            ),
            [issue("new/file", "deadline_exceeded")]
        );
    }

    #[test]
    fn creates_rechecks_missing_ancestors_identity_and_interruption() {
        for mutation in [
            "parent_appeared",
            "parent_replaced",
            "file_appeared",
            "interrupted",
            "work_limit",
        ] {
            let root = tempfile::tempdir().unwrap();
            if mutation != "parent_appeared" {
                std::fs::create_dir(root.path().join("new")).unwrap();
            }
            let cancelled = Cell::new(false);
            let callback = || cancelled.get();
            let mut inspector = checker(
                root.path(),
                ReadControl {
                    deadline: None,
                    interrupted: Some(&callback),
                },
            );
            let mut declared = targets("new/file", true);
            let target = declared.get_mut("new/file").unwrap();
            target.touched = false;
            target.created = true;
            let (receipts, issues) = inspector.collect(declared, true, |_| false);
            assert!(issues.is_empty(), "{issues:?}");
            assert!(receipts[0].absence.is_some());
            match mutation {
                "parent_appeared" => std::fs::create_dir(root.path().join("new")).unwrap(),
                "parent_replaced" => {
                    std::fs::rename(root.path().join("new"), root.path().join("old")).unwrap();
                    std::fs::create_dir(root.path().join("new")).unwrap();
                }
                "file_appeared" => std::fs::write(root.path().join("new/file"), "new").unwrap(),
                "interrupted" => cancelled.set(true),
                "work_limit" => inspector.work = 0,
                _ => unreachable!(),
            }
            let expected = match mutation {
                "interrupted" => "interrupted",
                "work_limit" => "work_budget_exhausted",
                _ => "target_changed",
            };
            assert_eq!(
                inspector.finish(receipts),
                [issue("new/file", expected)],
                "{mutation}"
            );
        }
    }

    #[test]
    fn creates_baseline_queries_preserve_literal_paths_types_and_errors() {
        let root = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .current_dir(root.path())
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q", "--initial-branch=main"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(root.path().join("a.txt"), [0xff, 0, 0xfe]).unwrap();
        std::fs::create_dir(root.path().join("directory")).unwrap();
        std::fs::write(root.path().join("directory/file"), "existing").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "baseline"]);
        let module_commit = git(&["rev-parse", "HEAD"]);
        git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            "160000",
            &module_commit,
            "module",
        ]);
        git(&["commit", "-qm", "baseline gitlink"]);
        let baseline = git(&["rev-parse", "HEAD"]);
        let mut inspector = checker(root.path(), ReadControl::default());
        let paths = ["a.txt", "directory", "[ab].txt", "new/file", "module"];
        assert_eq!(
            inspector.baseline_entries(&paths, &baseline).unwrap(),
            BTreeSet::from(["a.txt".into(), "directory".into(), "module".into()])
        );
        let many = (0..130).map(|i| format!("new-{i}")).collect::<Vec<_>>();
        let paths = many.iter().map(String::as_str).collect::<Vec<_>>();
        assert!(inspector
            .baseline_entries(&paths, &baseline)
            .unwrap()
            .is_empty());
        assert_eq!(
            inspector.baseline_entries(&paths, "--bad"),
            Err("creation_baseline_unavailable")
        );
        assert_eq!(
            inspector.baseline_entries(&paths, &"0".repeat(40)),
            Err("creation_baseline_unavailable")
        );
        inspector.work = 0;
        assert_eq!(
            inspector.baseline_entries(&paths, &baseline),
            Err("work_budget_exhausted")
        );
    }

    #[cfg(unix)]
    #[test]
    fn creates_preflight_rejects_links_and_missing_link_descendants_unix() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("real")).unwrap();
        symlink("real", root.path().join("linked")).unwrap();
        symlink("absent", root.path().join("dangling")).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(root.path().join("pipe"))
            .status()
            .unwrap()
            .success());
        for file in [
            "linked",
            "linked/missing",
            "dangling",
            "dangling/missing",
            "pipe",
            "pipe/child",
        ] {
            assert!(
                !preflight(
                    &creation_spec(&[], &[file], &[]),
                    root.path(),
                    ReadControl::default()
                )
                .is_empty(),
                "{file}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn creates_preflight_rejects_junction_missing_descendants_windows() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let destination = outside.path().join("target");
        std::fs::create_dir(&destination).unwrap();
        let junction = root.path().join("junction");
        let output = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&destination)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let parsed = creation_spec(&[], &["junction/missing"], &[]);
        assert!(!preflight(&parsed, root.path(), ReadControl::default()).is_empty());
        std::fs::remove_dir(&destination).unwrap();
        assert!(!preflight(&parsed, root.path(), ReadControl::default()).is_empty());
        std::fs::remove_dir(&junction).unwrap();
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
            let (receipts, issues) = inspector.collect(targets("file", false), false, |_| true);
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
        let (receipts, issues) = inspector.collect(targets("file", false), false, |_| false);
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
        let (receipts, issues) = inspector.collect(targets("file", false), false, |_| false);
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
        let (receipts, issues) = inspector.collect(targets("private", false), false, |_| false);
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
