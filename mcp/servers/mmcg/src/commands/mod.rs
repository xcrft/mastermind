//! Command handlers for the `mastermind` CLI.
//!
//! Each submodule owns one command's full logic so `main.rs` stays a thin
//! dispatcher. `UninstallScope` lives in `main.rs` because it is a
//! `ValueEnum` embedded in the clap CLI spec.

pub mod audit;
pub mod ci;
pub mod demo;
pub mod init;
pub mod new_spec;
pub mod pr_comment;
pub mod query;
pub mod run_task;
pub mod spec_gate;
pub mod tour;
pub mod uninstall;

pub use ci::run as ci;
pub use demo::run as demo;
pub use init::do_init;
pub use new_spec::run as new_spec;
pub use pr_comment::run as pr_comment;
pub use query::dispatch as dispatch_query;
pub use run_task::dispatch as run_task;
pub use spec_gate::{audit as audit_spec, verify as verify_spec};
pub use tour::run as tour;
pub use uninstall::do_uninstall;
