//! `mastermind context doctor` — semantic quality audit of project memory.
//!
//! The doctor validates the lean CONTEXT contract, explicit post-flight history
//! review, and structured lesson lifecycle. It deliberately does not require a
//! decision or lesson per task: durable knowledge is selective, not ceremonial.

use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const MAX_LESSONS_SIZE: u64 = crate::indexer::MAX_HISTORY_ARTIFACT_SIZE;
const MAX_CONTEXT_SIZE: u64 = crate::indexer::MAX_HISTORY_ARTIFACT_SIZE;
const MAX_TASK_STATE_SIZE: u64 = crate::indexer::MAX_HISTORY_ARTIFACT_SIZE;
const MAX_HISTORY_REVIEW_SIZE: u64 = crate::indexer::MAX_HISTORY_ARTIFACT_SIZE;
const MAX_HISTORY_TASKS: usize = 4_096;
const REQUIRED_CONTEXT_SECTIONS: &[&str] = &["identity", "active goals", "decision log"];
const REQUIRED_DECISION_FIELDS: &[&str] = &[
    "Decision",
    "Why",
    "Status",
    "Supersedes",
    "Provenance",
    "Evidence",
    "Reusable lesson",
];
const REQUIRED_LESSON_FIELDS: &[&str] = &[
    "Status",
    "Task",
    "Kind",
    "Provenance",
    "Evidence",
    "Supersedes",
    "Reusable lesson",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub root: String,
    pub checks: Vec<Check>,
    pub summary: Summary,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub ok: u32,
    pub warn: u32,
    pub fail: u32,
}

impl Report {
    pub fn from_checks(root: &Path, checks: Vec<Check>) -> Self {
        let summary = Summary {
            ok: checks
                .iter()
                .filter(|check| check.status == Status::Ok)
                .count() as u32,
            warn: checks
                .iter()
                .filter(|check| check.status == Status::Warn)
                .count() as u32,
            fail: checks
                .iter()
                .filter(|check| check.status == Status::Fail)
                .count() as u32,
        };
        Self {
            root: root.display().to_string(),
            checks,
            summary,
        }
    }

    pub fn render_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "mastermind context doctor — checking project memory at {}\n\n",
            self.root
        ));
        let name_width = self
            .checks
            .iter()
            .map(|check| check.name.chars().count())
            .max()
            .unwrap_or(20);
        for check in &self.checks {
            let marker = match check.status {
                Status::Ok => "✅",
                Status::Warn => "⚠️ ",
                Status::Fail => "❌",
            };
            out.push_str(&format!(
                "  {marker} {name:<width$}  {message}\n",
                name = check.name,
                width = name_width,
                message = check.message,
            ));
            if let Some(hint) = &check.hint {
                out.push_str(&format!("       → {hint}\n"));
            }
        }
        out.push_str(&format!(
            "\n{} ok, {} warn, {} fail\n",
            self.summary.ok, self.summary.warn, self.summary.fail
        ));
        if self.summary.fail > 0 || self.summary.warn > 0 {
            out.push_str("\nCommon fixes:\n");
            out.push_str("  Missing file       run `mastermind init` to scaffold CONTEXT.md\n");
            out.push_str(
                "  Template residue   replace angle-bracket placeholders with project facts\n",
            );
            out.push_str(
                "  Pending review     resolve task `history-review.md` after semantic review\n",
            );
            out.push_str("  Lesson candidate   promote, resolve, or supersede it with evidence\n");
        }
        out
    }

    pub fn has_failures(&self) -> bool {
        self.summary.fail > 0
    }
}

pub fn run(root: &Path) -> Report {
    let repository = crate::bounded_fs::RootCapability::open(root)
        .map_err(|error| format!("cannot retain repository snapshot: {error}"));
    let context_path = root.join("CONTEXT.md");
    let context = match &repository {
        Ok(repository) => read_optional_text(repository, &context_path, MAX_CONTEXT_SIZE),
        Err(error) => Err(error.clone()),
    };
    let body = context
        .as_ref()
        .ok()
        .and_then(|snapshot| snapshot.as_ref())
        .map(|snapshot| snapshot.body.as_str());
    let review_tasks = match &repository {
        Ok(repository) => history_review_task_dirs(repository, &root.join(".mastermind/tasks")),
        Err(error) => Err(error.clone()),
    };
    let history_review = match (&repository, &review_tasks) {
        (Ok(repository), Ok(tasks)) => check_history_review_with_capability(repository, tasks),
        (Err(error), _) => Check {
            name: "history review",
            status: Status::Fail,
            message: error.clone(),
            hint: Some("restore a regular repository root before trusting the result".into()),
        },
        (_, Err(error)) => Check {
            name: "history review",
            status: Status::Fail,
            message: error.clone(),
            hint: Some(
                "restore a bounded regular task inventory before trusting the result".into(),
            ),
        },
    };
    let checks = vec![
        check_context_file(&context),
        check_placeholders(body),
        check_minimum_content(body),
        check_core_sections(body),
        check_decision_schema(body),
        history_review,
        match &repository {
            Ok(repository) => check_lessons_file_with_capability(
                repository,
                review_tasks.as_deref().unwrap_or_default(),
            ),
            Err(error) => Check {
                name: "lessons quality",
                status: Status::Fail,
                message: error.clone(),
                hint: Some("restore a regular repository root before trusting the result".into()),
            },
        },
    ];
    Report::from_checks(root, checks)
}

struct TextSnapshot {
    body: String,
    bytes: u64,
}

fn read_optional_text(
    root: &crate::bounded_fs::RootCapability,
    path: &Path,
    maximum_bytes: u64,
) -> Result<Option<TextSnapshot>, String> {
    match crate::bounded_fs::read_regular_file_with_capability(
        root,
        path,
        maximum_bytes,
        maximum_bytes,
        crate::bounded_fs::ReadControl::default(),
    ) {
        Ok(file) => {
            let body = String::from_utf8(file.bytes)
                .map_err(|_| format!("{} is not valid UTF-8", path.display()))?;
            Ok(Some(TextSnapshot {
                body,
                bytes: file.declared_len,
            }))
        }
        Err(crate::bounded_fs::BoundedReadError::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(format!("cannot read {} safely: {error}", path.display())),
    }
}

fn check_context_file(context: &Result<Option<TextSnapshot>, String>) -> Check {
    match context {
        Ok(Some(snapshot)) => Check {
            name: "context.md exists",
            status: Status::Ok,
            message: format!("CONTEXT.md found ({})", format_bytes(snapshot.bytes)),
            hint: None,
        },
        Ok(None) => Check {
            name: "context.md exists",
            status: Status::Fail,
            message: "CONTEXT.md not found at project root".into(),
            hint: Some("run `mastermind init` to scaffold a lean CONTEXT.md".into()),
        },
        Err(error) => Check {
            name: "context.md exists",
            status: Status::Fail,
            message: error.clone(),
            hint: Some("replace it with a regular UTF-8 file within the size limit".into()),
        },
    }
}

fn check_placeholders(body: Option<&str>) -> Check {
    let Some(text) = body else {
        return skipped("no placeholders", "file not readable");
    };
    let tokens = placeholder_tokens(text);
    if tokens.is_empty() {
        Check {
            name: "no placeholders",
            status: Status::Ok,
            message: "no unfilled angle-bracket placeholders".into(),
            hint: None,
        }
    } else {
        let preview = tokens
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let suffix = if tokens.len() > 8 { ", …" } else { "" };
        Check {
            name: "no placeholders",
            status: Status::Fail,
            message: format!("unfilled placeholder(s): {preview}{suffix}"),
            hint: Some(
                "replace each placeholder with a verified project fact or remove the empty entry"
                    .into(),
            ),
        }
    }
}

fn placeholder_tokens(text: &str) -> Vec<String> {
    let mut tokens = BTreeSet::new();
    let mut fenced = false;
    let mut html_comment = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let mut in_inline_code = false;
        let chars: Vec<char> = line.chars().collect();
        let mut index = 0;
        while index < chars.len() {
            if html_comment {
                if chars[index..].starts_with(&['-', '-', '>']) {
                    html_comment = false;
                    index += 3;
                } else {
                    index += 1;
                }
                continue;
            }
            if !in_inline_code && chars[index..].starts_with(&['<', '!', '-', '-']) {
                html_comment = true;
                index += 4;
                continue;
            }
            if chars[index] == '`' {
                in_inline_code = !in_inline_code;
                index += 1;
                continue;
            }
            if !in_inline_code && chars[index] == '<' {
                if let Some(offset) = chars[index + 1..].iter().position(|ch| *ch == '>') {
                    let end = index + 1 + offset;
                    let inner: String = chars[index + 1..end].iter().collect();
                    let trimmed = inner.trim();
                    if !trimmed.is_empty()
                        && !trimmed.starts_with('!')
                        && !trimmed.starts_with('/')
                        && !matches!(trimmed, "br" | "details" | "summary")
                    {
                        tokens.insert(format!("<{trimmed}>"));
                    }
                    index = end + 1;
                    continue;
                }
            }
            index += 1;
        }
    }
    tokens.into_iter().collect()
}

fn check_minimum_content(body: Option<&str>) -> Check {
    let Some(text) = body else {
        return skipped("minimum content", "file not readable");
    };
    let meaningful = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("<!--"))
        .flat_map(str::chars)
        .filter(|ch| !ch.is_whitespace())
        .count();
    if meaningful >= 150 {
        Check {
            name: "minimum content",
            status: Status::Ok,
            message: format!("{meaningful} non-whitespace chars"),
            hint: None,
        }
    } else if meaningful >= 50 {
        Check {
            name: "minimum content",
            status: Status::Warn,
            message: format!("only {meaningful} non-whitespace chars"),
            hint: Some(
                "record project identity and current goals; do not pad with generic advice".into(),
            ),
        }
    } else {
        Check {
            name: "minimum content",
            status: Status::Fail,
            message: format!("{meaningful} non-whitespace chars — effectively empty"),
            hint: Some("populate project identity and active goals with verified facts".into()),
        }
    }
}

fn check_core_sections(body: Option<&str>) -> Check {
    let Some(text) = body else {
        return skipped("core sections", "file not readable");
    };
    let headings: BTreeSet<String> = text
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .map(|heading| heading.trim().to_ascii_lowercase())
        .collect();
    let missing: Vec<&str> = REQUIRED_CONTEXT_SECTIONS
        .iter()
        .copied()
        .filter(|section| !headings.contains(*section))
        .collect();
    if missing.is_empty() {
        Check {
            name: "core sections",
            status: Status::Ok,
            message: "Identity, Active goals, and Decision log present".into(),
            hint: None,
        }
    } else {
        Check {
            name: "core sections",
            status: Status::Fail,
            message: format!("missing section(s): {}", missing.join(", ")),
            hint: Some("restore the canonical lean CONTEXT headings".into()),
        }
    }
}

fn check_decision_schema(body: Option<&str>) -> Check {
    let Some(text) = body else {
        return skipped("decision schema", "file not readable");
    };
    let Some(section) = markdown_section(text, "Decision log") else {
        return skipped("decision schema", "Decision log section missing");
    };
    let entries = level_three_blocks(section);
    if entries.is_empty() {
        return Check {
            name: "decision schema",
            status: Status::Ok,
            message: "no durable decisions recorded yet".into(),
            hint: None,
        };
    }
    let mut problems = Vec::new();
    for (title, block) in entries {
        let missing: Vec<&str> = REQUIRED_DECISION_FIELDS
            .iter()
            .copied()
            .filter(|field| !has_field(block, field))
            .collect();
        if !missing.is_empty() {
            problems.push(format!("{title}: {}", missing.join(", ")));
        }
    }
    if problems.is_empty() {
        Check {
            name: "decision schema",
            status: Status::Ok,
            message: "all decision entries include provenance, evidence, and lifecycle fields"
                .into(),
            hint: None,
        }
    } else {
        Check {
            name: "decision schema",
            status: Status::Warn,
            message: format!("incomplete decision entries: {}", problems.join("; ")),
            hint: Some("add the missing fields; use `decision only — not technically verified` when appropriate".into()),
        }
    }
}

fn check_history_review_with_capability(
    root: &crate::bounded_fs::RootCapability,
    tasks: &[PathBuf],
) -> Check {
    if tasks.is_empty() {
        return Check {
            name: "history review",
            status: Status::Ok,
            message: "no completed tasks require semantic review".into(),
            hint: None,
        };
    }
    let mut unresolved = Vec::new();
    for task in tasks {
        let task_name = task
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown");
        let review_path = task.join("history-review.md");
        let review = match read_optional_text(root, &review_path, MAX_HISTORY_REVIEW_SIZE) {
            Ok(Some(review)) => review,
            Ok(None) => {
                unresolved.push(format!("{task_name}: missing"));
                continue;
            }
            Err(_) => {
                unresolved.push(format!("{task_name}: unreadable history review"));
                continue;
            }
        };
        let state = match read_task_state_with_capability(root, task) {
            Ok(state) => state,
            Err(_) => {
                unresolved.push(format!("{task_name}: unreadable task state"));
                continue;
            }
        };
        let snapshot = state
            .as_ref()
            .and_then(|state| state.history_snapshot_sha256.as_deref());
        if !crate::run_task::history_review_body_complete(&review.body, snapshot) {
            unresolved.push(format!(
                "{task_name}: incomplete, ambiguous, or mismatched review"
            ));
        }
    }
    if unresolved.is_empty() {
        Check {
            name: "history review",
            status: Status::Ok,
            message: format!("{} task review(s) have explicit dispositions", tasks.len()),
            hint: None,
        }
    } else {
        Check {
            name: "history review",
            status: Status::Warn,
            message: format!("unresolved: {}", unresolved.join(", ")),
            hint: Some(
                "review durable knowledge, then mark Context and Lesson as `updated` or `not applicable`"
                    .into(),
            ),
        }
    }
}

fn check_lessons_file_with_capability(
    root: &crate::bounded_fs::RootCapability,
    tasks: &[PathBuf],
) -> Check {
    let lessons_path = root.requested_root().join(".mastermind/tasks/_lessons.md");
    let lessons = match crate::bounded_fs::read_regular_file_with_capability(
        root,
        &lessons_path,
        MAX_LESSONS_SIZE,
        MAX_LESSONS_SIZE,
        crate::bounded_fs::ReadControl::default(),
    ) {
        Ok(file) => file,
        Err(crate::bounded_fs::BoundedReadError::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            let expected = tasks.iter().any(|task| {
                read_optional_text(
                    root,
                    &task.join("history-review.md"),
                    MAX_HISTORY_REVIEW_SIZE,
                )
                .ok()
                .flatten()
                .and_then(|snapshot| field_value(&snapshot.body, "Lesson").map(str::to_owned))
                .is_some_and(|value| normalized(&value) == "updated")
            });
            return if expected {
                Check {
                    name: "lessons quality",
                    status: Status::Warn,
                    message: "history review says lesson updated, but _lessons.md is missing"
                        .into(),
                    hint: Some("record the reviewed lesson with provenance and evidence".into()),
                }
            } else {
                Check {
                    name: "lessons quality",
                    status: Status::Ok,
                    message: "no project lessons recorded".into(),
                    hint: None,
                }
            };
        }
        Err(crate::bounded_fs::BoundedReadError::TooLarge { size, .. }) => {
            return Check {
                name: "lessons quality",
                status: Status::Warn,
                message: format!(
                    "_lessons.md is {} and will be skipped by history indexing",
                    format_bytes(size)
                ),
                hint: Some("archive resolved entries before the file reaches 1 MB".into()),
            };
        }
        Err(error) => {
            return Check {
                name: "lessons quality",
                status: Status::Fail,
                message: format!("cannot read _lessons.md safely: {error}"),
                hint: Some("replace it with a regular UTF-8 repository file".into()),
            };
        }
    };
    let body = match String::from_utf8(lessons.bytes) {
        Ok(body) => body,
        Err(_) => {
            return Check {
                name: "lessons quality",
                status: Status::Fail,
                message: "_lessons.md is not valid UTF-8".into(),
                hint: Some("replace it with a regular UTF-8 repository file".into()),
            };
        }
    };
    let entries = level_two_blocks(&body);
    if entries.is_empty() {
        return Check {
            name: "lessons quality",
            status: Status::Warn,
            message: "legacy or empty lesson format — no structured entries".into(),
            hint: Some("migrate lessons to `## lesson-<id>` entries with lifecycle fields".into()),
        };
    }
    let mut candidates = 0;
    let mut malformed = Vec::new();
    for (title, block) in &entries {
        let mut problems: Vec<String> = REQUIRED_LESSON_FIELDS
            .iter()
            .copied()
            .filter(|field| {
                field_value(block, field).is_none_or(|value| normalized(value).is_empty())
            })
            .map(|field| format!("missing, empty, or repeated {field}"))
            .collect();
        let status = field_value(block, "Status").map(normalized);
        if status.as_deref() == Some("candidate") {
            candidates += 1;
        } else if matches!(
            status.as_deref(),
            Some("active" | "resolved" | "superseded")
        ) {
            for field in ["Provenance", "Evidence", "Reusable lesson"] {
                if field_value(block, field).is_some_and(pending_lesson_value) {
                    problems.push(format!("{field} still awaits review"));
                }
            }
        } else {
            problems.push("Status must be candidate, active, resolved, or superseded".into());
        }
        if !problems.is_empty() {
            malformed.push(format!("{title}: {}", problems.join(", ")));
        }
    }
    if !malformed.is_empty() {
        Check {
            name: "lessons quality",
            status: Status::Warn,
            message: format!("malformed entries: {}", malformed.join("; ")),
            hint: Some("use a supported status and complete the lifecycle and evidence fields; keep unreviewed lessons as candidate".into()),
        }
    } else if candidates > 0 {
        Check {
            name: "lessons quality",
            status: Status::Warn,
            message: format!("{candidates} candidate lesson(s) await semantic review"),
            hint: Some("replace the pending lesson and set active, resolved, or superseded".into()),
        }
    } else {
        Check {
            name: "lessons quality",
            status: Status::Ok,
            message: format!(
                "{} structured lesson(s), no pending candidates",
                entries.len()
            ),
            hint: None,
        }
    }
}

#[cfg(test)]
fn check_lessons_file(root: &Path, tasks: &[PathBuf]) -> Check {
    match crate::bounded_fs::RootCapability::open(root) {
        Ok(root) => check_lessons_file_with_capability(&root, tasks),
        Err(error) => Check {
            name: "lessons quality",
            status: Status::Fail,
            message: format!("cannot retain repository snapshot: {error}"),
            hint: None,
        },
    }
}

fn history_review_task_dirs(
    root: &crate::bounded_fs::RootCapability,
    tasks_dir: &Path,
) -> Result<Vec<PathBuf>, String> {
    let names = match crate::bounded_fs::read_directory_names_with_capability(
        root,
        tasks_dir,
        MAX_HISTORY_TASKS,
        crate::bounded_fs::ReadControl::default(),
    ) {
        Ok(names) => names,
        Err(crate::bounded_fs::BoundedReadError::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(format!("cannot enumerate task inventory safely: {error}"));
        }
    };
    let mut tasks = Vec::new();
    for name in names {
        let Some(name_text) = name.to_str() else {
            return Err("task inventory contains a non-UTF-8 entry".into());
        };
        if name_text.starts_with('_') || name_text.starts_with('.') || name_text.ends_with(".md") {
            continue;
        }
        let path = tasks_dir.join(name);
        match crate::bounded_fs::inspect_path_kind_with_capability(
            root,
            &path,
            crate::bounded_fs::ReadControl::default(),
        ) {
            Ok(crate::bounded_fs::BoundedPathKind::Directory) => {}
            Ok(_) => {
                return Err(format!(
                    "task inventory entry is not a regular no-follow directory: {}",
                    path.display()
                ));
            }
            Err(error) => {
                return Err(format!(
                    "cannot inspect task inventory entry {}: {error}",
                    path.display()
                ));
            }
        }
        let state = read_task_state_with_capability(root, &path)
            .map_err(|error| format!("cannot read task state at {}: {error}", path.display()))?;
        let Some(state) = state else {
            continue;
        };
        if matches!(state.status.as_str(), "learned" | "history_review_required") {
            tasks.push(path);
        }
    }
    tasks.sort();
    Ok(tasks)
}

fn read_task_state_with_capability(
    root: &crate::bounded_fs::RootCapability,
    task: &Path,
) -> std::io::Result<Option<crate::workflow_status::TaskState>> {
    let state_path = task.join("state.json");
    match crate::bounded_fs::read_regular_file_with_capability(
        root,
        &state_path,
        MAX_TASK_STATE_SIZE,
        MAX_TASK_STATE_SIZE,
        crate::bounded_fs::ReadControl::default(),
    ) {
        Ok(file) => {
            let body = String::from_utf8(file.bytes).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "task state is not valid UTF-8",
                )
            })?;
            serde_json::from_str(&body)
                .map(Some)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
        }
        Err(crate::bounded_fs::BoundedReadError::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(std::io::Error::other(error)),
    }
}

/// A visible Markdown line, retaining its exact original location and bytes.
pub(crate) struct ProseLine<'a> {
    pub(crate) offset: usize,
    pub(crate) index: usize,
    pub(crate) raw: &'a str,
    pub(crate) text: &'a str,
}

/// Scan before slicing into records so quoted headings cannot lose their
/// opening fence or comment. Lines containing comments are excluded in full.
pub(crate) fn prose_lines(body: &str) -> Vec<ProseLine<'_>> {
    let mut visible = Vec::new();
    let mut offset = 0;
    let mut fence = None;
    let mut comment = false;
    for (index, raw) in body.split_inclusive('\n').enumerate() {
        let start = offset;
        offset += raw.len();
        let content = raw.trim_end_matches(['\r', '\n']);
        let indent = content.bytes().take_while(|byte| *byte == b' ').count();
        let text = &content[indent..];
        let code_indent = indent > 3 || text.starts_with('\t');
        let marker = text.as_bytes().first().copied();
        let width = text
            .bytes()
            .take_while(|byte| Some(*byte) == marker)
            .count();
        if let Some((character, opening_width)) = fence {
            if !code_indent
                && marker == Some(character)
                && width >= opening_width
                && text[width..].trim_matches([' ', '\t']).is_empty()
            {
                fence = None;
            }
            continue;
        }
        if comment {
            let mut remaining = content;
            while let Some(end) = remaining.find("-->") {
                comment = false;
                remaining = &remaining[end + 3..];
                let Some(start) = remaining.find("<!--") else {
                    break;
                };
                comment = true;
                remaining = &remaining[start + 4..];
            }
            continue;
        }
        if code_indent || text.starts_with('>') {
            continue;
        }
        if let Some(character @ (b'`' | b'~')) = marker {
            if width >= 3 && (character != b'`' || !text[width..].contains('`')) {
                fence = Some((character, width));
                continue;
            }
        }
        if let Some(start) = text.find("<!--") {
            let mut remaining = &text[start + 4..];
            comment = true;
            while let Some(end) = remaining.find("-->") {
                comment = false;
                remaining = &remaining[end + 3..];
                let Some(start) = remaining.find("<!--") else {
                    break;
                };
                comment = true;
                remaining = &remaining[start + 4..];
            }
            continue;
        }
        visible.push(ProseLine {
            offset: start,
            index,
            raw,
            text,
        });
    }
    visible
}

fn markdown_section<'a>(text: &'a str, heading: &str) -> Option<&'a str> {
    let marker = format!("## {heading}");
    let lines = prose_lines(text);
    let position = lines
        .iter()
        .position(|line| line.text.trim_end() == marker)?;
    let heading = &lines[position];
    let start = heading.offset + heading.raw.len();
    let end = lines[position + 1..]
        .iter()
        .find(|line| line.text.starts_with("## "))
        .map_or(text.len(), |line| line.offset);
    Some(&text[start..end])
}

fn level_three_blocks(section: &str) -> Vec<(&str, &str)> {
    markdown_blocks(section, "### ")
}

fn level_two_blocks(section: &str) -> Vec<(&str, &str)> {
    markdown_blocks(section, "## ")
        .into_iter()
        .filter(|(title, _)| title.starts_with("lesson-"))
        .collect()
}

fn markdown_blocks<'a>(text: &'a str, prefix: &str) -> Vec<(&'a str, &'a str)> {
    let mut starts = Vec::new();
    for line in prose_lines(text) {
        if let Some(title) = line.text.trim_end().strip_prefix(prefix) {
            starts.push((line.offset, title));
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, (start, title))| {
            let end = starts
                .get(index + 1)
                .map(|(next, _)| *next)
                .unwrap_or(text.len());
            (*title, &text[*start..end])
        })
        .collect()
}

fn has_field(block: &str, field: &str) -> bool {
    field_value(block, field).is_some()
}

fn field_value<'a>(body: &'a str, field: &str) -> Option<&'a str> {
    let marker = format!("- **{field}:**");
    let mut value = None;
    for line in prose_lines(body) {
        if let Some(found) = line.text.strip_prefix(&marker).map(str::trim) {
            if value.is_some() {
                return None;
            }
            value = Some(found);
        }
    }
    value
}

fn normalized(value: &str) -> String {
    value.trim().trim_matches('`').trim().to_ascii_lowercase()
}

fn pending_lesson_value(value: &str) -> bool {
    let value = normalized(value);
    matches!(
        value.as_str(),
        "pending" | "pending semantic review" | "semantic review required" | "todo" | "tbd"
    ) || !placeholder_tokens(&value).is_empty()
}

fn skipped(name: &'static str, reason: &str) -> Check {
    Check {
        name,
        status: Status::Warn,
        message: format!("skipped — {reason}"),
        hint: None,
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> &'static str {
        "# Demo — Context\n\n## Identity\n\n**What it is:** A deterministic codegraph and workflow CLI for coding agents.\n\n**What it is not:** A hosted execution platform.\n\n**Primary users:** Open-source maintainers and coding-agent users.\n\n## Active goals\n\n- Preserve evidence-backed workflow state across sessions.\n\n## Decision log\n\n"
    }

    fn reviewed_lesson(id: &str) -> String {
        format!("## {id}\n\n- **Status:** active\n- **Task:** `001`\n- **Kind:** audit_contract_failure\n- **Provenance:** planner review\n- **Evidence:** `audit.md`\n- **Supersedes:** none\n- **Reusable lesson:** Scope the implementation before handing off.\n")
    }

    #[test]
    fn prose_lines_preserve_offsets_and_reject_indented_fence_closers() {
        let body = "intro λ\r\n```markdown\r\n    ```\r\n## hidden-indented-close\r\n\t```\r\n## hidden-tab-close\r\n``\r\n## hidden-short-close\r\n``` trailing\r\n## hidden-suffixed-close\r\n   ```` \t\r\n<!--\r\n## hidden-comment\r\n--> <!--\r\n## hidden-second-comment\r\n-->\r\n> ## hidden-quote\r\n    ## hidden-code\r\n \t## hidden-mixed-code\r\n  ## live\r\n- **Status:** candidate\r\n~~~markdown\r\n## hidden-tilde\r\n~~~\r\n```html <!--\r\n## hidden-comment-in-info\r\n```\r\nlast";
        let visible = prose_lines(body);
        assert_eq!(
            visible.iter().map(|line| line.text).collect::<Vec<_>>(),
            ["intro λ", "## live", "- **Status:** candidate", "last"]
        );
        let heading = &visible[1];
        assert_eq!(heading.raw, "  ## live\r\n");
        assert_eq!(heading.offset, body.find("  ## live").unwrap());
        assert_eq!(heading.index, 19);
        for line in visible {
            assert_eq!(&body[line.offset..line.offset + line.raw.len()], line.raw);
        }
    }

    #[test]
    fn detects_real_template_placeholders_but_ignores_code() {
        let tokens = placeholder_tokens(
            "# <PROJECT_NAME>\n<one or two sentences>\n<!-- <ignored> -->\n`<task>/state.json`\n```ts\nconst x = <T>();\n```\n",
        );
        assert_eq!(
            tokens,
            vec![
                "<PROJECT_NAME>".to_string(),
                "<one or two sentences>".to_string()
            ]
        );
    }

    #[test]
    fn lean_context_does_not_require_a_stack_section() {
        let report = Report::from_checks(
            Path::new("."),
            vec![
                check_core_sections(Some(context())),
                check_decision_schema(Some(context())),
            ],
        );
        assert_eq!(report.summary.fail, 0);
        assert_eq!(report.summary.warn, 0);
    }

    #[test]
    fn incomplete_decision_warns_on_provenance_and_lifecycle() {
        let body = format!(
            "{}### 2026-07-19 — Pick storage\n\n- **Decision:** SQLite\n- **Why:** Local operation\n",
            context()
        );
        let check = check_decision_schema(Some(&body));
        assert_eq!(check.status, Status::Warn);
        assert!(check.message.contains("Provenance"));
        assert!(check.message.contains("Reusable lesson"));
    }

    #[test]
    fn tasks_requiring_history_review_are_selected_by_exact_status() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let learned = tasks.join("001-learned");
        let awaiting = tasks.join("002-awaiting");
        let unrelated = tasks.join("003-unrelated");
        std::fs::create_dir_all(&learned).unwrap();
        std::fs::create_dir_all(&awaiting).unwrap();
        std::fs::create_dir_all(&unrelated).unwrap();
        std::fs::write(learned.join("state.json"), r#"{"status":"learned"}"#).unwrap();
        std::fs::write(
            awaiting.join("state.json"),
            r#"{"status":"history_review_required"}"#,
        )
        .unwrap();
        std::fs::write(
            unrelated.join("state.json"),
            r#"{"status":"held","blocking_reason":"not learned yet"}"#,
        )
        .unwrap();
        let root = crate::bounded_fs::RootCapability::open(dir.path()).unwrap();
        assert_eq!(
            history_review_task_dirs(&root, &tasks).unwrap(),
            vec![learned, awaiting]
        );
    }

    #[test]
    fn malformed_task_state_cannot_be_reported_as_an_empty_history_queue() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CONTEXT.md"), context()).unwrap();
        let task = dir.path().join(".mastermind/tasks/001-broken");
        std::fs::create_dir_all(&task).unwrap();
        std::fs::write(task.join("state.json"), "{broken state").unwrap();

        let report = run(dir.path());
        let check = report
            .checks
            .iter()
            .find(|check| check.name == "history review")
            .unwrap();
        assert_eq!(check.status, Status::Fail);
        assert!(check.message.contains("cannot read task state"));
    }

    #[cfg(unix)]
    #[test]
    fn doctor_rejects_special_context_and_lesson_files_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let context_case = tempfile::tempdir().unwrap();
        let context_path = context_case.path().join("CONTEXT.md");
        let context_name = CString::new(context_path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(context_name.as_ptr(), 0o600) }, 0);
        let report = run(context_case.path());
        assert!(report.checks.iter().any(|check| {
            check.name == "context.md exists"
                && check.status == Status::Fail
                && check.message.contains("not a regular")
        }));

        let lessons_case = tempfile::tempdir().unwrap();
        std::fs::write(lessons_case.path().join("CONTEXT.md"), context()).unwrap();
        let tasks = lessons_case.path().join(".mastermind/tasks");
        std::fs::create_dir_all(&tasks).unwrap();
        let lessons_path = tasks.join("_lessons.md");
        let lessons_name = CString::new(lessons_path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(lessons_name.as_ptr(), 0o600) }, 0);
        let report = run(lessons_case.path());
        assert!(report.checks.iter().any(|check| {
            check.name == "lessons quality"
                && check.status == Status::Fail
                && check.message.contains("not a regular")
        }));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_task_inventory_is_visible_in_the_diagnosis() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CONTEXT.md"), context()).unwrap();
        std::fs::create_dir_all(dir.path().join(".mastermind")).unwrap();
        symlink(outside.path(), dir.path().join(".mastermind/tasks")).unwrap();

        let report = run(dir.path());
        let check = report
            .checks
            .iter()
            .find(|check| check.name == "history review")
            .unwrap();
        assert_eq!(check.status, Status::Fail);
        assert!(check.message.contains("cannot enumerate task inventory"));
    }

    #[test]
    fn history_review_requires_explicit_dispositions_and_reason() {
        let dir = tempfile::tempdir().unwrap();
        let task = dir.path().join("001-task");
        std::fs::create_dir_all(&task).unwrap();
        std::fs::write(
            task.join("history-review.md"),
            "- **Context:** pending\n- **Lesson:** pending\n- **Reason:** semantic review required\n",
        )
        .unwrap();
        let root = crate::bounded_fs::RootCapability::open(dir.path()).unwrap();
        assert_eq!(
            check_history_review_with_capability(&root, std::slice::from_ref(&task)).status,
            Status::Warn
        );
        std::fs::write(
            task.join("history-review.md"),
            "- **Context:** not applicable\n- **Lesson:** updated\n- **Reason:** Captured a reusable boundary rule.\n",
        )
        .unwrap();
        assert_eq!(
            check_history_review_with_capability(&root, &[task]).status,
            Status::Ok
        );
    }

    #[test]
    fn candidate_lessons_are_not_reported_as_active_knowledge() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join(".mastermind/tasks");
        std::fs::create_dir_all(&tasks).unwrap();
        std::fs::write(
            tasks.join("_lessons.md"),
            "# Project lessons\n\n## lesson-abc\n\n- **Status:** candidate\n- **Task:** `001`\n- **Kind:** `audit_contract_failure`\n- **Provenance:** controller\n- **Evidence:** `audit.md`\n- **Supersedes:** none\n- **Reusable lesson:** pending semantic review\n",
        )
        .unwrap();
        let check = check_lessons_file(dir.path(), &[]);
        assert_eq!(check.status, Status::Warn);
        assert!(check.message.contains("candidate"));
    }

    #[test]
    fn doctor_warns_until_the_awaiting_task_review_is_resolved() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CONTEXT.md"), context()).unwrap();
        let task = dir.path().join(".mastermind/tasks/001-awaiting");
        std::fs::create_dir_all(&task).unwrap();
        std::fs::write(
            task.join("state.json"),
            r#"{"status":"history_review_required"}"#,
        )
        .unwrap();
        let review = task.join("history-review.md");
        std::fs::write(
            &review,
            "- **Context:** pending\n- **Lesson:** pending\n- **Reason:** semantic review required\n",
        )
        .unwrap();
        let report = run(dir.path());
        assert!(report.checks.iter().any(|check| {
            check.name == "history review"
                && check.status == Status::Warn
                && check.message.contains("001-awaiting")
        }));
        std::fs::write(
            &review,
            "- **Context:** not applicable\n- **Lesson:** not applicable\n- **Reason:** The verified typo fix adds no durable rule.\n",
        )
        .unwrap();
        let report = run(dir.path());
        assert!(report
            .checks
            .iter()
            .any(|check| check.name == "history review" && check.status == Status::Ok));
    }

    #[test]
    fn doctor_rejects_quoted_or_ambiguous_review_completion() {
        let dir = tempfile::tempdir().unwrap();
        let complete = "- **Context:** not applicable\n- **Lesson:** updated\n- **Reason:** Reviewed the boundary rule against the audit.\n";
        for body in [
            format!("```markdown\n{complete}```\n"),
            format!("<!--\n{complete}-->\n"),
            format!("{complete}- **Lesson:** pending\n"),
            format!("{complete}- **Reason:** semantic review required\n"),
        ] {
            std::fs::write(dir.path().join("history-review.md"), &body).unwrap();
            let root = crate::bounded_fs::RootCapability::open(dir.path()).unwrap();
            assert_eq!(
                check_history_review_with_capability(&root, &[dir.path().to_path_buf()]).status,
                Status::Warn,
                "{body}"
            );
        }
    }

    #[test]
    fn doctor_requires_the_task_snapshot_when_reviewing_new_audits() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CONTEXT.md"), context()).unwrap();
        let task = dir.path().join(".mastermind/tasks/001-awaiting");
        std::fs::create_dir_all(&task).unwrap();
        let complete = "- **Context:** not applicable\n- **Lesson:** not applicable\n- **Reason:** The audited typo fix introduces no durable decision.\n";
        for status in ["history_review_required", "learned"] {
            std::fs::write(
                task.join("state.json"),
                format!(r#"{{"status":"{status}","history_snapshot_sha256":"current"}}"#),
            )
            .unwrap();
            for (marker, expected) in [
                ("", Status::Warn),
                ("- **Audit snapshot:** previous\n", Status::Warn),
                ("- **Audit snapshot:** current\n", Status::Ok),
            ] {
                std::fs::write(
                    task.join("history-review.md"),
                    format!("{marker}{complete}"),
                )
                .unwrap();
                let report = run(dir.path());
                let check = report
                    .checks
                    .iter()
                    .find(|check| check.name == "history review")
                    .unwrap();
                assert_eq!(check.status, expected, "{status}: {marker}");
            }
        }
        std::fs::write(task.join("state.json"), r#"{"status":"learned"}"#).unwrap();
        std::fs::write(task.join("history-review.md"), complete).unwrap();
        assert!(run(dir.path())
            .checks
            .iter()
            .any(|check| check.name == "history review" && check.status == Status::Ok));
    }

    #[test]
    fn copied_lessons_in_notes_are_not_records_or_record_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join(".mastermind/tasks");
        std::fs::create_dir_all(&tasks).unwrap();
        let copied = reviewed_lesson("lesson-copy");
        let reviewed = reviewed_lesson("lesson-reviewed");
        let pending =
            reviewed_lesson("lesson-live").replace("Status:** active", "Status:** candidate");
        for example in [
            format!("```markdown\n{copied}```\n"),
            format!("~~~markdown\n{copied}~~~\n"),
            format!("<!--\n{copied}-->\n"),
            copied.lines().map(|line| format!("> {line}\n")).collect(),
            copied.lines().map(|line| format!("    {line}\n")).collect(),
        ] {
            let only_example = format!("# Project lessons\n\n{example}");
            std::fs::write(tasks.join("_lessons.md"), &only_example).unwrap();
            let check = check_lessons_file(dir.path(), &[]);
            assert_eq!(check.status, Status::Warn, "{example}");
            assert!(check.message.contains("no structured entries"), "{example}");

            let notes =
                format!("# Project lessons\n\n{reviewed}\n### Review notes\n{example}\n{pending}");
            std::fs::write(tasks.join("_lessons.md"), &notes).unwrap();
            let blocks = level_two_blocks(&notes);
            assert_eq!(
                blocks.iter().map(|(title, _)| *title).collect::<Vec<_>>(),
                ["lesson-reviewed", "lesson-live"],
                "{example}"
            );
            let check = check_lessons_file(dir.path(), &[]);
            assert_eq!(check.status, Status::Warn, "{example}");
            assert_eq!(check.message, "1 candidate lesson(s) await semantic review");
        }
    }

    #[test]
    fn reviewed_lessons_require_a_valid_status_and_completed_fields() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join(".mastermind/tasks");
        std::fs::create_dir_all(&tasks).unwrap();
        let valid = "# Project lessons\n\n## lesson-abc\n\n- **Status:** active\n- **Task:** `001`\n- **Kind:** audit_contract_failure\n- **Provenance:** planner review\n- **Evidence:** `audit.md`\n- **Supersedes:** none\n- **Reusable lesson:** Scope the implementation before handing off.\n";
        for status in ["active", "resolved", "superseded"] {
            std::fs::write(
                tasks.join("_lessons.md"),
                valid.replace("Status:** active", &format!("Status:** {status}")),
            )
            .unwrap();
            assert_eq!(check_lessons_file(dir.path(), &[]).status, Status::Ok);
        }
        for (from, to) in [
            ("Status:** active", "Status:**"),
            ("Status:** active", "Status:** unexpected"),
            ("Evidence:** `audit.md`", "Evidence:** ``"),
            ("Evidence:** `audit.md`", "Evidence:** TBD"),
            ("Provenance:** planner review", "Provenance:**"),
            (
                "Reusable lesson:** Scope the implementation before handing off.",
                "Reusable lesson:**",
            ),
            (
                "Reusable lesson:** Scope the implementation before handing off.",
                "Reusable lesson:** pending semantic review",
            ),
            (
                "Reusable lesson:** Scope the implementation before handing off.",
                "Reusable lesson:** `<reviewed lesson>`",
            ),
        ] {
            std::fs::write(tasks.join("_lessons.md"), valid.replace(from, to)).unwrap();
            assert_eq!(
                check_lessons_file(dir.path(), &[]).status,
                Status::Warn,
                "{to}"
            );
        }
        std::fs::write(
            tasks.join("_lessons.md"),
            format!("{valid}- **Status:** candidate\n"),
        )
        .unwrap();
        assert_eq!(check_lessons_file(dir.path(), &[]).status, Status::Warn);

        std::fs::write(
            tasks.join("_lessons.md"),
            format!("{valid}\n### Rejected example\n~~~markdown\n- **Status:** candidate\n- **Evidence:** pending\n~~~\n<!--\n- **Reusable lesson:** pending\n-->\n"),
        )
        .unwrap();
        assert_eq!(check_lessons_file(dir.path(), &[]).status, Status::Ok);
    }
}
