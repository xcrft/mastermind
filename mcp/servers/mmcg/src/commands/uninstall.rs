//! `mastermind uninstall` command handler.

use std::fs;
use std::path::Path;

use crate::UninstallScope;

/// Remove a Mastermind setup.
///
/// `Project` scope removes `.mastermind/` + the project `.mcp.json` mmcg entry.
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
        mmcg::setup::remove_claude(&mmcg::setup::Target::project(root), force);
    }

    if do_global {
        mmcg::setup::remove_claude_user(force);
    }

    if !force {
        println!("\n(dry-run — pass --force to apply. CONTEXT.md / CLAUDE.md are never touched.)");
    }
    Ok(())
}

fn scope_label(s: UninstallScope) -> &'static str {
    match s {
        UninstallScope::Project => "project (.mastermind/ + project .mcp.json)",
        UninstallScope::Global => "global (claude mcp remove → ~/.claude.json)",
        UninstallScope::All => "all (project + global)",
    }
}
