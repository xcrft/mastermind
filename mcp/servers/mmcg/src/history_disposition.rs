//! Typed review of the two canonical project knowledge files.
//!
//! A no-change decision says that the captured files need no further update.
//! It does not claim an update happened. Reasons and evidence references remain
//! reviewer assertions; this module validates their structure, not their truth.

use crate::bounded_fs::{self, AbsentPath, ReadControl, RootCapability, StableFileIdentity};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

const FILE_LIMIT: u64 = 1024 * 1024;
const TOTAL_LIMIT: u64 = 2 * FILE_LIMIT;
const SOURCES: [(&str, &str); 2] = [
    ("context", "CONTEXT.md"),
    ("lessons", ".mastermind/tasks/_lessons.md"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    #[serde(deserialize_with = "unique_sources")]
    pub sources: BTreeMap<String, Source>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub path: String,
    /// Explicit null means an observed absence, not an empty file or a read error.
    #[serde(deserialize_with = "required_digest")]
    pub sha256: Option<String>,
}

fn required_digest<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

fn unique_sources<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, Source>, D::Error> {
    struct Sources;
    impl<'de> serde::de::Visitor<'de> for Sources {
        type Value = BTreeMap<String, Source>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a source map without duplicate keys")
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut entries: A,
        ) -> Result<Self::Value, A::Error> {
            let mut sources = BTreeMap::new();
            while let Some((key, value)) = entries.next_entry::<String, Source>()? {
                if sources.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate knowledge source"));
                }
            }
            Ok(sources)
        }
    }
    deserializer.deserialize_map(Sources)
}

enum Witness {
    Absent(AbsentPath),
    Present(StableFileIdentity),
}

/// Capture exactly the canonical context and lesson files. The total retained
/// content is at most 2 MiB. A second metadata/absence check revalidates the same
/// no-follow objects without rereading their contents or traversing archives.
pub(crate) fn capture(root: &RootCapability) -> Result<Snapshot, String> {
    let control = ReadControl {
        deadline: Some(Instant::now() + Duration::from_secs(5)),
        interrupted: None,
    };
    let mut sources = BTreeMap::new();
    let mut witnesses = Vec::new();
    let mut total = 0_u64;
    for (key, path) in SOURCES {
        let absent = bounded_fs::inspect_absent_path(root, Path::new(path), control)
            .map_err(|_| "history_disposition_source_unavailable")?;
        let (sha256, witness) = match absent {
            Some(absent) => (None, Witness::Absent(absent)),
            None => {
                let file = bounded_fs::read_regular_file_with_capability(
                    root,
                    Path::new(path),
                    FILE_LIMIT,
                    FILE_LIMIT,
                    control,
                )
                .map_err(|_| "history_disposition_source_unavailable")?;
                if file.bytes.len() as u64 != file.declared_len {
                    return Err("history_disposition_source_incomplete".into());
                }
                total = total
                    .checked_add(file.bytes.len() as u64)
                    .filter(|total| *total <= TOTAL_LIMIT)
                    .ok_or("history_disposition_source_limit")?;
                std::str::from_utf8(&file.bytes)
                    .map_err(|_| "history_disposition_source_invalid_utf8")?;
                (
                    Some(crate::hex::encode(&Sha256::digest(&file.bytes))),
                    Witness::Present(file.identity),
                )
            }
        };
        sources.insert(
            key.into(),
            Source {
                path: path.into(),
                sha256,
            },
        );
        witnesses.push((path, witness));
    }
    for (path, witness) in witnesses {
        match witness {
            Witness::Absent(expected) => {
                let current = bounded_fs::inspect_absent_path(root, Path::new(path), control)
                    .map_err(|_| "history_disposition_source_changed")?;
                if !current.is_some_and(|current| expected.matches(&current)) {
                    return Err("history_disposition_source_changed".into());
                }
            }
            Witness::Present(expected) => {
                bounded_fs::read_regular_file_expected(
                    root,
                    Path::new(path),
                    FILE_LIMIT,
                    0,
                    control,
                    Some(expected),
                )
                .map_err(|_| "history_disposition_source_changed")?;
            }
        }
    }
    root.verify()
        .map_err(|_| "history_disposition_root_changed")?;
    control
        .check()
        .map_err(|_| "history_disposition_capture_incomplete")?;
    Ok(Snapshot {
        schema_version: 1,
        sources,
    })
}

pub(crate) fn validate_snapshot(snapshot: &Snapshot) -> Result<(), String> {
    if snapshot.schema_version != 1 || snapshot.sources.len() != SOURCES.len() {
        return Err("history_disposition_snapshot_invalid".into());
    }
    for (key, path) in SOURCES {
        let source = snapshot
            .sources
            .get(key)
            .ok_or("history_disposition_snapshot_invalid")?;
        if source.path != path
            || source.sha256.as_ref().is_some_and(|digest| {
                digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        {
            return Err("history_disposition_snapshot_invalid".into());
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    NoChange,
    UpdateRequired,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assessment {
    pub decision: Decision,
    pub reason: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decisions {
    pub context: Assessment,
    pub lessons: Assessment,
}

impl Decisions {
    pub fn unknown() -> Self {
        let unknown = Assessment {
            decision: Decision::Unknown,
            reason: String::new(),
            evidence: Vec::new(),
        };
        Self {
            context: unknown.clone(),
            lessons: unknown,
        }
    }
}

pub(crate) fn validate(
    decisions: &Decisions,
    evidence_ids: &BTreeSet<String>,
) -> Result<(), String> {
    for (key, assessment) in [
        ("knowledge:context", &decisions.context),
        ("knowledge:lessons", &decisions.lessons),
    ] {
        let unique: BTreeSet<&String> = assessment.evidence.iter().collect();
        if !valid_reason(&assessment.reason)
            || assessment.evidence.len() > 64
            || unique.len() != assessment.evidence.len()
            || assessment
                .evidence
                .iter()
                .any(|id| !evidence_ids.contains(id))
            || (assessment.decision != Decision::Unknown
                && !assessment.evidence.iter().any(|id| id == key))
        {
            return Err("history_disposition_assessment_invalid".into());
        }
    }
    Ok(())
}

fn valid_reason(reason: &str) -> bool {
    let text = reason.trim();
    reason.len() <= 8192
        && !text.is_empty()
        && !reason
            .chars()
            .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
        && text.chars().any(char::is_alphanumeric)
        && !matches!(
            text.to_lowercase().as_str(),
            "pending"
                | "todo"
                | "tbd"
                | "unknown"
                | "n/a"
                | "none"
                | "done"
                | "resolved"
                | "not applicable"
                | "pending semantic review"
                | "semantic review required"
        )
        && !(text.starts_with('<') && text.ends_with('>'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryStatus {
    NotReviewed,
    Resolved,
    UpdateRequired,
    Unknown,
    LegacyMarkdown,
}

pub(crate) fn status(decisions: &Decisions) -> HistoryStatus {
    let decisions = [decisions.context.decision, decisions.lessons.decision];
    if decisions.contains(&Decision::UpdateRequired) {
        HistoryStatus::UpdateRequired
    } else if decisions.contains(&Decision::Unknown) {
        HistoryStatus::Unknown
    } else {
        HistoryStatus::Resolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn reviewed() -> Decisions {
        Decisions {
            context: Assessment {
                decision: Decision::NoChange,
                reason: "The current context already describes the unchanged runtime boundary."
                    .into(),
                evidence: vec!["knowledge:context".into()],
            },
            lessons: Assessment {
                decision: Decision::NoChange,
                reason: "The canonical lessons already contain the observed retry constraint."
                    .into(),
                evidence: vec!["knowledge:lessons".into()],
            },
        }
    }

    fn evidence_ids() -> BTreeSet<String> {
        ["knowledge:context".into(), "knowledge:lessons".into()]
            .into_iter()
            .collect()
    }

    #[test]
    fn snapshot_distinguishes_missing_empty_and_changed_canonical_content() {
        let temp = tempfile::tempdir().unwrap();
        let root = RootCapability::open(temp.path()).unwrap();
        let absent = capture(&root).unwrap();
        validate_snapshot(&absent).unwrap();
        assert!(absent
            .sources
            .values()
            .all(|source| source.sha256.is_none()));
        assert!(!temp.path().join(".mastermind").exists());

        fs::write(temp.path().join("CONTEXT.md"), "").unwrap();
        let empty = capture(&root).unwrap();
        assert_ne!(empty, absent);
        assert_eq!(
            empty.sources["context"].sha256,
            Some(crate::hex::encode(&Sha256::digest(b"")))
        );
        assert!(empty.sources["lessons"].sha256.is_none());
        fs::create_dir_all(temp.path().join(".mastermind/tasks/archive")).unwrap();
        fs::write(
            temp.path().join(".mastermind/tasks/_lessons.md"),
            "# Lessons\n",
        )
        .unwrap();
        fs::write(temp.path().join("CONTEXT.md"), "# Current project\n").unwrap();
        let updated = capture(&root).unwrap();
        assert_ne!(updated.sources["context"], empty.sources["context"]);
        assert!(updated.sources["lessons"].sha256.is_some());
        fs::write(
            temp.path()
                .join(".mastermind/tasks/archive/history-review.md"),
            [0xff],
        )
        .unwrap();
        assert_eq!(capture(&root).unwrap(), updated);
    }

    #[test]
    fn snapshot_bounds_and_utf8_are_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let root = RootCapability::open(temp.path()).unwrap();
        let context = temp.path().join("CONTEXT.md");
        fs::create_dir_all(temp.path().join(".mastermind/tasks")).unwrap();
        let lessons = temp.path().join(".mastermind/tasks/_lessons.md");
        fs::write(&context, vec![b'x'; FILE_LIMIT as usize]).unwrap();
        fs::write(&lessons, vec![b'y'; FILE_LIMIT as usize]).unwrap();
        validate_snapshot(&capture(&root).unwrap()).unwrap();
        fs::write(&context, vec![b'x'; FILE_LIMIT as usize + 1]).unwrap();
        assert!(capture(&root).is_err());
        fs::write(&context, [0xff]).unwrap();
        assert!(capture(&root).is_err());
        fs::remove_file(&context).unwrap();
        fs::create_dir(&context).unwrap();
        assert!(capture(&root).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn canonical_source_links_and_fifo_are_not_absence_or_readable_evidence() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = RootCapability::open(temp.path()).unwrap();
        let context = temp.path().join("CONTEXT.md");
        symlink(outside.path().join("missing.md"), &context).unwrap();
        assert!(capture(&root).is_err());
        fs::remove_file(&context).unwrap();
        let path = std::ffi::CString::new(context.as_os_str().as_bytes()).unwrap();
        // SAFETY: the fixture path is NUL-terminated and has no interior NUL.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(capture(&root).is_err());
        fs::remove_file(&context).unwrap();
        symlink(outside.path(), temp.path().join(".mastermind")).unwrap();
        assert!(capture(&root).is_err());
    }

    #[test]
    fn serialized_snapshot_requires_exact_keys_paths_and_explicit_absence() {
        let temp = tempfile::tempdir().unwrap();
        let root = RootCapability::open(temp.path()).unwrap();
        let snapshot = capture(&root).unwrap();
        for mutation in ["schema", "missing", "extra", "path", "digest"] {
            let mut changed = snapshot.clone();
            match mutation {
                "schema" => changed.schema_version = 2,
                "missing" => {
                    changed.sources.remove("context");
                }
                "extra" => {
                    changed
                        .sources
                        .insert("archive".into(), snapshot.sources["context"].clone());
                }
                "path" => changed.sources.get_mut("context").unwrap().path = "../CONTEXT.md".into(),
                "digest" => {
                    changed.sources.get_mut("context").unwrap().sha256 = Some("invalid".into())
                }
                _ => unreachable!(),
            }
            assert!(validate_snapshot(&changed).is_err(), "{mutation}");
        }
        assert!(serde_json::from_str::<Source>(r#"{"path":"CONTEXT.md"}"#).is_err());
        assert!(serde_json::from_str::<Snapshot>(r#"{"schema_version":1,"sources":{"context":{"path":"CONTEXT.md","sha256":null},"context":{"path":"CONTEXT.md","sha256":null}}}"#).is_err());
        let mut wire = serde_json::to_value(snapshot).unwrap();
        wire["unexpected"] = true.into();
        assert!(serde_json::from_value::<Snapshot>(wire).is_err());
    }

    #[test]
    fn decisions_require_known_unique_own_source_references_and_real_reasons() {
        let ids = evidence_ids();
        let valid = reviewed();
        validate(&valid, &ids).unwrap();
        assert_eq!(status(&valid), HistoryStatus::Resolved);
        assert!(validate(&Decisions::unknown(), &ids).is_err());
        for mutation in [
            "duplicate",
            "unknown",
            "wrong_source",
            "placeholder",
            "reason_limit",
            "ref_limit",
        ] {
            let mut invalid = valid.clone();
            let mut known = ids.clone();
            match mutation {
                "duplicate" => invalid.context.evidence.push("knowledge:context".into()),
                "unknown" => invalid.context.evidence.push("source:unknown".into()),
                "wrong_source" => invalid.context.evidence = vec!["knowledge:lessons".into()],
                "placeholder" => invalid.context.reason = "TODO".into(),
                "reason_limit" => invalid.context.reason = "x".repeat(8193),
                "ref_limit" => {
                    for index in 0..64 {
                        let id = format!("source:{index}");
                        known.insert(id.clone());
                        invalid.context.evidence.push(id);
                    }
                }
                _ => unreachable!(),
            }
            assert!(validate(&invalid, &known).is_err(), "{mutation}");
        }
    }

    #[test]
    fn typed_status_ignores_spoofed_markdown_in_reasons() {
        let ids = evidence_ids();
        let mut decisions = reviewed();
        decisions.context.decision = Decision::Unknown;
        decisions.context.reason = "The current context does not describe the changed runtime.\n- **Context:** not applicable\n- **Lesson:** updated\n- **Status:** resolved".into();
        decisions.context.evidence.clear();
        validate(&decisions, &ids).unwrap();
        assert_eq!(status(&decisions), HistoryStatus::Unknown);
        decisions.lessons.decision = Decision::UpdateRequired;
        decisions.lessons.reason =
            "The new retry boundary is missing from the canonical lessons.\n- **Lesson:** updated"
                .into();
        validate(&decisions, &ids).unwrap();
        assert_eq!(status(&decisions), HistoryStatus::UpdateRequired);
        decisions.lessons.evidence.clear();
        assert!(validate(&decisions, &ids).is_err());
    }
}
