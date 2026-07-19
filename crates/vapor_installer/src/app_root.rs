//! App-root discovery and manifest role checks.
//!
//! Installer operations are always scoped to a directory whose
//! `App.vapor.toml` declares `[root]`. This keeps destructive uninstall paths
//! confined to the Steam application root and prevents accidental cleanup of
//! source checkouts.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

pub(crate) const APP_MANIFEST: &str = "App.vapor.toml";
pub(crate) const REGISTRY_MANIFEST: &str = "Registry.vapor.toml";

/// Resolve a Vapor Steam app root from an explicit path, the running binary, or
/// the current directory.
///
/// # Errors
///
/// Returns all rejected candidates when no checked directory is a `[root]`
/// Vapor app root.
pub(crate) fn resolve_app_root(explicit: Option<&Path>) -> Result<PathBuf, String> {
    let candidates = if let Some(path) = explicit {
        vec![path.to_path_buf()]
    } else {
        default_app_root_candidates()
    };
    let mut rejected = Vec::new();
    for candidate in candidates {
        match fs::canonicalize(&candidate) {
            Ok(path) if path.is_dir() => {
                let marker = path.join(APP_MANIFEST);
                if marker.is_file() && manifest_declares_root(&marker)? {
                    return Ok(path);
                }
                rejected.push(format!(
                    "{} (missing root {APP_MANIFEST})",
                    candidate.display()
                ));
            }
            Ok(path) => rejected.push(format!("{} (not a directory)", path.display())),
            Err(error) => rejected.push(format!("{} ({error})", candidate.display())),
        }
    }
    Err(format!(
        "could not resolve a Vapor Steam app root\nchecked:\n  - {}\nhelp: pass --app-root /path/to/steam/app",
        rejected.join("\n  - ")
    ))
}

pub(crate) fn manifest_declares_root(path: &Path) -> Result<bool, String> {
    manifest_declares_table(path, "root")
}

pub(crate) fn manifest_declares_registry(path: &Path) -> Result<bool, String> {
    manifest_declares_table(path, "registry")
}

fn default_app_root_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(executable) = env::current_exe()
        && let Some(candidate) = candidate_app_root_from_executable(&executable)
    {
        candidates.push(candidate);
    }
    if let Ok(current) = env::current_dir() {
        candidates.push(current);
    }
    candidates
}

fn candidate_app_root_from_executable(executable: &Path) -> Option<PathBuf> {
    let directory = executable.parent()?;
    if directory.file_name().is_some_and(|name| name == "bin") {
        return directory.parent().map(Path::to_path_buf);
    }
    if directory
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "bin")
    {
        return directory
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
    }
    None
}

fn manifest_declares_table(path: &Path, expected: &str) -> Result<bool, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    Ok(source.lines().any(|line| {
        let line = line.trim();
        let Some(table) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        else {
            return false;
        };
        !table.starts_with('[') && table.trim() == expected
    }))
}
