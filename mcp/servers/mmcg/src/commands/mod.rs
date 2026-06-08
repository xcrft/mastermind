//! Command handlers for the `mastermind` CLI.
//!
//! Each submodule owns one command's full logic so `main.rs` stays a thin
//! dispatcher. Shared types (`Profile`, `UninstallScope`) live in `main.rs`
//! because they are `ValueEnum` variants embedded in the clap CLI spec.

pub mod init;
pub mod uninstall;

pub use init::do_init;
pub use uninstall::do_uninstall;
