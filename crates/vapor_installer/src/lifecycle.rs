//! Top-level installer lifecycle commands.
//!
//! These functions are the public command model:
//!
//! - `install` prepares the default player-mode app root.
//! - `uninstall` removes all installer-managed mutable app-root state.
//! - `dev-env install` and `dev-env uninstall` live in `dev_env` as explicit
//!   upgrade/downgrade operations.

use crate::{
    app_root::resolve_app_root,
    bootstrap::{player_install, player_uninstall},
    dev_env::dev_env_uninstall,
    model::{InstallerOptions, InstallerReport},
};

/// Install or reconcile the default player-mode app-root tooling.
///
/// # Errors
///
/// Fails when player-mode acquisition, extraction, verification, or path
/// confinement fails.
pub fn install(options: &InstallerOptions) -> Result<InstallerReport, String> {
    player_install(options)
}

/// Uninstall every installer-managed mutable app-root path.
///
/// This removes development tooling first, then removes default player-mode
/// tooling, generated state, logs, caches, downloads, and receipts. Depot-owned
/// files are left for Steam's own uninstall operation.
///
/// # Errors
///
/// Fails when any target path escapes the app root or removal fails.
pub fn uninstall(options: &InstallerOptions) -> Result<InstallerReport, String> {
    let app_root = resolve_app_root(options.app_root.as_deref())?;
    let scoped_options = InstallerOptions {
        app_root: Some(app_root.clone()),
        dry_run: options.dry_run,
    };

    let dev_env = dev_env_uninstall(&scoped_options)?;
    let player = player_uninstall(&scoped_options)?;
    let mut actions = Vec::new();
    actions.extend(
        dev_env
            .actions()
            .iter()
            .map(|action| format!("dev-env: {action}")),
    );
    actions.extend(
        player
            .actions()
            .iter()
            .map(|action| format!("player: {action}")),
    );

    Ok(InstallerReport::new(app_root, options.dry_run, actions))
}
