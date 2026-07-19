//! `mastermind context doctor` — semantic quality audit of project memory.
//!
//! The doctor validates the lean CONTEXT contract, explicit post-flight history
//! review, and structured lesson lifecycle. It deliberately does not require a
//! decision or lesson per task: durable knowledge is selective, not ceremonial.

use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const MAX_LESSONS_SIZE: u64 = 1024 * 1024;
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
    let context_path = root.join("CONTEXT.md");
    let body = std::fs::read_to_string(&context_path).ok();
    let learned_tasks = learned_task_dirs(&root.join(".mastermind/tasks"));
    let checks = vec![
        check_exists(&context_path),
        check_placeholders(body.as_deref()),
        check_minimum_content(body.as_deref()),
        check_core_sections(body.as_deref()),
        check_decision_schema(body.as_deref()),
        check_history_review(&learned_tasks),
        check_lessons_file(root, &learned_tasks),
    ];
    Report::from_checks(root, checks)
}

fn check_exists(path: &Path) -> Check {
    if path.is_file() {
        let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
        Check {
            name: "context.md exists",
            status: Status::Ok,
            message: format!("CONTEXT.md found ({})", format_bytes(size)),
            hint: None,
        }
    } else {
        Check {
            name: "context.md exists",
            status: Status::Fail,
            message: "CONTEXT.md not found at project root".into(),
            hint: Some("run `mastermind init` to scaffold a lean CONTEXT.md".into()),
        }
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

fn check_history_review(tasks: &[PathBuf]) -> Check {
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
        let Ok(body) = std::fs::read_to_string(&review_path) else {
            unresolved.push(format!("{task_name}: missing"));
            continue;
        };
        let context = field_value(&body, "Context");
        let lesson = field_value(&body, "Lesson");
        let reason = field_value(&body, "Reason");
        let valid = |value: Option<&str>| {
            value.is_some_and(|value| {
                matches!(normalized(value).as_str(), "updated" | "not applicable")
            })
        };
        if !valid(context) || !valid(lesson) {
            unresolved.push(format!("{task_name}: pending disposition"));
        } else if reason.is_none_or(|value| {
            value.trim().is_empty() || normalized(value) == "semantic review required"
        }) {
            unresolved.push(format!("{task_name}: reason not reviewed"));
        }
    }
    if unresolved.is_empty() {
        Check {
            name: "history review",
            status: Status::Ok,
            message: format!(
                "{} completed task(s) have explicit dispositions",
                tasks.len()
            ),
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

fn check_lessons_file(root: &Path, tasks: &[PathBuf]) -> Check {
    let lessons_path = root.join(".mastermind/tasks/_lessons.md");
    if !lessons_path.is_file() {
        let expected = tasks.iter().any(|task| {
            std::fs::read_to_string(task.join("history-review.md"))
                .ok()
                .and_then(|body| field_value(&body, "Lesson").map(str::to_owned))
                .is_some_and(|value| normalized(&value) == "updated")
        });
        return if expected {
            Check {
                name: "lessons quality",
                status: Status::Warn,
                message: "history review says lesson updated, but _lessons.md is missing".into(),
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
    let size = std::fs::metadata(&lessons_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if size > MAX_LESSONS_SIZE {
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
    let body = std::fs::read_to_string(&lessons_path).unwrap_or_default();
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
        let missing: Vec<&str> = REQUIRED_LESSON_FIELDS
            .iter()
            .copied()
            .filter(|field| !has_field(block, field))
            .collect();
        if !missing.is_empty() {
            malformed.push(format!("{title}: {}", missing.join(", ")));
        }
        if field_value(block, "Status").is_some_and(|value| normalized(value) == "candidate") {
            candidates += 1;
        }
    }
    if !malformed.is_empty() {
        Check {
            name: "lessons quality",
            status: Status::Warn,
            message: format!("malformed entries: {}", malformed.join("; ")),
            hint: Some("add the missing lifecycle and evidence fields".into()),
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

fn learned_task_dirs(tasks_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(tasks_dir) else {
        return Vec::new();
    };
    let mut tasks = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(path.join("state.json")) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
            continue;
        };
        if value.get("status").and_then(|status| status.as_str()) == Some("learned") {
            tasks.push(path);
        }
    }
    tasks.sort();
    tasks
}

fn markdown_section<'a>(text: &'a str, heading: &str) -> Option<&'a str> {
    let marker = format!("## {heading}");
    let start = text.find(&marker)? + marker.len();
    let rest = &text[start..];
    let end = rest
        .find("\n## ")
        .map(|index| index + 1)
        .unwrap_or(rest.len());
    Some(&rest[..end])
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
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if let Some(title) = line.trim_end().strip_prefix(prefix) {
            starts.push((offset, title));
        }
        offset += line.len();
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
    body.lines()
        .find_map(|line| line.trim().strip_prefix(&marker).map(str::trim))
}

fn normalized(value: &str) -> String {
    value.trim().trim_matches('`').trim().to_ascii_lowercase()
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
    fn completed_task_status_is_parsed_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let learned = tasks.join("001-learned");
        let unrelated = tasks.join("002-unrelated");
        std::fs::create_dir_all(&learned).unwrap();
        std::fs::create_dir_all(&unrelated).unwrap();
        std::fs::write(learned.join("state.json"), r#"{"status":"learned"}"#).unwrap();
        std::fs::write(
            unrelated.join("state.json"),
            r#"{"status":"held","blocking_reason":"not learned yet"}"#,
        )
        .unwrap();
        assert_eq!(learned_task_dirs(&tasks), vec![learned]);
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
        assert_eq!(
            check_history_review(std::slice::from_ref(&task)).status,
            Status::Warn
        );
        std::fs::write(
            task.join("history-review.md"),
            "- **Context:** not applicable\n- **Lesson:** updated\n- **Reason:** Captured a reusable boundary rule.\n",
        )
        .unwrap();
        assert_eq!(check_history_review(&[task]).status, Status::Ok);
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
}
