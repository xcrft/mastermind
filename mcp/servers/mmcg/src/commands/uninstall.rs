//! `mastermind uninstall` command handler.

use std::fs;
use std::path::Path;

use mmcg::setup::{self, Outcome};

use crate::UninstallScope;

/// Remove a Mastermind setup.
///
/// `Project` scope stops each client's capture, mining and profile delivery,
/// then removes `.mastermind/` + the project `.mcp.json` mmcg entry.
/// `Global` de-registers mmcg from Claude Code user scope via `claude mcp remove`.
/// `All` does both. Safe by default — prints the plan and exits unless `force`.
/// Never touches CONTEXT.md / CLAUDE.md.
pub fn do_uninstall(
    root: &Path,
    scope: UninstallScope,
    force: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let do_project = matches!(scope, UninstallScope::Project | UninstallScope::All);
    let do_global = matches!(scope, UninstallScope::Global | UninstallScope::All);
    println!(
        "=== mastermind uninstall — scope: {} ===",
        scope_label(scope)
    );

    if do_project {
        // A failed client step keeps `.mastermind/`, so the command can be repeated.
        remove_clients(root, force)?;
        let mastermind_dir = root.join(".mastermind");
        if mastermind_dir.exists() {
            if force {
                fs::remove_dir_all(&mastermind_dir)?;
                println!(
                    "Removed {} (index, tasks, run-state).",
                    mastermind_dir.display()
                );
            } else {
                println!(
                    "Would remove {} (index, tasks, run-state).",
                    mastermind_dir.display()
                );
            }
        } else {
            println!(
                "No `.mastermind/` at {} — nothing to remove there.",
                root.display()
            );
        }
        check_removal(setup::remove_claude(&setup::Target::project(root), force))?;
        // Other clients are intentionally removed through the explicit,
        // dry-run-first `mastermind setup <client> --remove` surface.
    }

    if do_global {
        check_removal(setup::remove_claude_user(force))?;
    }

    if !force {
        println!("\n(dry-run — pass --force to apply. CONTEXT.md / CLAUDE.md are never touched.)");
    }
    Ok(())
}

fn remove_clients(root: &Path, force: bool) -> Result<(), Box<dyn std::error::Error>> {
    // Hooks can come from `miner hooks setup` without init, and setup.json may
    // be stale or unreadable, so both clients are always cleaned. Removal of an
    // absent client writes nothing.
    for client in ["claude", "codex"] {
        if !force {
            println!(
                "Would stop the {client} worker, remove its hooks and capture grant, and revoke its profile access."
            );
            continue;
        }
        for (component, result) in super::onboard::remove_client(root, client) {
            if let Err(error) = result {
                return Err(format!("{client} {component} failed: {error}. `.mastermind/` was kept. Fix it and run uninstall again").into());
            }
        }
        println!("Stopped {client} capture, mining and profile delivery for this project.");
    }
    Ok(())
}

fn check_removal(outcome: Outcome) -> Result<(), &'static str> {
    match outcome {
        Outcome::Error => Err("Claude Code registration removal failed"),
        Outcome::RefusedOverwrite => Err("Claude Code registration removal was refused"),
        _ => Ok(()),
    }
}

fn scope_label(s: UninstallScope) -> &'static str {
    match s {
        UninstallScope::Project => "project (.mastermind/ + project .mcp.json)",
        UninstallScope::Global => "global (claude mcp remove → ~/.claude.json)",
        UninstallScope::All => "all (project + global)",
    }
}
