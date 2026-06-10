//! `mastermind context doctor` — quality audit of `CONTEXT.md`.
//!
//! Checks (in order):
//!
//! | # | Name                   | Catches                                                |
//! |---|------------------------|--------------------------------------------------------|
//! | 1 | `context.md exists`    | file is missing entirely                               |
//! | 2 | `no placeholders`      | unfilled `<PLACEHOLDER>` / `<TODO>` / `<FILL>` tokens |
//! | 3 | `minimum content`      | file is basically empty (< 100 non-whitespace chars)   |
//! | 4 | `stack section`        | no Stack / Tech / Language heading                     |
//! | 5 | `decision log`         | completed tasks exist but no Decision Log section      |
//! | 6 | `freshness`            | newest task spec is newer than CONTEXT.md              |
//! | 7 | `lessons file`         | learned tasks exist but no _lessons.md                 |

use std::path::Path;
use serde::Serialize;

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
            ok: checks.iter().filter(|c| c.status == Status::Ok).count() as u32,
            warn: checks.iter().filter(|c| c.status == Status::Warn).count() as u32,
            fail: checks.iter().filter(|c| c.status == Status::Fail).count() as u32,
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
            "mastermind context doctor — checking CONTEXT.md at {}\n\n",
            self.root
        ));
        let name_width = self
            .checks
            .iter()
            .map(|c| c.name.chars().count())
            .max()
            .unwrap_or(20);
        for c in &self.checks {
            let marker = match c.status {
                Status::Ok => "✅",
                Status::Warn => "⚠️ ",
                Status::Fail => "❌",
            };
            out.push_str(&format!(
                "  {marker} {name:<width$}  {msg}\n",
                marker = marker,
                name = c.name,
                width = name_width,
                msg = c.message,
            ));
            if let Some(hint) = &c.hint {
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
            out.push_str("  Placeholders left  fill every <PLACEHOLDER> / <TODO> with real project data\n");
            out.push_str("  Stale context      add a Decision Log entry after each completed task\n");
            out.push_str("  No lessons file    create `.mastermind/tasks/_lessons.md` with lessons from audits\n");
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

    let checks = vec![
        check_exists(&context_path),
        check_placeholders(body.as_deref()),
        check_minimum_content(body.as_deref()),
        check_stack_section(body.as_deref()),
        check_decision_log(root, body.as_deref()),
        check_freshness(root, &context_path),
        check_lessons_file(root),
    ];
    Report::from_checks(root, checks)
}

fn check_exists(path: &Path) -> Check {
    if path.is_file() {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
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
            hint: Some("run `mastermind init` to scaffold CONTEXT.md from the codebase".into()),
        }
    }
}

fn check_placeholders(body: Option<&str>) -> Check {
    let Some(text) = body else {
        return Check {
            name: "no placeholders",
            status: Status::Warn,
            message: "skipped — file not readable".into(),
            hint: None,
        };
    };
    let markers = ["<PLACEHOLDER>", "<TODO>", "<FILL>", "<fill>", "<todo>"];
    let hits: Vec<&str> = markers.iter().copied().filter(|m| text.contains(m)).collect();
    if hits.is_empty() {
        Check {
            name: "no placeholders",
            status: Status::Ok,
            message: "no unfilled placeholder tokens".into(),
            hint: None,
        }
    } else {
        let found = hits.join(", ");
        Check {
            name: "no placeholders",
            status: Status::Fail,
            message: format!("unfilled placeholder(s) found: {found}"),
            hint: Some("replace every placeholder with real project-specific content".into()),
        }
    }
}

fn check_minimum_content(body: Option<&str>) -> Check {
    let Some(text) = body else {
        return Check {
            name: "minimum content",
            status: Status::Warn,
            message: "skipped — file not readable".into(),
            hint: None,
        };
    };
    let non_whitespace: usize = text.chars().filter(|c| !c.is_whitespace()).count();
    if non_whitespace >= 200 {
        Check {
            name: "minimum content",
            status: Status::Ok,
            message: format!("{non_whitespace} non-whitespace chars"),
            hint: None,
        }
    } else if non_whitespace >= 50 {
        Check {
            name: "minimum content",
            status: Status::Warn,
            message: format!("only {non_whitespace} non-whitespace chars — likely a template stub"),
            hint: Some(
                "populate CONTEXT.md with stack, architecture, conventions, and key decisions"
                    .into(),
            ),
        }
    } else {
        Check {
            name: "minimum content",
            status: Status::Fail,
            message: format!("{non_whitespace} non-whitespace chars — file is essentially empty"),
            hint: Some(
                "run `mastermind init` with `--no-claude` removed to auto-populate CONTEXT.md from the codebase".into(),
            ),
        }
    }
}

fn check_stack_section(body: Option<&str>) -> Check {
    let Some(text) = body else {
        return Check {
            name: "stack section",
            status: Status::Warn,
            message: "skipped — file not readable".into(),
            hint: None,
        };
    };
    let has_stack = text.lines().any(|l| {
        let lower = l.to_lowercase();
        (l.starts_with('#') || l.starts_with("##") || l.starts_with("###"))
            && (lower.contains("stack")
                || lower.contains("tech")
                || lower.contains("language")
                || lower.contains("runtime")
                || lower.contains("framework"))
    });
    if has_stack {
        Check {
            name: "stack section",
            status: Status::Ok,
            message: "stack / tech section found".into(),
            hint: None,
        }
    } else {
        Check {
            name: "stack section",
            status: Status::Warn,
            message: "no Stack / Tech / Language / Framework heading found".into(),
            hint: Some(
                "add a section describing the language, runtime, and key libraries — the executor uses this to pick tools".into(),
            ),
        }
    }
}

fn check_decision_log(root: &Path, body: Option<&str>) -> Check {
    let tasks_dir = root.join(".mastermind").join("tasks");
    let has_completed_tasks = completed_task_count(&tasks_dir) > 0;

    if !has_completed_tasks {
        return Check {
            name: "decision log",
            status: Status::Ok,
            message: "no completed tasks yet — decision log not required".into(),
            hint: None,
        };
    }

    let Some(text) = body else {
        return Check {
            name: "decision log",
            status: Status::Warn,
            message: "skipped — CONTEXT.md not readable".into(),
            hint: None,
        };
    };

    let has_log = text.lines().any(|l| {
        let lower = l.to_lowercase();
        (l.starts_with('#'))
            && (lower.contains("decision")
                || lower.contains("history")
                || lower.contains("log")
                || lower.contains("change"))
    });

    if has_log {
        Check {
            name: "decision log",
            status: Status::Ok,
            message: "decision log section present".into(),
            hint: None,
        }
    } else {
        let n = completed_task_count(&tasks_dir);
        Check {
            name: "decision log",
            status: Status::Warn,
            message: format!("{n} completed task(s) but no Decision Log section in CONTEXT.md"),
            hint: Some(
                "add a ## Decision Log section and record one line per completed task — future planners use this to avoid repeating rejected approaches".into(),
            ),
        }
    }
}

fn check_freshness(root: &Path, context_path: &Path) -> Check {
    if !context_path.is_file() {
        return Check {
            name: "freshness",
            status: Status::Warn,
            message: "skipped — CONTEXT.md not found".into(),
            hint: None,
        };
    }

    let ctx_mtime = match std::fs::metadata(context_path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => {
            return Check {
                name: "freshness",
                status: Status::Warn,
                message: "cannot stat CONTEXT.md mtime".into(),
                hint: None,
            };
        }
    };

    let tasks_dir = root.join(".mastermind").join("tasks");
    let newest_spec = newest_spec_mtime(&tasks_dir);

    let Some(spec_mtime) = newest_spec else {
        return Check {
            name: "freshness",
            status: Status::Ok,
            message: "no task specs found — nothing to compare against".into(),
            hint: None,
        };
    };

    if spec_mtime <= ctx_mtime {
        Check {
            name: "freshness",
            status: Status::Ok,
            message: "CONTEXT.md is at least as recent as the newest task spec".into(),
            hint: None,
        }
    } else {
        let delta_secs = spec_mtime
            .duration_since(ctx_mtime)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let delta_label = format_duration(delta_secs);
        Check {
            name: "freshness",
            status: Status::Warn,
            message: format!("newest task spec is {delta_label} newer than CONTEXT.md"),
            hint: Some(
                "update CONTEXT.md Decision Log / Architecture section to reflect recent changes".into(),
            ),
        }
    }
}

fn check_lessons_file(root: &Path) -> Check {
    let tasks_dir = root.join(".mastermind").join("tasks");
    let n = completed_task_count(&tasks_dir);
    if n == 0 {
        return Check {
            name: "lessons file",
            status: Status::Ok,
            message: "no completed tasks yet — _lessons.md not required".into(),
            hint: None,
        };
    }

    let lessons = tasks_dir.join("_lessons.md");
    if lessons.is_file() {
        let size = std::fs::metadata(&lessons).map(|m| m.len()).unwrap_or(0);
        Check {
            name: "lessons file",
            status: Status::Ok,
            message: format!("_lessons.md found ({})", format_bytes(size)),
            hint: None,
        }
    } else {
        Check {
            name: "lessons file",
            status: Status::Warn,
            message: format!("{n} completed task(s) but no .mastermind/tasks/_lessons.md"),
            hint: Some(
                "create _lessons.md — the auditor appends one-liner lessons after each audit; planners read it before designing".into(),
            ),
        }
    }
}

fn completed_task_count(tasks_dir: &Path) -> usize {
    if !tasks_dir.is_dir() {
        return 0;
    }
    let mut count = 0usize;
    if let Ok(entries) = std::fs::read_dir(tasks_dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let state_path = entry.path().join("state.json");
            if state_path.is_file() {
                if let Ok(body) = std::fs::read_to_string(&state_path) {
                    if body.contains("\"learned\"") {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

fn newest_spec_mtime(tasks_dir: &Path) -> Option<std::time::SystemTime> {
    if !tasks_dir.is_dir() {
        return None;
    }
    let mut newest: Option<std::time::SystemTime> = None;
    if let Ok(entries) = std::fs::read_dir(tasks_dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let spec = entry.path().join("spec.md");
            if let Ok(meta) = std::fs::metadata(&spec) {
                if let Ok(mtime) = meta.modified() {
                    newest = Some(match newest {
                        None => mtime,
                        Some(prev) => prev.max(mtime),
                    });
                }
            }
        }
    }
    newest
}

fn format_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}
