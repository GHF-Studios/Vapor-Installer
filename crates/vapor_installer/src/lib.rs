//! Precompiled Vapor Vapor-home maintenance and workflow shim.
//!
//! Setup, maintenance, and IDE workflow launch behavior delegates into the
//! shared Vapor-home tool implementation in-process. That keeps bootstrap paths
//! usable before Rust, Cargo, Git, or SteamCMD providers are installed.

pub mod app_release;
pub mod cli;
mod script_bridge;

use std::path::Path;

/// Create Vapor-home player state for the selected standalone installation.
pub fn setup_player_for_vapor_home(app_root: &Path) -> Result<(), String> {
    script_bridge::setup_player_for(app_root)
}
