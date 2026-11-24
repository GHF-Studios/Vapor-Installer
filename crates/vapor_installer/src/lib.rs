//! Precompiled Vapor app-root maintenance and workflow shim.
//!
//! Setup, maintenance, and IDE workflow launch behavior delegates into the
//! shared app-root tool implementation in-process. That keeps bootstrap paths
//! usable before Rust, Cargo, Git, or SteamCMD providers are installed.

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
