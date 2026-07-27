//! Vapor Installer compatibility shim.
//!
//! DevOps/setup behavior lives in the super-workspace `tools/` scripts. This
//! crate stays intentionally small so Steam launch wrappers and old command
//! muscle memory can delegate into that script layer.

pub mod cli;
mod script_bridge;
