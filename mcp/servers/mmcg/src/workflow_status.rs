use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskPhase {
    Ready,
    AwaitingExecutor,
    AwaitingAudit,
    Complete,
}

impl TaskPhase {
    fn label(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::AwaitingExecutor => "awaiting executor",
            Self::AwaitingAudit => "awaiting audit",
            Self::Complete => "complete",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskInfo {
    pub folder: String,
    pub spec_path: PathBuf,
    pub phase: TaskPhase,
}

pub struct IndexInfo {
    pub db_exists: bool,
    pub symbol_count: u64,
    pub file_count: u64,
    pub stale_count: usize,
}

pub struct InstallInfo {
    pub claude_md_present: bool,
    pub agents_count: usize,
    pub skills_count: usize,
}

pub struct WorkflowStatus {
    pub root: PathBuf,
    pub index: IndexInfo,
    pub install: InstallInfo,
    pub tasks: Vec<TaskInfo>,
}

pub struct NextAction {
    pub description: String,
    pub command: Option<String>,
    pub claude_prompt: Option<String>,
}

impl WorkflowStatus {
    pub fn scan(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            index: scan_index(root),
            install: scan_install(root),
            tasks: scan_tasks(root),
        }
    }

    pub fn next_action(&self) -> Option<NextAction> {
        if let Some(task) = self.tasks.iter().find(|t| t.phase == TaskPhase::AwaitingAudit) {
            let spec = task.spec_path.display().to_string();
            let task_dir = task.spec_path.parent().unwrap_or(task.spec_path.as_path());
            return Some(NextAction {
                description: format!("Task {} — executor done, run post-flight audit", task.folder),
                command: Some(format!("mastermind run-task {} --post-only", spec)),
                claude_prompt: Some(format!(
                    "Run the Mastermind post-flight audit for:\n\
                     {spec}\n\n\
                     The executor report is in {}. \
                     Invoke the Mastermind auditor subagent to verify the execution matched the spec.",
                    task_dir.display()
                )),
            });
        }
        if let Some(task) = self.tasks.iter().find(|t| t.phase == TaskPhase::AwaitingExecutor) {
            let spec = task.spec_path.display().to_string();
            let task_dir = task.spec_path.parent().unwrap_or(task.spec_path.as_path());
            return Some(NextAction {
                description: format!(
                    "Task {} — pre-flight passed, invoke the executor",
                    task.folder
                ),
                command: None,
                claude_prompt: Some(format!(
                    "Run the Mastermind executor for:\n\
                     {spec}\n\n\
                     Read the spec, implement each step in the Scope section, run the VERIFY \
                     commands, and write an executor report to {}/executor-report.md.",
                    task_dir.display()
                )),
            });
        }
        if let Some(task) = self.tasks.iter().find(|t| t.phase == TaskPhase::Ready) {
            return Some(NextAction {
                description: format!("Task {} — spec ready for pre-flight", task.folder),
                command: Some(format!(
                    "mastermind run-task {}",
                    task.spec_path.display()
                )),
                claude_prompt: None,
            });
        }
        if !self.tasks.is_empty() && self.tasks.iter().all(|t| t.phase == TaskPhase::Complete) {
            return Some(NextAction {
                description: "All tasks complete.".into(),
                command: Some("mastermind new-spec 'description of next task'".into()),
                claude_prompt: None,
            });
        }
        None
    }

    pub fn render_text(&self) -> String {
        let mut out = String::new();
        out.push_str("Mastermind status\n\n");

        out.push_str("Index\n");
        if !self.index.db_exists {
            out.push_str("  ✗ no index — run `mastermind index .` or `mastermind init`\n");
        } else {
            out.push_str(&format!(
                "  ✓ .mastermind/mmcg.db — {} symbols, {} files\n",
                self.index.symbol_count, self.index.file_count
            ));
            if self.index.stale_count == 0 {
                out.push_str("  ✓ index up to date\n");
            } else {
                let suffix = if self.index.stale_count >= 10 { " or more" } else { "" };
                out.push_str(&format!(
                    "  ⚠ {} source file(s){} changed since last index — run `mastermind index .`\n",
                    self.index.stale_count, suffix
                ));
            }
        }
        out.push('\n');

        out.push_str("Workflow\n");
        if self.install.claude_md_present {
            out.push_str("  ✓ CLAUDE.md present\n");
        } else {
            out.push_str(
                "  ⚠ CLAUDE.md not found — run `mastermind init --with-claude-md`\n",
            );
        }
        if self.install.agents_count > 0 {
            out.push_str(&format!(
                "  ✓ {} subagent(s) in ~/.claude/agents/\n",
                self.install.agents_count
            ));
        } else {
            out.push_str(
                "  ⚠ no subagents in ~/.claude/agents/ — run `mastermind init`\n",
            );
        }
        if self.install.skills_count > 0 {
            out.push_str(&format!(
                "  ✓ {} skill(s) in ~/.claude/skills/\n",
                self.install.skills_count
            ));
        } else {
            out.push_str("  ⚠ no skills in ~/.claude/skills/ — run `mastermind init`\n");
        }
        out.push('\n');

        if self.tasks.is_empty() {
            out.push_str(
                "Tasks\n  (none — create one with `mastermind new-spec 'description'`)\n\n",
            );
        } else {
            out.push_str("Tasks\n");
            let name_w = self
                .tasks
                .iter()
                .map(|t| t.folder.len())
                .max()
                .unwrap_or(20);
            for task in &self.tasks {
                let marker = match task.phase {
                    TaskPhase::Complete => "✓",
                    TaskPhase::Ready => "○",
                    TaskPhase::AwaitingExecutor | TaskPhase::AwaitingAudit => "⚡",
                };
                out.push_str(&format!(
                    "  {marker} {:<width$}  {}\n",
                    task.folder,
                    task.phase.label(),
                    width = name_w
                ));
            }
            out.push('\n');
        }

        if let Some(action) = self.next_action() {
            out.push_str("Next step\n");
            out.push_str(&format!("  {}\n", action.description));
            if let Some(ref cmd) = action.command {
                out.push_str(&format!("\n  Run:  {cmd}\n"));
            }
            if let Some(ref prompt) = action.claude_prompt {
                out.push_str("\n  Or paste into Claude:\n\n");
                for line in prompt.lines() {
                    out.push_str(&format!("    {line}\n"));
                }
            }
        }

        out
    }

    pub fn render_next_text(&self) -> String {
        match self.next_action() {
            None if self.tasks.is_empty() => {
                "No active tasks.\n\nCreate one: mastermind new-spec 'description'\n".into()
            }
            None => "No pending action found.\n".into(),
            Some(action) => {
                let mut out = String::new();
                out.push_str(&format!("{}\n", action.description));
                if let Some(ref cmd) = action.command {
                    out.push_str(&format!("\nRun:\n  {cmd}\n"));
                }
                if let Some(ref prompt) = action.claude_prompt {
                    out.push_str("\nPaste into Claude:\n\n");
                    for line in prompt.lines() {
                        out.push_str(&format!("  {line}\n"));
                    }
                }
                out
            }
        }
    }
}

fn scan_index(root: &Path) -> IndexInfo {
    let db = root.join(".mastermind").join("mmcg.db");
    if !db.is_file() {
        return IndexInfo {
            db_exists: false,
            symbol_count: 0,
            file_count: 0,
            stale_count: 0,
        };
    }

    let (symbol_count, file_count) = db_counts(&db).unwrap_or((0, 0));

    let stale_count = std::fs::metadata(&db)
        .and_then(|m| m.modified())
        .map(|db_mtime| count_stale(root, db_mtime, 10))
        .unwrap_or(0);

    IndexInfo {
        db_exists: true,
        symbol_count,
        file_count,
        stale_count,
    }
}

fn db_counts(db: &Path) -> Option<(u64, u64)> {
    let conn = rusqlite::Connection::open(db).ok()?;
    let sym: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
        .ok()?;
    let fil: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .ok()?;
    Some((sym.max(0) as u64, fil.max(0) as u64))
}

fn count_stale(root: &Path, db_mtime: std::time::SystemTime, cap: usize) -> usize {
    let mut n = 0usize;
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_skipped(e.file_name().to_str().unwrap_or("")))
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if crate::indexer::extractor_for_path(entry.path()).is_none() {
            continue;
        }
        if let Ok(fs_mtime) = entry.path().metadata().and_then(|m| m.modified()) {
            if fs_mtime > db_mtime {
                n += 1;
                if n >= cap {
                    break;
                }
            }
        }
    }
    n
}

fn is_skipped(name: &str) -> bool {
    matches!(
        name,
        "node_modules"
            | ".git"
            | ".mastermind"
            | "target"
            | "__pycache__"
            | ".venv"
            | "venv"
            | "dist"
            | "build"
            | ".next"
            | ".nuxt"
            | "coverage"
            | ".nyc_output"
            | "vendor"
    )
}

fn scan_install(root: &Path) -> InstallInfo {
    let claude_md_present = root.join("CLAUDE.md").is_file();
    let (agents_count, skills_count) = match dirs::home_dir() {
        None => (0, 0),
        Some(home) => {
            let agents_dir = home.join(".claude").join("agents");
            let skills_dir = home.join(".claude").join("skills");
            let agents = count_matching_files(&agents_dir, "mastermind-", ".md");
            let skills = count_matching_dirs(&skills_dir, "mastermind-");
            (agents, skills)
        }
    };
    InstallInfo {
        claude_md_present,
        agents_count,
        skills_count,
    }
}

fn count_matching_files(dir: &Path, prefix: &str, suffix: &str) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    let n = e.file_name();
                    let s = n.to_string_lossy();
                    e.file_type().map(|t| t.is_file()).unwrap_or(false)
                        && s.starts_with(prefix)
                        && s.ends_with(suffix)
                })
                .count()
        })
        .unwrap_or(0)
}

fn count_matching_dirs(dir: &Path, prefix: &str) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                        && e.file_name().to_string_lossy().starts_with(prefix)
                })
                .count()
        })
        .unwrap_or(0)
}

fn scan_tasks(root: &Path) -> Vec<TaskInfo> {
    let tasks_dir = root.join(".mastermind").join("tasks");
    if !tasks_dir.is_dir() {
        return vec![];
    }

    let inflight_spec = read_inflight_spec(root);

    let mut entries: Vec<_> = std::fs::read_dir(&tasks_dir)
        .map(|rd| rd.filter_map(|e| e.ok()).collect())
        .unwrap_or_default();
    entries.sort_by_key(|e| e.file_name());

    let mut tasks = Vec::new();
    for entry in entries {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let folder = entry.file_name().to_string_lossy().to_string();
        let spec_path = entry.path().join("spec.md");
        if !spec_path.is_file() {
            continue;
        }
        let phase = detect_phase(&spec_path, &inflight_spec);
        tasks.push(TaskInfo { folder, spec_path, phase });
    }
    tasks
}

fn read_inflight_spec(root: &Path) -> Option<PathBuf> {
    let state_file = root
        .join(".mastermind")
        .join("run-state")
        .join("spec.json");
    if !state_file.is_file() {
        return None;
    }
    let body = std::fs::read_to_string(&state_file).ok()?;
    let state: crate::run_task::RunState = serde_json::from_str(&body).ok()?;
    Some(PathBuf::from(state.spec_path))
}

fn detect_phase(spec_path: &Path, inflight_spec: &Option<PathBuf>) -> TaskPhase {
    let task_dir = spec_path.parent().unwrap_or(spec_path);

    if task_dir.join("audit.md").is_file() {
        return TaskPhase::Complete;
    }

    let is_inflight = inflight_spec.as_deref().is_some_and(|inflight| {
        let a = spec_path.canonicalize().ok();
        let b = inflight.canonicalize().ok();
        match (a, b) {
            (Some(ca), Some(cb)) => ca == cb,
            _ => spec_path == inflight,
        }
    });

    if is_inflight {
        if task_dir.join("executor-report.md").is_file() {
            return TaskPhase::AwaitingAudit;
        }
        return TaskPhase::AwaitingExecutor;
    }

    TaskPhase::Ready
}
