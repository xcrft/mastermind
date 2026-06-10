//! `mastermind run-task` command handler.

use std::path::{Path, PathBuf};

pub fn dispatch(
    spec: &Path,
    root: PathBuf,
    index_path: &Path,
    reset: bool,
    pre_only: bool,
    post_only: bool,
    exec: bool,
    allow_no_index: bool,
    strict: bool,
    max_iterations: u32,
    force_iteration: bool,
) -> Result<mmcg::run_task::Outcome, Box<dyn std::error::Error>> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
    let opts = mmcg::run_task::RunOpts {
        reset,
        pre_only,
        post_only,
        exec,
        allow_no_index,
        strict,
        max_iterations,
        force_iteration,
    };
    Ok(mmcg::run_task::run(spec, &root, index_path, opts))
}
