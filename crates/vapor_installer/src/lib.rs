//! Precompiled Vapor Vapor-home maintenance and workflow shim.
//!
//! Setup, maintenance, and IDE workflow launch behavior delegates into the
//! shared Vapor-home tool implementation in-process. That keeps bootstrap paths
//! usable before Rust, Cargo, Git, or SteamCMD providers are installed.

pub mod app_release;
pub mod cli;
pub mod release;
pub mod standalone;
mod script_bridge;

use std::path::Path;

/// Establish machine integration and the Player receipt as one idempotent
/// installation operation. CLI and GUI use this same boundary.
pub fn prepare_player_installation(
    installation: &vapor_core::VaporInstallation,
) -> Result<vapor_core::InstallReport, String> {
    let report =
        vapor_core::install_installation_for(installation).map_err(|error| error.to_string())?;
    setup_player_for_app_root(&installation.root)?;
    Ok(report)
}

/// Install app-local player prerequisites for the selected Steam installation.
pub fn setup_player_for_app_root(app_root: &Path) -> Result<(), String> {
    script_bridge::setup_player_for(app_root)
}

/// Install the app-local Rust, cross-compilation tools, and setup state used
/// by the Developer role. GUI and CLI entrypoints share this implementation.
pub fn setup_development_for_app_root_with_progress(
    app_root: &Path,
    report_progress: impl FnMut(&str),
) -> Result<(), String> {
    script_bridge::setup_development_for_with_progress(app_root, report_progress)
}

/// Remove Vapor-managed developer tools from the selected Vapor Home.
pub fn teardown_development_for_app_root(app_root: &Path) -> Result<(), String> {
    script_bridge::teardown_development_for(app_root)
}
