//! `mastermind run-task` command handler.

use std::path::{Path, PathBuf};

pub fn dispatch(
    spec: &Path,
    root: PathBuf,
    index_path: &Path,
    opts: mmcg::run_task::RunOpts,
) -> Result<mmcg::run_task::Outcome, Box<dyn std::error::Error>> {
    // Keep the requested root spelling alongside absolute spec paths. The
    // bounded reader retains both requested and canonical roots for admission.
    let root =
        std::path::absolute(&root).map_err(|e| format!("resolve root {}: {e}", root.display()))?;
    let spec = if spec.is_absolute() {
        spec.to_path_buf()
    } else {
        root.join(spec)
    };
    Ok(mmcg::run_task::run(&spec, &root, index_path, opts))
}
