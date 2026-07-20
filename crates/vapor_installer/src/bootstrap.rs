//! Default player-mode install operations.
//!
//! Player mode is the minimum app-local preparation required for ordinary
//! closed-alpha use: SteamCMD and generated disposable app-root state. It
//! deliberately does not install Rust/Cargo or Git.

use crate::{
    acquire::{download, downloads_dir, extract_zip},
    app_root::resolve_app_root,
    fsutil::{
        Logger, ensure_contained, is_executable, relative_label, remove_empty_dir, remove_path,
        reset_directory, write_receipt,
    },
    model::{ComponentStatus, InstallerOptions, InstallerReport, PlayerStatus},
    paths::{basic_directories, steam_candidates, steam_executable},
};
use std::{fs, path::Path, process::Command};

const STEAMCMD_LINUX: &str =
    "https://steamcdn-a.akamaihd.net/client/installer/steamcmd_linux.tar.gz";
const STEAMCMD_WINDOWS: &str = "https://steamcdn-a.akamaihd.net/client/installer/steamcmd.zip";

/// Inspect player-mode readiness.
///
/// # Errors
///
/// Fails when no valid app root can be resolved.
pub fn player_status(options: &InstallerOptions) -> Result<PlayerStatus, String> {
    let app_root = resolve_app_root(options.app_root.as_deref())?;
    Ok(inspect_player_root(&app_root))
}

/// Install or reconcile default player-mode tooling.
///
/// # Errors
///
/// Fails when acquisition, extraction, verification, or path confinement fails.
pub fn player_install(options: &InstallerOptions) -> Result<InstallerReport, String> {
    let app_root = resolve_app_root(options.app_root.as_deref())?;
    let mut actions = player_install_actions(&app_root);
    if options.dry_run {
        return Ok(InstallerReport::new(app_root, true, actions));
    }

    let mut logger = Logger::open(&app_root)?;
    logger.log("player install started");
    create_basic_directories(&app_root, &mut actions, Some(&mut logger))?;
    let before = inspect_player_root(&app_root);
    if !before.steamcmd().ready() {
        bootstrap_steamcmd(&app_root, &mut logger)?;
        actions.push("installed app-local SteamCMD".to_owned());
    } else {
        actions.push("kept existing app-local SteamCMD".to_owned());
    }
    write_receipt(&app_root, "player", "ready")?;
    logger.log("player install finished");

    let after = inspect_player_root(&app_root);
    if !after.ready() {
        logger.log("player-mode verification failed");
        return Err(format_player_missing(&after));
    }
    Ok(InstallerReport::new(app_root, false, actions))
}

/// Uninstall default player-mode tooling and generated disposable app-root state.
///
/// # Errors
///
/// Fails when a target path escapes the app root or removal fails.
pub fn player_uninstall(options: &InstallerOptions) -> Result<InstallerReport, String> {
    let app_root = resolve_app_root(options.app_root.as_deref())?;
    let paths = player_uninstall_paths(&app_root);
    let empty_dirs = player_empty_parent_dirs(&app_root);

    let mut actions = Vec::new();
    for path in &paths {
        ensure_contained(&app_root, path)?;
        actions.push(format!(
            "{} {}",
            if path.exists() {
                "remove"
            } else {
                "skip absent"
            },
            relative_label(&app_root, path)
        ));
    }
    for path in &empty_dirs {
        ensure_contained(&app_root, path)?;
        actions.push(format!(
            "{} {}",
            if path.exists() {
                "remove if empty"
            } else {
                "skip absent"
            },
            relative_label(&app_root, path)
        ));
    }
    if options.dry_run {
        return Ok(InstallerReport::new(app_root, true, actions));
    }

    let mut logger = Logger::open(&app_root)?;
    logger.log("player uninstall started");
    for action in &actions {
        logger.log(format!("planned {action}"));
    }
    logger.log("player uninstall removing generated app-root state");
    drop(logger);

    for path in &paths {
        remove_path(&app_root, path)?;
    }
    for path in &empty_dirs {
        remove_empty_dir(&app_root, path)?;
    }
    Ok(InstallerReport::new(app_root, false, actions))
}

fn player_uninstall_paths(app_root: &Path) -> Vec<std::path::PathBuf> {
    vec![
        app_root.join("tools/git"),
        app_root.join("tools/steamcmd"),
        app_root.join(".vapor/registry"),
        app_root.join(".vapor/downloads"),
        app_root.join(".vapor/extract"),
        app_root.join(".vapor/state"),
        app_root.join(".vapor/diagnostics"),
        app_root.join(".vapor/logs"),
        app_root.join("content/cache"),
        app_root.join("content/installed"),
        app_root.join("content/workshop/downloads"),
        app_root.join("output"),
    ]
}

fn player_empty_parent_dirs(app_root: &Path) -> Vec<std::path::PathBuf> {
    vec![
        app_root.join("content/workshop"),
        app_root.join("content"),
        app_root.join("tools"),
        app_root.join(".vapor"),
    ]
}

pub(crate) fn player_install_actions(app_root: &Path) -> Vec<String> {
    let mut actions = basic_directories()
        .iter()
        .map(|relative| format!("ensure directory {}", app_root.join(relative).display()))
        .collect::<Vec<_>>();
    let status = inspect_player_root(app_root);
    actions.push(format!(
        "{} app-local SteamCMD at {}",
        if status.steamcmd().ready() {
            "keep"
        } else {
            "install"
        },
        status.steamcmd().path().display()
    ));
    actions
}

pub(crate) fn inspect_player_root(app_root: &Path) -> PlayerStatus {
    let steamcmd_path = steam_executable(app_root);
    let steamcmd_ready = steam_candidates(&app_root.join("tools/steamcmd"))
        .iter()
        .any(|path| is_executable(path));
    let steamcmd = ComponentStatus::new(
        "SteamCMD",
        steamcmd_ready,
        steamcmd_path,
        if steamcmd_ready {
            Vec::new()
        } else {
            vec!["app-local steamcmd executable".to_owned()]
        },
    );

    let missing = basic_directories()
        .into_iter()
        .filter(|relative| !app_root.join(relative).is_dir())
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .collect::<Vec<_>>();
    let directories = ComponentStatus::new(
        "Generated directories",
        missing.is_empty(),
        app_root.join(".vapor"),
        missing,
    );

    PlayerStatus::new(app_root.to_path_buf(), steamcmd, directories)
}

fn create_basic_directories(
    app_root: &Path,
    actions: &mut Vec<String>,
    mut logger: Option<&mut Logger>,
) -> Result<(), String> {
    for relative in basic_directories() {
        let path = app_root.join(&relative);
        ensure_contained(app_root, &path)?;
        fs::create_dir_all(&path)
            .map_err(|error| format!("failed to create '{}': {error}", path.display()))?;
        if let Some(logger) = &mut logger {
            logger.log(format!("ensured directory {}", relative.display()));
        }
        actions.push(format!("ensured directory {}", relative.display()));
    }
    Ok(())
}

fn bootstrap_steamcmd(app_root: &Path, logger: &mut Logger) -> Result<(), String> {
    logger.log("installing SteamCMD");
    let target = app_root.join("tools/steamcmd");
    reset_directory(app_root, &target)?;
    if cfg!(target_os = "windows") {
        let archive = downloads_dir(app_root)?.join("steamcmd.zip");
        download(STEAMCMD_WINDOWS, &archive, logger)?;
        return extract_zip(&archive, &target, "SteamCMD archive", logger);
    }
    let archive = downloads_dir(app_root)?.join("steamcmd_linux.tar.gz");
    download(STEAMCMD_LINUX, &archive, logger)?;
    let mut tar = Command::new("tar");
    tar.args(["-xzf"]).arg(&archive).arg("-C").arg(&target);
    logger.attach_command_output(&mut tar);
    let status = tar
        .status()
        .map_err(|error| format!("failed to start tar for SteamCMD archive: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("SteamCMD archive extraction exited with {status}"))
    }
}

fn format_player_missing(status: &PlayerStatus) -> String {
    format!(
        "player-mode install verification failed\n{}\n{}",
        format_component_missing(status.steamcmd()),
        format_component_missing(status.directories())
    )
}

fn format_component_missing(status: &ComponentStatus) -> String {
    if status.ready() {
        return format!("  - {}: ready", status.label());
    }
    format!(
        "  - {}: missing {} (primary path {})",
        status.label(),
        status.missing().join(", "),
        status.path().display()
    )
}
