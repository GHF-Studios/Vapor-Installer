//! Constrained filesystem helpers.
//!
//! Every mutation helper takes the resolved app root and verifies that the
//! target stays underneath it. Symlink removal removes the link itself, not the
//! external target.

use crate::model::INSTALLER_LOG;
use std::{
    fs,
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) fn ensure_contained(root: &Path, candidate: &Path) -> Result<(), String> {
    if candidate.starts_with(root) {
        Ok(())
    } else {
        Err(format!(
            "installer boundary violation: '{}' is outside '{}'",
            candidate.display(),
            root.display()
        ))
    }
}

pub(crate) fn relative_label(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub(crate) fn executable(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

pub(crate) fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub(crate) fn is_healthy_executable(path: &Path, app_root: &Path) -> bool {
    is_executable(path)
        && Command::new(path)
            .arg("--version")
            .env("VAPOR_HOME", app_root)
            .env("CARGO_HOME", app_root.join("cargo-home"))
            .env("RUSTUP_HOME", app_root.join("rustup-home"))
            .output()
            .is_ok_and(|output| output.status.success())
}

pub(crate) fn make_executable(path: &Path) -> Result<(), String> {
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .map_err(|error| format!("failed to inspect '{}': {error}", path.display()))?
            .permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        fs::set_permissions(path, permissions)
            .map_err(|error| format!("failed to make '{}' executable: {error}", path.display()))?;
    }
    Ok(())
}

pub(crate) fn reset_directory(app_root: &Path, target: &Path) -> Result<(), String> {
    ensure_contained(app_root, target)?;
    remove_path(app_root, target)?;
    fs::create_dir_all(target)
        .map_err(|error| format!("failed to create '{}': {error}", target.display()))
}

pub(crate) fn remove_path(app_root: &Path, target: &Path) -> Result<(), String> {
    ensure_contained(app_root, target)?;
    let Ok(metadata) = fs::symlink_metadata(target) else {
        return Ok(());
    };
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(target)
            .map_err(|error| format!("failed to remove '{}': {error}", target.display()))
    } else {
        fs::remove_file(target)
            .map_err(|error| format!("failed to remove '{}': {error}", target.display()))
    }
}

pub(crate) fn remove_empty_dir(app_root: &Path, target: &Path) -> Result<(), String> {
    ensure_contained(app_root, target)?;
    let Ok(metadata) = fs::symlink_metadata(target) else {
        return Ok(());
    };
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Ok(());
    }
    match fs::remove_dir(target) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(format!(
            "failed to remove empty directory '{}': {error}",
            target.display()
        )),
    }
}

pub(crate) fn write_receipt(app_root: &Path, name: &str, status: &str) -> Result<(), String> {
    let path = app_root
        .join(".vapor/state/installer")
        .join(format!("{name}.toml"));
    ensure_contained(app_root, &path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
    }
    let source = format!(
        "schema = 1\nstatus = \"{status}\"\nupdated_at = \"{}\"\n",
        timestamp()
    );
    fs::write(&path, source)
        .map_err(|error| format!("failed to write '{}': {error}", path.display()))
}

pub(crate) fn copy_external_file(
    app_root: &Path,
    source: &Path,
    target: &Path,
) -> Result<(), String> {
    ensure_contained(app_root, target)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
    }
    fs::copy(source, target).map_err(|error| {
        format!(
            "failed to copy host file '{}' to '{}': {error}",
            source.display(),
            target.display()
        )
    })?;
    make_executable(target)
}

pub(crate) fn copy_external_tree(
    app_root: &Path,
    source: &Path,
    destination: &Path,
) -> Result<(), String> {
    let canonical = fs::canonicalize(source).map_err(|error| {
        format!(
            "failed to resolve host Git path '{}': {error}",
            source.display()
        )
    })?;
    copy_external_tree_entry(app_root, &canonical, destination)
}

pub(crate) fn copy_app_tree(
    app_root: &Path,
    source: &Path,
    destination: &Path,
) -> Result<(), String> {
    ensure_contained(app_root, source)?;
    ensure_contained(app_root, destination)?;
    copy_app_tree_entry(source, destination, source)
}

pub(crate) fn copy_app_file(app_root: &Path, source: &Path, target: &Path) -> Result<(), String> {
    ensure_contained(app_root, source)?;
    ensure_contained(app_root, target)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
    }
    fs::copy(source, target).map_err(|error| {
        format!(
            "failed to copy '{}' to '{}': {error}",
            source.display(),
            target.display()
        )
    })?;
    make_executable(target)
}

pub(crate) fn path_is_inside(path: &Path, root: &Path) -> bool {
    fs::canonicalize(root).is_ok_and(|root| path.starts_with(root))
}

pub(crate) struct Logger {
    file: File,
}

impl Logger {
    pub(crate) fn open(app_root: &Path) -> Result<Self, String> {
        let path = app_root.join(INSTALLER_LOG);
        ensure_contained(app_root, &path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                format!("failed to open installer log '{}': {error}", path.display())
            })?;
        Ok(Self { file })
    }

    pub(crate) fn log(&mut self, message: impl AsRef<str>) {
        let _ = writeln!(self.file, "[{}] {}", timestamp(), message.as_ref());
        let _ = self.file.flush();
    }
}

pub(crate) fn timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:09}Z", duration.as_secs(), duration.subsec_nanos())
}

fn copy_external_tree_entry(
    app_root: &Path,
    source: &Path,
    destination: &Path,
) -> Result<(), String> {
    ensure_contained(app_root, destination)?;
    let metadata = fs::metadata(source).map_err(|error| {
        format!(
            "failed to inspect host Git path '{}': {error}",
            source.display()
        )
    })?;
    if metadata.is_dir() {
        fs::create_dir_all(destination)
            .map_err(|error| format!("failed to create '{}': {error}", destination.display()))?;
        for entry in fs::read_dir(source).map_err(|error| {
            format!(
                "failed to read host Git path '{}': {error}",
                source.display()
            )
        })? {
            let entry = entry.map_err(|error| format!("failed to read host Git entry: {error}"))?;
            copy_external_tree_entry(
                app_root,
                &entry.path(),
                &destination.join(entry.file_name()),
            )?;
        }
    } else if metadata.is_file() {
        copy_external_file(app_root, source, destination)?;
    }
    Ok(())
}

fn copy_app_tree_entry(source: &Path, destination: &Path, item_root: &Path) -> Result<(), String> {
    let canonical = fs::canonicalize(source)
        .map_err(|error| format!("failed to resolve '{}': {error}", source.display()))?;
    ensure_contained(item_root, &canonical)?;
    let metadata = fs::metadata(&canonical)
        .map_err(|error| format!("failed to inspect '{}': {error}", canonical.display()))?;
    if metadata.is_dir() {
        fs::create_dir_all(destination)
            .map_err(|error| format!("failed to create '{}': {error}", destination.display()))?;
        for entry in fs::read_dir(&canonical)
            .map_err(|error| format!("failed to read '{}': {error}", canonical.display()))?
        {
            let entry =
                entry.map_err(|error| format!("failed to read directory entry: {error}"))?;
            copy_app_tree_entry(
                &entry.path(),
                &destination.join(entry.file_name()),
                item_root,
            )?;
        }
    } else if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
        }
        fs::copy(&canonical, destination).map_err(|error| {
            format!(
                "failed to copy '{}' to '{}': {error}",
                canonical.display(),
                destination.display()
            )
        })?;
    }
    Ok(())
}
