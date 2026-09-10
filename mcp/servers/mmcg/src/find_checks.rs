//! Bounded preflight checks for literal FIND preconditions.

use crate::bounded_fs::{self, BoundedReadError, ReadControl, RootCapability, StableFileIdentity};
use crate::spec::FindBlock;
use crate::verify_spec::Finding;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const MAX_BLOCKS: usize = 1024;
const MAX_WORK: usize = 16_384;
const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = crate::indexer::MAX_INDEXABLE_FILE_SIZE;
const MAX_PATH_BYTES: usize = 4096;
const MAX_PATH_COMPONENTS: usize = 64;

struct Receipt {
    block: usize,
    relative: PathBuf,
    identity: StableFileIdentity,
    digest: [u8; 32],
}

struct Collected {
    findings: Vec<Option<Finding>>,
    receipts: Vec<Receipt>,
}

struct Checker<'a> {
    root: Result<RootCapability, &'static str>,
    control: ReadControl<'a>,
    work: usize,
    bytes: u64,
}

pub(crate) fn check(blocks: &[FindBlock], root: &Path, control: ReadControl<'_>) -> Vec<Finding> {
    if blocks.is_empty() {
        return Vec::new();
    }
    if blocks.len() > MAX_BLOCKS {
        return vec![Finding::FindBlockUnavailable {
            file: None,
            phase: None,
            reason: "block_limit_exceeded".into(),
        }];
    }
    let mut checker = Checker::new(root, control);
    let collected = checker.collect(blocks);
    checker.finish(blocks, collected)
}

fn unavailable(block: &FindBlock, reason: &str) -> Finding {
    Finding::FindBlockUnavailable {
        file: block.file.clone(),
        phase: block.phase.clone(),
        reason: reason.into(),
    }
}

fn read_reason(error: BoundedReadError) -> &'static str {
    match error {
        BoundedReadError::InvalidPath | BoundedReadError::OutsideRoot => "target_path_invalid",
        BoundedReadError::NotRegular => "target_not_regular",
        BoundedReadError::TooLarge { .. } => "target_too_large",
        BoundedReadError::SnapshotChanged => "target_changed",
        BoundedReadError::Interrupted => "interrupted",
        BoundedReadError::DeadlineExceeded => "deadline_exceeded",
        BoundedReadError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            "target_missing"
        }
        BoundedReadError::Io(_) => "target_unreadable",
    }
}

impl<'a> Checker<'a> {
    fn new(root: &Path, control: ReadControl<'a>) -> Self {
        Self {
            root: control
                .check()
                .map_err(read_reason)
                .and_then(|()| RootCapability::open(root).map_err(|_| "root_unavailable")),
            control,
            work: MAX_WORK,
            bytes: MAX_BYTES,
        }
    }

    fn charge(&mut self, work: usize) -> Result<(), &'static str> {
        self.control.check().map_err(read_reason)?;
        self.work = self.work.checked_sub(work).ok_or("work_budget_exhausted")?;
        Ok(())
    }

    fn relative(file: &str) -> Result<PathBuf, &'static str> {
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
        let relative = PathBuf::from(relative);
        if relative.components().count() > MAX_PATH_COMPONENTS {
            return Err("path_limit_exceeded");
        }
        Ok(relative)
    }

    fn read(
        &mut self,
        relative: &Path,
        expected: Option<StableFileIdentity>,
    ) -> Result<bounded_fs::BoundedFile, &'static str> {
        self.charge(1 + relative.components().count())?;
        let allowance = self.bytes.min(MAX_FILE_BYTES);
        if allowance == 0 {
            return Err("read_budget_exhausted");
        }
        self.bytes -= allowance;
        let root = self.root.as_ref().map_err(|reason| *reason)?;
        let file = bounded_fs::read_regular_file_expected(
            root,
            &root.requested_root().join(relative),
            allowance,
            allowance,
            self.control,
            expected,
        )
        .map_err(|error| match error {
            BoundedReadError::TooLarge { .. } if allowance < MAX_FILE_BYTES => {
                "read_budget_exhausted"
            }
            error => read_reason(error),
        })?;
        if file.bytes.len() as u64 != file.declared_len {
            return Err("target_changed");
        }
        self.bytes += allowance - file.declared_len;
        Ok(file)
    }

    fn collect(&mut self, blocks: &[FindBlock]) -> Collected {
        let mut collected = Collected {
            findings: Vec::with_capacity(blocks.len()),
            receipts: Vec::new(),
        };
        for (index, block) in blocks.iter().enumerate() {
            let inspected = (|| {
                self.charge(1)?;
                let relative = Self::relative(block.file.as_deref().ok_or("target_unspecified")?)?;
                if block.find_text.is_empty() {
                    return Err("find_text_empty");
                }
                let file = self.read(&relative, None)?;
                let body = std::str::from_utf8(&file.bytes).map_err(|_| "target_not_utf8")?;
                let finding =
                    (!body.contains(&block.find_text)).then(|| Finding::FindBlockMismatch {
                        file: block.file.clone().expect("checked target"),
                        phase: block.phase.clone(),
                        find_text_preview: block
                            .find_text
                            .lines()
                            .next()
                            .unwrap_or("")
                            .chars()
                            .take(80)
                            .collect(),
                    });
                collected.receipts.push(Receipt {
                    block: index,
                    relative,
                    identity: file.identity,
                    digest: Sha256::digest(&file.bytes).into(),
                });
                Ok(finding)
            })();
            collected.findings.push(match inspected {
                Ok(finding) => finding,
                Err(reason) => Some(unavailable(block, reason)),
            });
        }
        collected
    }

    fn finish(&mut self, blocks: &[FindBlock], mut collected: Collected) -> Vec<Finding> {
        for receipt in &collected.receipts {
            let checked = self
                .read(&receipt.relative, Some(receipt.identity))
                .and_then(|file| {
                    let digest: [u8; 32] = Sha256::digest(&file.bytes).into();
                    (digest == receipt.digest)
                        .then_some(())
                        .ok_or("target_changed")
                });
            if let Err(reason) = checked {
                collected.findings[receipt.block] =
                    Some(unavailable(&blocks[receipt.block], reason));
            }
        }
        let final_check = self.control.check().map_err(read_reason).and_then(|()| {
            self.root
                .as_ref()
                .map_err(|reason| *reason)?
                .verify()
                .map_err(|_| "root_changed")?;
            self.control.check().map_err(read_reason)
        });
        if let Err(reason) = final_check {
            for receipt in &collected.receipts {
                collected.findings[receipt.block] =
                    Some(unavailable(&blocks[receipt.block], reason));
            }
        }
        collected.findings.into_iter().flatten().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::time::Instant;

    fn block(file: Option<&str>, find: &str) -> FindBlock {
        FindBlock {
            file: file.map(str::to_string),
            find_text: find.into(),
            phase: Some("Phase 1: replace".into()),
        }
    }

    fn reasons(findings: &[Finding]) -> Vec<&str> {
        findings
            .iter()
            .map(|finding| match finding {
                Finding::FindBlockUnavailable { reason, .. } => reason.as_str(),
                other => panic!("expected unavailable, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn find_checks_require_complete_utf8_and_explicit_targets() {
        let root = tempfile::tempdir().unwrap();
        let blocks = [block(Some("input.txt"), "needle")];
        std::fs::write(root.path().join("input.txt"), b"prefix needle suffix").unwrap();
        assert!(check(&blocks, root.path(), ReadControl::default()).is_empty());
        std::fs::write(root.path().join("input.txt"), b"different").unwrap();
        assert!(matches!(
            check(&blocks, root.path(), ReadControl::default()).as_slice(),
            [Finding::FindBlockMismatch { find_text_preview, .. }] if find_text_preview == "needle"
        ));
        std::fs::write(root.path().join("input.txt"), b"needle\xff").unwrap();
        assert_eq!(
            reasons(&check(&blocks, root.path(), ReadControl::default())),
            ["target_not_utf8"]
        );
        assert_eq!(
            reasons(&check(
                &[block(None, "needle")],
                root.path(),
                ReadControl::default()
            )),
            ["target_unspecified"]
        );
        assert_eq!(
            reasons(&check(
                &[block(Some("input.txt"), "")],
                root.path(),
                ReadControl::default()
            )),
            ["find_text_empty"]
        );
        std::fs::remove_file(root.path().join("input.txt")).unwrap();
        assert_eq!(
            reasons(&check(&blocks, root.path(), ReadControl::default())),
            ["target_missing"]
        );
        std::fs::create_dir(root.path().join("input.txt")).unwrap();
        assert!(matches!(
            check(&blocks, root.path(), ReadControl::default()).as_slice(),
            [Finding::FindBlockUnavailable { .. }]
        ));
    }

    #[test]
    fn find_checks_bound_paths_blocks_and_repeated_work() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("input.txt"), b"needle").unwrap();
        assert_eq!(
            Checker::relative("./src\\input.txt").unwrap(),
            PathBuf::from("src/input.txt")
        );
        for path in [
            "",
            ".",
            "../input.txt",
            "src/../../input.txt",
            "/input.txt",
            "C:/input.txt",
            "C:input.txt",
            "\\\\host\\file",
            "input\0.txt",
            "input\n.txt",
        ] {
            assert_eq!(
                Checker::relative(path),
                Err("target_path_invalid"),
                "{path:?}"
            );
        }
        assert_eq!(
            Checker::relative(&"a/".repeat(MAX_PATH_COMPONENTS)),
            Err("target_path_invalid")
        );
        let deep = format!("{}input.txt", "a/".repeat(MAX_PATH_COMPONENTS));
        assert_eq!(Checker::relative(&deep), Err("path_limit_exceeded"));
        assert_eq!(
            Checker::relative(&"a".repeat(MAX_PATH_BYTES + 1)),
            Err("path_limit_exceeded")
        );
        let blocks = [block(Some("input.txt"), "needle")];
        assert_eq!(
            reasons(&check(
                &vec![blocks[0].clone(); MAX_BLOCKS + 1],
                root.path(),
                ReadControl::default()
            )),
            ["block_limit_exceeded"]
        );
        let mut checker = Checker::new(root.path(), ReadControl::default());
        checker.work = 5;
        let collected = checker.collect(&blocks);
        assert!(checker.finish(&blocks, collected).is_empty());
        assert_eq!(checker.work, 0);
        let collected = checker.collect(&blocks);
        assert_eq!(
            reasons(&checker.finish(&blocks, collected)),
            ["work_budget_exhausted"]
        );
    }

    #[test]
    fn find_checks_byte_budget_includes_rechecks_and_failed_reads() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("input.txt");
        std::fs::write(&path, b"needle").unwrap();
        let blocks = [block(Some("input.txt"), "needle")];
        for (bytes, passed) in [(11, false), (12, true)] {
            let mut checker = Checker::new(root.path(), ReadControl::default());
            checker.bytes = bytes;
            let collected = checker.collect(&blocks);
            let findings = checker.finish(&blocks, collected);
            assert_eq!(findings.is_empty(), passed, "{findings:?}");
            if !passed {
                assert_eq!(reasons(&findings), ["read_budget_exhausted"]);
            }
            assert_eq!(checker.bytes, 0);
        }
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        let mut checker = Checker::new(root.path(), ReadControl::default());
        let before = checker.bytes;
        let collected = checker.collect(&blocks);
        assert_eq!(checker.bytes, before - MAX_FILE_BYTES);
        assert_eq!(
            reasons(&checker.finish(&blocks, collected)),
            ["target_too_large"]
        );
        checker.bytes = 10;
        let repeated = [blocks[0].clone(), blocks[0].clone()];
        let collected = checker.collect(&repeated);
        assert_eq!(
            reasons(&checker.finish(&repeated, collected)),
            ["read_budget_exhausted", "read_budget_exhausted"]
        );
        assert_eq!(checker.bytes, 0);
    }

    #[test]
    fn find_checks_preserve_deadline_and_cancellation_errors() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("input.txt"), b"needle").unwrap();
        let blocks = [block(Some("input.txt"), "needle")];
        assert_eq!(
            reasons(&check(
                &blocks,
                root.path(),
                ReadControl {
                    deadline: Some(Instant::now()),
                    interrupted: None
                }
            )),
            ["deadline_exceeded"]
        );
        let stopped = Cell::new(false);
        let interrupted = || stopped.get();
        let mut checker = Checker::new(
            root.path(),
            ReadControl {
                deadline: None,
                interrupted: Some(&interrupted),
            },
        );
        let collected = checker.collect(&blocks);
        stopped.set(true);
        assert_eq!(
            reasons(&checker.finish(&blocks, collected)),
            ["interrupted"]
        );
    }

    #[test]
    fn find_checks_revalidate_file_identity_and_content() {
        for mutation in [
            "rewrite",
            "same_metadata",
            "remove",
            "replace",
            "directory",
            "stale_mismatch",
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("input.txt");
            std::fs::write(&path, b"needle").unwrap();
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            let find = if mutation == "stale_mismatch" {
                "different"
            } else {
                "needle"
            };
            let blocks = [block(Some("input.txt"), find)];
            let mut checker = Checker::new(root.path(), ReadControl::default());
            let collected = checker.collect(&blocks);
            if mutation == "stale_mismatch" {
                assert!(matches!(
                    collected.findings.as_slice(),
                    [Some(Finding::FindBlockMismatch { .. })]
                ));
            } else {
                assert!(collected.findings.iter().all(Option::is_none));
            }
            match mutation {
                "rewrite" | "stale_mismatch" => std::fs::write(&path, b"new contents").unwrap(),
                "same_metadata" => {
                    std::fs::write(&path, b"edited").unwrap();
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(&path)
                        .unwrap()
                        .set_times(std::fs::FileTimes::new().set_modified(modified))
                        .unwrap();
                }
                "remove" => std::fs::remove_file(&path).unwrap(),
                "replace" => {
                    std::fs::rename(&path, root.path().join("old.txt")).unwrap();
                    std::fs::write(&path, b"needle").unwrap();
                }
                "directory" => {
                    std::fs::remove_file(&path).unwrap();
                    std::fs::create_dir(&path).unwrap();
                }
                _ => unreachable!(),
            }
            let findings = checker.finish(&blocks, collected);
            assert_eq!(findings.len(), 1, "{mutation}: {findings:?}");
            assert!(
                matches!(findings[0], Finding::FindBlockUnavailable { .. }),
                "{mutation}: {findings:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn find_checks_reject_links_special_files_and_root_replacement_unix() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("root");
        let outside = directory.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("input.txt"), b"needle").unwrap();
        symlink(outside.join("input.txt"), root.join("link.txt")).unwrap();
        symlink(&outside, root.join("linked")).unwrap();
        let status = std::process::Command::new("mkfifo")
            .arg(root.join("pipe"))
            .status()
            .unwrap();
        assert!(status.success());
        for path in ["link.txt", "linked/input.txt", "pipe"] {
            let findings = check(
                &[block(Some(path), "needle")],
                &root,
                ReadControl::default(),
            );
            assert_eq!(findings.len(), 1);
            assert!(
                matches!(findings[0], Finding::FindBlockUnavailable { .. }),
                "{findings:?}"
            );
        }
        std::fs::write(root.join("input.txt"), b"needle").unwrap();
        let blocks = [block(Some("input.txt"), "needle")];
        let mut checker = Checker::new(&root, ReadControl::default());
        let collected = checker.collect(&blocks);
        std::fs::rename(&root, directory.path().join("moved")).unwrap();
        symlink(&outside, &root).unwrap();
        assert_eq!(
            reasons(&checker.finish(&blocks, collected)),
            ["root_changed"]
        );
    }
}
