//! Precompiled Vapor app-root maintenance and workflow shim.
//!
//! Setup, maintenance, and IDE workflow launch behavior delegates into the
//! shared app-root tool implementation in-process. That keeps bootstrap paths
//! usable before Rust, Cargo, Git, or SteamCMD providers are installed.

pub mod cli;
mod script_bridge;

use std::path::Path;

/// Install app-local player prerequisites for the selected Steam installation.
pub fn setup_player_for_app_root(app_root: &Path) -> Result<(), String> {
    script_bridge::setup_player_for(app_root)
}
