//! Explicit development-environment operations.
//!
//! This module owns Rust/Cargo and cross-build tooling. These operations are
//! intentionally separate from runtime bootstrap so ordinary closed-alpha
//! installs do not download a compiler toolchain.

use crate::{
    acquire::{
        download, downloads_dir, extract_tar_xz, extract_zip, verify_sha256_with_powershell,
        verify_sha256_with_sha256sum,
    },
    app_root::resolve_app_root,
    bootstrap::{bootstrap_install, bootstrap_install_actions},
    fsutil::{
        Logger, copy_app_file, copy_app_tree, ensure_contained, executable, is_executable,
        is_healthy_executable, make_executable, relative_label, remove_path, reset_directory,
        write_receipt,
    },
    model::{ComponentStatus, DevEnvStatus, InstallerOptions, InstallerReport},
    paths::{cross_linker_path, llvm_mingw_bin, llvm_mingw_root, zig_executable},
};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

const RUSTUP_INIT_X86_64_LINUX: &str =
    "https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/rustup-init";
const RUSTUP_INIT_AARCH64_LINUX: &str =
    "https://static.rust-lang.org/rustup/dist/aarch64-unknown-linux-gnu/rustup-init";
const RUSTUP_INIT_X86_64_WINDOWS: &str =
    "https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-gnu/rustup-init.exe";
const ZIG_VERSION: &str = "0.16.0";
const ZIG_X86_64_LINUX: &str = "https://ziglang.org/download/0.16.0/zig-x86_64-linux-0.16.0.tar.xz";
const ZIG_X86_64_WINDOWS: &str =
    "https://ziglang.org/download/0.16.0/zig-x86_64-windows-0.16.0.zip";
const LLVM_MINGW_VERSION: &str = "20260616";
const LLVM_MINGW_X86_64_LINUX: &str = "https://github.com/mstorsjo/llvm-mingw/releases/download/20260616/llvm-mingw-20260616-msvcrt-ubuntu-22.04-x86_64.tar.xz";
const LLVM_MINGW_X86_64_LINUX_SHA256: &str =
    "a1f7968b48ba8d949194d6dee6c76f3cd0f61cba91658599af2c2c834a55ab87";
const LLVM_MINGW_X86_64_WINDOWS: &str = "https://github.com/mstorsjo/llvm-mingw/releases/download/20260616/llvm-mingw-20260616-msvcrt-x86_64.zip";
const LLVM_MINGW_X86_64_WINDOWS_SHA256: &str =
    "744809646fdefe24a357399788d68fb07ecc65fa0be71baa2406793ce25c9813";

/// Inspect explicit development environment readiness.
///
/// # Errors
///
/// Fails when no valid app root can be resolved.
pub fn dev_env_status(options: &InstallerOptions) -> Result<DevEnvStatus, String> {
    let app_root = resolve_app_root(options.app_root.as_deref())?;
    Ok(inspect_dev_env_root(&app_root))
}

/// Install or reconcile explicit development tooling.
///
/// # Errors
///
/// Fails when acquisition, extraction, verification, or path confinement fails.
pub fn dev_env_install(options: &InstallerOptions) -> Result<InstallerReport, String> {
    let app_root = resolve_app_root(options.app_root.as_deref())?;
    let mut actions = dev_env_install_actions(&app_root);
    if options.dry_run {
        actions.extend(bootstrap_install_actions(&app_root));
        return Ok(InstallerReport::new(app_root, true, actions));
    }

    bootstrap_install(&InstallerOptions {
        app_root: Some(app_root.clone()),
        dry_run: false,
    })?;

    let mut logger = Logger::open(&app_root)?;
    logger.log("dev-env install started");
    let before = inspect_dev_env_root(&app_root);
    if !before.rust().ready() {
        bootstrap_rust(&app_root, &mut logger)?;
        actions.push("installed Rust/Cargo development toolchain".to_owned());
    } else {
        install_rust_targets_and_components(&app_root, &mut logger)?;
        actions.push("kept existing Rust/Cargo development toolchain".to_owned());
    }
    if !before.cross().ready() {
        bootstrap_cross_toolchains(&app_root, &mut logger)?;
        actions.push("installed cross-build development tools".to_owned());
    } else {
        actions.push("kept existing cross-build development tools".to_owned());
    }
    write_receipt(&app_root, "dev-env", "ready")?;
    logger.log("dev-env install finished");

    let after = inspect_dev_env_root(&app_root);
    if !after.ready() {
        logger.log("dev-env verification failed");
        return Err(format_dev_env_missing(&after));
    }
    Ok(InstallerReport::new(app_root, false, actions))
}

/// Uninstall explicit development tooling.
///
/// # Errors
///
/// Fails when a target path escapes the app root or removal fails.
pub fn dev_env_uninstall(options: &InstallerOptions) -> Result<InstallerReport, String> {
    let app_root = resolve_app_root(options.app_root.as_deref())?;
    let paths = [
        app_root.join("rustup"),
        app_root.join("rustup-home"),
        app_root.join("cargo-home"),
        app_root.join("tools/zig"),
        app_root.join("tools/llvm-mingw"),
        app_root.join("tools/cross"),
        app_root.join(".vapor/state/installer/dev-env.toml"),
    ];
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
    if options.dry_run {
        return Ok(InstallerReport::new(app_root, true, actions));
    }

    let mut logger = Logger::open(&app_root)?;
    logger.log("dev-env uninstall started");
    for path in &paths {
        remove_path(&app_root, path)?;
        logger.log(format!("removed {}", relative_label(&app_root, path)));
    }
    logger.log("dev-env uninstall finished");
    Ok(InstallerReport::new(app_root, false, actions))
}

fn dev_env_install_actions(app_root: &Path) -> Vec<String> {
    let status = inspect_dev_env_root(app_root);
    vec![
        format!(
            "{} Rust/Cargo development toolchain at {}",
            if status.rust().ready() {
                "keep"
            } else {
                "install"
            },
            status.rust().path().display()
        ),
        format!(
            "{} cross-build development tools at {}",
            if status.cross().ready() {
                "keep"
            } else {
                "install"
            },
            status.cross().path().display()
        ),
    ]
}

pub(crate) fn inspect_dev_env_root(app_root: &Path) -> DevEnvStatus {
    let rustup = app_root.join("rustup/bin").join(executable("rustup"));
    let toolchains = app_root.join("rustup-home/toolchains");
    let (rust_bin, rust_missing) = inspect_rust(&toolchains, Some(app_root));
    let mut missing = rust_missing;
    if !is_executable(&rustup) {
        missing.push(format!("rustup (expected at {})", rustup.display()));
    }
    let rust = ComponentStatus::new(
        "Rust/Cargo",
        missing.is_empty(),
        rust_bin.unwrap_or(toolchains),
        missing,
    );

    let cross = inspect_cross_tools(app_root);
    DevEnvStatus::new(app_root.to_path_buf(), rust, cross)
}

fn bootstrap_rust(app_root: &Path, logger: &mut Logger) -> Result<(), String> {
    logger.log("installing Rust/Cargo");
    let downloads = downloads_dir(app_root)?;
    let rustup_init = downloads.join(executable("rustup-init"));
    download(rustup_init_url()?, &rustup_init, logger)?;
    make_executable(&rustup_init)?;
    let toolchain = vapor_core::canonical_toolchain()
        .map_err(|error| format!("failed to resolve canonical Vapor toolchain: {error}"))?;
    let default_host = vapor_core::current_host_triple();
    if !toolchain.supports_host(default_host) {
        return Err(format!(
            "development toolchain installation is not configured for host {default_host}"
        ));
    }
    let toolchain_id = toolchain.identifier();
    let status = Command::new(&rustup_init)
        .args([
            "-y",
            "--no-modify-path",
            "--profile",
            "default",
            "--default-toolchain",
            &toolchain_id,
            "--default-host",
            default_host,
        ])
        .env("RUSTUP_HOME", app_root.join("rustup-home"))
        .env("CARGO_HOME", app_root.join("cargo-home"))
        .status()
        .map_err(|error| format!("failed to start rustup-init: {error}"))?;
    if !status.success() {
        return Err(format!("rustup-init exited with {status}"));
    }
    let source = app_root.join("cargo-home/bin").join(executable("rustup"));
    let target = app_root.join("rustup/bin").join(executable("rustup"));
    copy_app_file(app_root, &source, &target)?;
    install_rust_targets_and_components(app_root, logger)?;
    Ok(())
}

fn install_rust_targets_and_components(app_root: &Path, logger: &mut Logger) -> Result<(), String> {
    let rustup = app_root.join("rustup/bin").join(executable("rustup"));
    for target in vapor_core::SUPPORTED_TARGET_TRIPLES {
        logger.log(format!("installing Rust target {target}"));
        let status = Command::new(&rustup)
            .args(["target", "add", target])
            .env("RUSTUP_HOME", app_root.join("rustup-home"))
            .env("CARGO_HOME", app_root.join("cargo-home"))
            .status()
            .map_err(|error| format!("failed to start rustup target add {target}: {error}"))?;
        if !status.success() {
            return Err(format!("rustup target add {target} exited with {status}"));
        }
    }
    for component in ["rustfmt", "clippy", "rust-src"] {
        logger.log(format!("installing Rust component {component}"));
        let status = Command::new(&rustup)
            .args(["component", "add", component])
            .env("RUSTUP_HOME", app_root.join("rustup-home"))
            .env("CARGO_HOME", app_root.join("cargo-home"))
            .status()
            .map_err(|error| {
                format!("failed to start rustup component add {component}: {error}")
            })?;
        if !status.success() {
            return Err(format!(
                "rustup component add {component} exited with {status}"
            ));
        }
    }
    Ok(())
}

fn bootstrap_cross_toolchains(app_root: &Path, logger: &mut Logger) -> Result<(), String> {
    if !is_executable(&zig_executable(app_root)) {
        bootstrap_zig(app_root, logger)?;
    }
    if !is_executable(&llvm_mingw_bin(app_root).join(executable("x86_64-w64-mingw32-clang"))) {
        bootstrap_llvm_mingw(app_root, logger)?;
    }
    write_cross_wrappers(app_root)
}

fn bootstrap_zig(app_root: &Path, logger: &mut Logger) -> Result<(), String> {
    logger.log("installing Zig");
    let target = app_root.join("tools/zig");
    let extract_root = app_root.join(".vapor/extract/zig");
    reset_directory(app_root, &target)?;
    reset_directory(app_root, &extract_root)?;
    if cfg!(target_os = "windows") {
        let archive =
            downloads_dir(app_root)?.join(format!("zig-x86_64-windows-{ZIG_VERSION}.zip"));
        download(ZIG_X86_64_WINDOWS, &archive, logger)?;
        extract_zip(&archive, &extract_root, "Zig archive", logger)?;
    } else {
        let archive =
            downloads_dir(app_root)?.join(format!("zig-x86_64-linux-{ZIG_VERSION}.tar.xz"));
        download(ZIG_X86_64_LINUX, &archive, logger)?;
        extract_tar_xz(&archive, &extract_root, "Zig archive")?;
    }
    let extracted =
        find_extracted_tool_root(&extract_root, PathBuf::from(executable("zig")), "Zig")?;
    copy_app_tree(app_root, &extracted, &target)?;
    make_executable(&zig_executable(app_root))?;
    Ok(())
}

fn bootstrap_llvm_mingw(app_root: &Path, logger: &mut Logger) -> Result<(), String> {
    logger.log("installing llvm-mingw");
    let target = llvm_mingw_root(app_root);
    let extract_root = app_root.join(".vapor/extract/llvm-mingw");
    reset_directory(app_root, &target)?;
    reset_directory(app_root, &extract_root)?;
    if cfg!(target_os = "windows") {
        let archive = downloads_dir(app_root)?
            .join(format!("llvm-mingw-{LLVM_MINGW_VERSION}-msvcrt-x86_64.zip"));
        download(LLVM_MINGW_X86_64_WINDOWS, &archive, logger)?;
        verify_sha256_with_powershell(&archive, LLVM_MINGW_X86_64_WINDOWS_SHA256)?;
        extract_zip(&archive, &extract_root, "llvm-mingw archive", logger)?;
    } else {
        let archive = downloads_dir(app_root)?.join(format!(
            "llvm-mingw-{LLVM_MINGW_VERSION}-msvcrt-ubuntu-22.04-x86_64.tar.xz"
        ));
        download(LLVM_MINGW_X86_64_LINUX, &archive, logger)?;
        verify_sha256_with_sha256sum(&archive, LLVM_MINGW_X86_64_LINUX_SHA256)?;
        extract_tar_xz(&archive, &extract_root, "llvm-mingw archive")?;
    }
    let extracted = find_extracted_tool_root(
        &extract_root,
        Path::new("bin").join(executable("x86_64-w64-mingw32-clang")),
        "llvm-mingw",
    )?;
    copy_app_tree(app_root, &extracted, &target)?;
    for tool in [
        "x86_64-w64-mingw32-clang",
        "x86_64-w64-mingw32-dlltool",
        "llvm-dlltool",
    ] {
        make_executable(&target.join("bin").join(executable(tool)))?;
    }
    Ok(())
}

fn inspect_rust(toolchains: &Path, app_root: Option<&Path>) -> (Option<PathBuf>, Vec<String>) {
    let required = ["cargo", "rustc", "rustfmt", "cargo-clippy", "rustdoc"];
    let mut candidates = fs::read_dir(toolchains)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path().join("bin"))
        .collect::<Vec<_>>();
    candidates.sort();
    for bin in &candidates {
        let missing = required
            .iter()
            .filter(|name| {
                let path = bin.join(executable(name));
                app_root.map_or_else(
                    || !is_executable(&path),
                    |root| !is_healthy_executable(&path, root),
                )
            })
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return (Some(bin.clone()), Vec::new());
        }
    }
    (
        candidates.into_iter().next(),
        required.iter().map(|name| (*name).to_owned()).collect(),
    )
}

fn inspect_cross_tools(app_root: &Path) -> ComponentStatus {
    let mut missing = Vec::new();
    let zig = zig_executable(app_root);
    if !is_executable(&zig) {
        missing.push(format!("zig (expected at {})", zig.display()));
    }
    for tool in [
        "x86_64-w64-mingw32-clang",
        "x86_64-w64-mingw32-dlltool",
        "llvm-dlltool",
    ] {
        let path = llvm_mingw_bin(app_root).join(executable(tool));
        if !is_executable(&path) {
            missing.push(format!(
                "llvm-mingw tool {tool} (expected at {})",
                path.display()
            ));
        }
    }
    let target = "x86_64-unknown-linux-gnu";
    let wrapper = cross_linker_path(app_root, target);
    if !is_executable(&wrapper) {
        missing.push(format!(
            "{target} linker wrapper (expected at {})",
            wrapper.display()
        ));
    }
    ComponentStatus::new("Cross-build tools", missing.is_empty(), zig, missing)
}

fn write_cross_wrappers(app_root: &Path) -> Result<(), String> {
    write_cross_wrapper(app_root, "x86_64-unknown-linux-gnu", "x86_64-linux-gnu")
}

fn write_cross_wrapper(app_root: &Path, rust_target: &str, zig_target: &str) -> Result<(), String> {
    let path = cross_linker_path(app_root, rust_target);
    ensure_contained(app_root, &path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
    }
    let source = if cfg!(target_os = "windows") {
        format!(
            "@echo off\r\nset \"SELF_DIR=%~dp0\"\r\nset \"ZIG=%SELF_DIR%..\\..\\zig\\zig.exe\"\r\n\"%ZIG%\" cc -target {zig_target} %*\r\n"
        )
    } else {
        format!(
            "#!/bin/sh\nset -eu\nself_dir=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\nzig=\"$self_dir/../../zig/zig\"\nexec \"$zig\" cc -target {zig_target} \"$@\"\n"
        )
    };
    fs::write(&path, source)
        .map_err(|error| format!("failed to write '{}': {error}", path.display()))?;
    make_executable(&path)
}

fn find_extracted_tool_root(
    extract_root: &Path,
    required_relative: PathBuf,
    label: &str,
) -> Result<PathBuf, String> {
    let mut candidates = fs::read_dir(extract_root)
        .map_err(|error| format!("failed to read '{}': {error}", extract_root.display()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .filter(|path| path.join(&required_relative).is_file())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next().ok_or_else(|| {
        format!(
            "{label} archive did not contain an expected '{}' entry under '{}'",
            required_relative.display(),
            extract_root.display()
        )
    })
}

fn rustup_init_url() -> Result<&'static str, String> {
    match (env::consts::OS, env::consts::ARCH) {
        ("linux", "x86_64") => Ok(RUSTUP_INIT_X86_64_LINUX),
        ("linux", "aarch64") => Ok(RUSTUP_INIT_AARCH64_LINUX),
        ("windows", "x86_64") => Ok(RUSTUP_INIT_X86_64_WINDOWS),
        (os, arch) => Err(format!(
            "Rust/Cargo development installation is not configured for {arch}-{os}"
        )),
    }
}

fn format_dev_env_missing(status: &DevEnvStatus) -> String {
    format!(
        "dev-env verification failed\n{}\n{}",
        format_component_missing(status.rust()),
        format_component_missing(status.cross())
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
