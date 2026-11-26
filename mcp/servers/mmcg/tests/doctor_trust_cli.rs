//! `mastermind doctor` against the trust boundary fixtures in
//! `tests/fixtures/trust`. Each run uses an isolated home, so the user's own
//! client settings never reach the report.
#![cfg(unix)]

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Project {
    _temp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/trust")
        .join(name)
}

impl Project {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let root = temp.path().join("project");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&root).unwrap();
        Self {
            home: home.canonicalize().unwrap(),
            root: root.canonicalize().unwrap(),
            _temp: temp,
        }
    }

    /// Copy a fixture file to a path inside the project.
    fn place(&self, source: &str, target: &str) {
        let target = self.root.join(target);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(fixture(source), target).unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mmcg"));
        command
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", &self.home)
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        command
    }

    fn doctor(&self) -> Vec<Value> {
        // The project has no index, so unrelated checks fail on purpose.
        let output = self.command().args(["doctor", "--json"]).output().unwrap();
        let report: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {output:?}"));
        report["checks"].as_array().unwrap().clone()
    }
}

fn check<'a>(checks: &'a [Value], name: &str) -> &'a Value {
    checks
        .iter()
        .find(|check| check["name"] == name)
        .unwrap_or_else(|| panic!("no {name} check in {checks:?}"))
}

fn r1() -> Project {
    let project = Project::new();
    project.place("r1/claude-settings.json", ".claude/settings.json");
    project.place("r1/mcp.json", ".mcp.json");
    project.place("r1/codex-config.toml", ".codex/config.toml");
    project.place("r1/skill-helper.txt", ".claude/skills/helper/SKILL.md");
    project.place("r1/agent-reviewer.txt", ".claude/agents/reviewer.md");
    project
}

#[test]
fn every_misconfiguration_in_r1_is_reported() {
    let checks = r1().doctor();
    let permissions = check(&checks, "permission rules");
    let message = permissions["message"].as_str().unwrap();
    assert_eq!(permissions["status"], "fail", "{message}");
    for expected in [
        "claude-project #1 git (pushes, publishes or calls the GitHub API, warn)",
        "claude-project #2 gh (pushes, publishes or calls the GitHub API, warn)",
        "claude-project #3 gh (pushes, publishes or calls the GitHub API, warn)",
        "claude-project #4 WebFetch (web access to any host, warn)",
        "claude-project #5 mcp__* (every MCP server, which Claude Code skips in allow, warn)",
        "claude-project #6 Edit (writes Mastermind state, warn)",
        "claude-project #7 Edit (writes Mastermind state, warn)",
        "claude-project #8 Read (reads a secret store, error)",
        "claude-project enableAllProjectMcpServers (starts every project MCP server without asking, warn)",
        "mcp-project:helper command (downloads and runs code, error)",
        "claude-project:skill:helper #1 * (repository skill runs chosen code without asking, error)",
        "claude-project:skill:helper #2 WebFetch (web access to any host, warn)",
        "claude-project:agent:reviewer permissionMode (repository subagent asks to bypass prompts, warn)",
        "codex-project:config.toml shell_environment_policy (commands inherit KEY, SECRET and TOKEN variables, warn)",
        "codex-project:config.toml:mcp_servers.helper command (downloads and runs code, error)",
        "codex-config approval_policy (never asks and has network access, error)",
    ] {
        assert!(message.contains(expected), "missing {expected:?} in {message}");
    }
    assert!(!message.contains("and 1 more"), "{message}");
    // Rule text, addresses and commands stay out of the report.
    for secret in ["attacker.example", ".ssh", "curl"] {
        assert!(!message.contains(secret), "{secret} leaked: {message}");
    }

    let hooks = check(&checks, "client hooks");
    assert_eq!(hooks["status"], "fail", "{hooks}");
    assert_eq!(
        hooks["message"],
        "claude-project=1 hook (0 Mastermind, 1 other) | claude-project PreToolUse #1 (downloads and runs code, error)"
    );
    let readiness = check(&checks, "Claude hook readiness");
    assert!(
        readiness["message"]
            .as_str()
            .unwrap()
            .ends_with("; 1 other hook is configured, see client hooks"),
        "{readiness}"
    );
}

#[test]
fn r2_broad_shell_rules_only_warn() {
    let project = Project::new();
    project.place("r2/claude-settings.json", ".claude/settings.json");
    let checks = project.doctor();
    let permissions = check(&checks, "permission rules");
    assert_eq!(permissions["status"], "warn", "{permissions}");
    assert_eq!(
        permissions["message"],
        "claude-project=3 read, 3 flagged | claude-project #1 python (interpreter, warn), \
         claude-project #2 git (git or gh without a fixed subcommand, warn), \
         claude-project #3 curl (network fetch without a fixed host, warn)"
    );
    assert_eq!(check(&checks, "client hooks")["status"], "ok");
}

fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&path, &target.join(entry.file_name()));
        } else {
            fs::copy(&path, target.join(entry.file_name())).unwrap();
        }
    }
}

#[test]
fn mastermind_installed_configuration_stays_clean() {
    let project = Project::new();
    for args in [
        &["miner", "hooks", "setup", "--client", "claude", "--write"][..],
        &["miner", "hooks", "setup", "--client", "codex", "--write"][..],
        &[
            "setup", "claude", "--scope", "project", "--root", ".", "--write",
        ][..],
    ] {
        let output = project.command().args(args).output().unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
    }
    // The shipped skills and subagents, as the installer copies them.
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut skills = Vec::new();
    let mut pending = vec![repo.join("skills")];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.join("SKILL.md").is_file() {
                skills.push(path);
            } else if path.is_dir() {
                pending.push(path);
            }
        }
    }
    assert!(skills.len() > 20, "{skills:?}");
    for skill in &skills {
        copy_tree(
            skill,
            &project
                .root
                .join(".claude/skills")
                .join(skill.file_name().unwrap()),
        );
    }
    copy_tree(
        &repo.join("agents/subagents"),
        &project.root.join(".claude/agents"),
    );

    let checks = project.doctor();
    let permissions = check(&checks, "permission rules");
    assert_eq!(permissions["status"], "ok", "{permissions}");
    let hooks = check(&checks, "client hooks");
    assert_eq!(hooks["status"], "ok", "{hooks}");
    let message = hooks["message"].as_str().unwrap();
    assert!(message.contains("claude-local="), "{message}");
    assert!(message.contains(" Mastermind, 0 other)"), "{message}");
    assert!(!message.contains("not configured"), "{message}");
}
