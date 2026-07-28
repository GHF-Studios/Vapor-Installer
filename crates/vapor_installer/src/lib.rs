//! Precompiled Vapor app-root maintenance and workflow shim.
//!
//! Setup, maintenance, and IDE workflow launch behavior delegates into the
//! shared app-root tool implementation in-process. That keeps bootstrap paths
//! usable before Rust, Cargo, Git, or SteamCMD providers are installed.

pub mod cli;
mod script_bridge;
