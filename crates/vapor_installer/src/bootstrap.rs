//! Default player-mode install operations.
//!
//! Player mode is the minimum app-local preparation required for ordinary
//! closed-alpha use: Git, SteamCMD, the public registry checkout, and generated
//! disposable app-root state. It deliberately does not install Rust/Cargo.

use crate::{
    acquire::{download, downloads_dir, extract_zip},
    app_root::resolve_app_root,
    fsutil::{
        Logger, ensure_contained, is_executable, relative_label, remove_empty_dir, remove_path,
        reset_directory, write_receipt,
    },
    git::{bootstrap_git, git_executable, git_status},
    model::{ComponentStatus, InstallerOptions, InstallerReport, PlayerStatus},
    paths::{
        basic_directories, is_registry_checkout, registry_path, steam_candidates, steam_executable,
    },
};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

const STEAMCMD_LINUX: &str =
    "https://steamcdn-a.akamaihd.net/client/installer/steamcmd_linux.tar.gz";
const STEAMCMD_WINDOWS: &str = "https://steamcdn-a.akamaihd.net/client/installer/steamcmd.zip";
const VAPOR_REGISTRY_URL: &str = "https://github.com/GHF-Studios/Vapor-Registry.git";

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
    if !before.git().ready() {
        bootstrap_git(&app_root, &mut logger)?;
        actions.push("installed app-local Git".to_owned());
    } else {
        actions.push("kept existing app-local Git".to_owned());
    }
    if !before.steamcmd().ready() {
        bootstrap_steamcmd(&app_root, &mut logger)?;
        actions.push("installed app-local SteamCMD".to_owned());
    } else {
        actions.push("kept existing app-local SteamCMD".to_owned());
    }
    if !inspect_player_root(&app_root).registry().ready() {
        bootstrap_registry(&app_root, &mut logger)?;
        actions.push("checked out app-local Vapor-Registry".to_owned());
    } else {
        update_registry_checkout(&app_root, &mut logger)?;
        actions.push("updated app-local Vapor-Registry checkout".to_owned());
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
        "{} app-local Git at {}",
        if status.git().ready() {
            "keep"
        } else {
            "install"
        },
        status.git().path().display()
    ));
    actions.push(format!(
        "{} app-local SteamCMD at {}",
        if status.steamcmd().ready() {
            "keep"
        } else {
            "install"
        },
        status.steamcmd().path().display()
    ));
    actions.push(format!(
        "{} app-local Vapor-Registry checkout at {}",
        if status.registry().ready() {
            "update"
        } else {
            "checkout"
        },
        status.registry().path().display()
    ));
    actions
}

pub(crate) fn inspect_player_root(app_root: &Path) -> PlayerStatus {
    let (git_ready, git_path) = git_status(app_root);
    let git = ComponentStatus::new(
        "Git",
        git_ready,
        git_path,
        if git_ready {
            Vec::new()
        } else {
            vec!["app-local git executable".to_owned()]
        },
    );

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

    let registry_path = registry_path(app_root);
    let registry_ready = is_registry_checkout(&registry_path);
    let registry = ComponentStatus::new(
        "Vapor-Registry",
        registry_ready,
        registry_path,
        if registry_ready {
            Vec::new()
        } else {
            vec!["app-local Vapor-Registry Git checkout".to_owned()]
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

    PlayerStatus::new(app_root.to_path_buf(), git, steamcmd, registry, directories)
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
    let status = Command::new("tar")
        .args(["-xzf"])
        .arg(&archive)
        .arg("-C")
        .arg(&target)
        .status()
        .map_err(|error| format!("failed to start tar for SteamCMD archive: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("SteamCMD archive extraction exited with {status}"))
    }
}

fn bootstrap_registry(app_root: &Path, logger: &mut Logger) -> Result<(), String> {
    logger.log("checking out Vapor-Registry");
    let target = registry_path(app_root);
    ensure_contained(app_root, &target)?;
    remove_path(app_root, &target)?;
    let git = git_executable(app_root)?;
    let mut command = Command::new(&git);
    command
        .args(["clone", "--quiet", VAPOR_REGISTRY_URL])
        .arg(&target)
        .env("VAPOR_HOME", app_root);
    run_quiet_command(command, "Vapor-Registry clone")?;
    if is_registry_checkout(&target) {
        Ok(())
    } else {
        Err(format!(
            "Vapor-Registry checkout at '{}' did not contain a valid Registry.vapor.toml",
            target.display()
        ))
    }
}

fn update_registry_checkout(app_root: &Path, logger: &mut Logger) -> Result<(), String> {
    let target = registry_path(app_root);
    ensure_contained(app_root, &target)?;
    if !is_registry_checkout(&target) {
        return bootstrap_registry(app_root, logger);
    }
    if !registry_worktree_clean(app_root, &target)? {
        logger.log("skipped Vapor-Registry update because local changes are present");
        return Ok(());
    }
    logger.log("updating Vapor-Registry");
    let git = git_executable(app_root)?;
    let mut command = Command::new(&git);
    command
        .current_dir(&target)
        .args(["pull", "--ff-only"])
        .env("VAPOR_HOME", app_root);
    run_quiet_command(command, "Vapor-Registry update")
}

fn registry_worktree_clean(app_root: &Path, target: &Path) -> Result<bool, String> {
    let git = git_executable(app_root)?;
    let output = Command::new(&git)
        .current_dir(target)
        .args(["status", "--porcelain"])
        .env("VAPOR_HOME", app_root)
        .output()
        .map_err(|error| {
            format!(
                "failed to start Git status in Vapor-Registry '{}': {error}",
                target.display()
            )
        })?;
    if output.status.success() {
        Ok(output.stdout.is_empty())
    } else {
        Err(format!(
            "Vapor-Registry status check exited with {}",
            output.status
        ))
    }
}

fn run_quiet_command(mut command: Command, label: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("failed to start {label}: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{label} exited with {}\n{}",
        output.status,
        captured_output(&output)
    ))
}

fn captured_output(output: &Output) -> String {
    let mut detail = String::new();
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.trim().is_empty() {
        detail.push_str("stdout:\n");
        detail.push_str(stdout.trim());
        detail.push('\n');
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        detail.push_str("stderr:\n");
        detail.push_str(stderr.trim());
        detail.push('\n');
    }
    if detail.is_empty() {
        "no command output captured".to_owned()
    } else {
        detail
    }
}

fn format_player_missing(status: &PlayerStatus) -> String {
    format!(
        "player-mode install verification failed\n{}\n{}\n{}\n{}",
        format_component_missing(status.git()),
        format_component_missing(status.steamcmd()),
        format_component_missing(status.registry()),
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
