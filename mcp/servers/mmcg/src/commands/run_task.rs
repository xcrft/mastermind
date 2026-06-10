//! `mastermind run-task` command handler.

use std::path::{Path, PathBuf};

pub fn dispatch(
    spec: &Path,
    root: PathBuf,
    index_path: &Path,
    opts: mmcg::run_task::RunOpts,
) -> Result<mmcg::run_task::Outcome, Box<dyn std::error::Error>> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
    Ok(mmcg::run_task::run(spec, &root, index_path, opts))
}
