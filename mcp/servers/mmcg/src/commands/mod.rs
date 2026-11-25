//! Command handlers for the `mastermind` CLI.
//!
//! Each submodule owns one command's full logic so `main.rs` stays a thin
//! dispatcher. `UninstallScope` lives in `main.rs` because it is a
//! `ValueEnum` embedded in the clap CLI spec.

pub mod acceptance;
pub mod audit;
pub mod ci;
pub mod demo;
pub mod effort;
pub mod init;
pub mod new_spec;
pub mod onboard;
pub mod pr_comment;
pub mod query;
pub mod run_task;
pub mod spec_gate;
pub mod task_review;
pub mod tour;
pub mod uninstall;
mod verification;

pub use ci::run as ci;
pub use demo::run as demo;
pub use new_spec::run as new_spec;
pub use pr_comment::run as pr_comment;
pub use query::dispatch as dispatch_query;
pub use run_task::dispatch as run_task;
pub use spec_gate::{audit as audit_spec, verify as verify_spec};
pub use tour::run as tour;
pub use uninstall::do_uninstall;
pub use verification::dispatch as verification_run;

use std::path::{Path, PathBuf};

/// Resolves a task command's root and spec. The default root comes from the
/// physical working directory, while an absolute spec may spell the same root
/// through a linked alias, as `$PWD` does in a linked directory, or through
/// `..`, as `sub/../spec.md` does. Only the longest spec prefix that resolves
/// to the root is rewritten, so the bounded readers still see and refuse links
/// below it.
pub(crate) fn task_paths(spec: &Path, root: PathBuf) -> std::io::Result<(PathBuf, PathBuf)> {
    let root = std::path::absolute(root)?;
    let spec = if !spec.is_absolute() {
        root.join(spec)
    } else {
        root_alias_suffix(spec, &root).map_or_else(|| spec.to_path_buf(), |rest| root.join(rest))
    };
    Ok((root, spec))
}

fn root_alias_suffix<'a>(spec: &'a Path, root: &Path) -> Option<&'a Path> {
    let canonical_root = root.canonicalize().ok()?;
    let alias = spec.ancestors().skip(1).find(|ancestor| {
        ancestor
            .canonicalize()
            .is_ok_and(|path| path == canonical_root)
    })?;
    spec.strip_prefix(alias).ok()
}
