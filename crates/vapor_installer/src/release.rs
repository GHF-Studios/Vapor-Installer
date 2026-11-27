//! Compatibility re-exports for the shared Vapor release trust boundary.
//!
//! Release manifest authentication is owned by Vapor Core so the standalone
//! updater and Developer role provisioning use the same pinned key and schema.

pub use vapor_core::release::*;
