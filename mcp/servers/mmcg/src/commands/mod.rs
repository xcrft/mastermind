//! Command handlers for the `mastermind` CLI.
//!
//! Each submodule owns one command's full logic so `main.rs` stays a thin
//! dispatcher. Shared types (`Profile`, `UninstallScope`) live in `main.rs`
//! because they are `ValueEnum` variants embedded in the clap CLI spec.

pub mod demo;
pub mod init;
pub mod new_spec;
pub mod query;
pub mod run_task;
pub mod spec_gate;
pub mod uninstall;

pub use demo::run as demo;
pub use init::do_init;
pub use new_spec::run as new_spec;
pub use query::dispatch as dispatch_query;
pub use run_task::dispatch as run_task;
pub use spec_gate::{audit as audit_spec, verify as verify_spec};
pub use uninstall::do_uninstall;
