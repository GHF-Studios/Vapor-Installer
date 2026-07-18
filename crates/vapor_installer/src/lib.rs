//! Cross-platform Vapor app-root installer.
//!
//! The installer owns mutation of disposable app-root tooling and generated
//! state. Vapor Shell may inspect or invoke this binary, but installer behavior
//! should not leak into ordinary shell commands.

mod acquire;
mod app_root;
mod bootstrap;
pub mod cli;
mod dev_env;
mod fsutil;
mod git;
mod lifecycle;
mod model;
mod paths;

pub use bootstrap::{player_install, player_status, player_uninstall};
pub use dev_env::{dev_env_install, dev_env_status, dev_env_uninstall};
pub use lifecycle::{install, uninstall};
pub use model::{
    ComponentStatus, DevEnvStatus, INSTALLER_LOG, InstallerOptions, InstallerReport, PlayerStatus,
};
