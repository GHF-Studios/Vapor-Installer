//! App-local Git installation and health checks.
//!
//! Windows uses portable MinGit. Linux currently imports a real host Git binary
//! plus its exec-path/templates into the app root; delegating scripts are
//! rejected so runtime behavior does not depend on an external wrapper.

use crate::{
    acquire::{download, downloads_dir, extract_zip, verify_sha256_with_powershell},
    fsutil::{
        Logger, copy_external_file, copy_external_tree, ensure_contained, executable,
        is_executable, is_healthy_executable, make_executable, path_is_inside, reset_directory,
    },
    paths::{git_candidates, preferred_git_path},
};
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

const MINGIT_X86_64_WINDOWS: &str = "https://github.com/git-for-windows/git/releases/download/v2.55.0.windows.3/MinGit-2.55.0.3-64-bit.zip";
const MINGIT_X86_64_WINDOWS_SHA256: &str =
    "f48e2bead5cbc31f36c5808d67bdc1826965e22729391d3874656b4056e61ab5";

pub(crate) fn bootstrap_git(app_root: &Path, logger: &mut Logger) -> Result<(), String> {
    logger.log("installing Git");
    if cfg!(target_os = "windows") {
        return bootstrap_windows_mingit(app_root, logger);
    }
    bootstrap_host_git(app_root, logger)
}

pub(crate) fn git_executable(app_root: &Path) -> Result<PathBuf, String> {
    git_candidates(app_root)
        .into_iter()
        .find(|path| is_healthy_git(path, app_root))
        .ok_or_else(|| {
            format!(
                "app-local Git is not ready\nhelp: run `vapor-installer bootstrap install --app-root {}`",
                app_root.display()
            )
        })
}

pub(crate) fn git_status(app_root: &Path) -> (bool, PathBuf) {
    let candidates = git_candidates(app_root);
    let path = candidates
        .iter()
        .find(|path| is_healthy_git(path, app_root))
        .cloned()
        .unwrap_or_else(|| preferred_git_path(app_root));
    let ready = candidates.iter().any(|path| is_healthy_git(path, app_root));
    (ready, path)
}

fn bootstrap_windows_mingit(app_root: &Path, logger: &mut Logger) -> Result<(), String> {
    let archive = downloads_dir(app_root)?.join("MinGit-2.55.0.3-64-bit.zip");
    download(MINGIT_X86_64_WINDOWS, &archive, logger)?;
    verify_sha256_with_powershell(&archive, MINGIT_X86_64_WINDOWS_SHA256)?;
    let target = app_root.join("tools/git");
    reset_directory(app_root, &target)?;
    extract_zip(&archive, &target, "MinGit archive", logger)
}

fn bootstrap_host_git(app_root: &Path, logger: &mut Logger) -> Result<(), String> {
    let host = find_host_git(app_root)?;
    let target = app_root.join("tools/git");
    reset_directory(app_root, &target)?;
    let bin = target.join("bin");
    fs::create_dir_all(&bin)
        .map_err(|error| format!("failed to create '{}': {error}", bin.display()))?;

    if let Some(exec_path) = host.exec_path {
        copy_external_tree(app_root, &exec_path, &target.join("libexec/git-core"))?;
    }
    let app_git = target.join("libexec/git-core").join(executable("git"));
    if !is_executable(&app_git) {
        copy_external_file(app_root, &host.binary, &app_git)?;
    }
    if let Some(templates) = host.templates {
        copy_external_tree(
            app_root,
            &templates,
            &target.join("share/git-core/templates"),
        )?;
    }
    write_git_launcher(app_root, &target)?;
    logger.log(format!("installed Git from {}", host.binary.display()));
    Ok(())
}

#[derive(Debug)]
struct HostGit {
    binary: PathBuf,
    exec_path: Option<PathBuf>,
    templates: Option<PathBuf>,
}

fn find_host_git(app_root: &Path) -> Result<HostGit, String> {
    let mut candidates = host_git_candidates();
    candidates.sort();
    candidates.dedup();
    let mut inspected = BTreeSet::new();
    let mut rejected = Vec::new();
    for candidate in candidates {
        if !is_executable(&candidate) {
            continue;
        }
        let canonical = match fs::canonicalize(&candidate) {
            Ok(path) => path,
            Err(error) => {
                rejected.push(format!("{} ({error})", candidate.display()));
                continue;
            }
        };
        if !inspected.insert(canonical.clone()) || path_is_inside(&canonical, app_root) {
            continue;
        }
        if is_delegating_git_script(&canonical) {
            rejected.push(format!(
                "{} (delegates to another Git)",
                canonical.display()
            ));
            continue;
        }
        let version = Command::new(&canonical).arg("--version").output();
        if !version.is_ok_and(|output| output.status.success()) {
            rejected.push(format!("{} (`git --version` failed)", canonical.display()));
            continue;
        }
        let exec_path = git_stdout_path(&canonical, "--exec-path");
        return Ok(HostGit {
            binary: canonical,
            templates: git_template_path(exec_path.as_deref()),
            exec_path,
        });
    }
    let detail = if rejected.is_empty() {
        "no executable Git candidate was found on PATH or in common system locations".to_owned()
    } else {
        format!("rejected candidates:\n  - {}", rejected.join("\n  - "))
    };
    Err(format!(
        "cannot install Git: no usable host Git is available to import\n{detail}\nhelp: install Git with the operating-system package manager, then run `vapor-installer bootstrap install --app-root <app-root>`\nnote: Vapor imports a real Git binary into tools/git; it will not install a wrapper that delegates to system Git"
    ))
}

fn host_git_candidates() -> Vec<PathBuf> {
    let mut candidates = env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(executable("git")))
        .collect::<Vec<_>>();
    if cfg!(target_os = "linux") {
        candidates.extend([
            PathBuf::from("/usr/bin/git"),
            PathBuf::from("/usr/local/bin/git"),
            PathBuf::from("/bin/git"),
        ]);
    }
    candidates
}

fn git_stdout_path(git: &Path, arg: &str) -> Option<PathBuf> {
    let output = Command::new(git).arg(arg).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let value = stdout.trim();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

fn git_template_path(exec_path: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(exec_path) = exec_path
        && let Some(prefix) = exec_path.parent().and_then(Path::parent)
    {
        candidates.push(prefix.join("share/git-core/templates"));
    }
    candidates.extend([
        PathBuf::from("/usr/share/git-core/templates"),
        PathBuf::from("/usr/local/share/git-core/templates"),
    ]);
    candidates.into_iter().find(|path| path.is_dir())
}

fn write_git_launcher(app_root: &Path, git_root: &Path) -> Result<(), String> {
    let launcher = git_root.join("bin").join(executable("git"));
    ensure_contained(app_root, &launcher)?;
    let source = "#!/bin/sh\nset -eu\nself_dir=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\ngit_root=$(CDPATH= cd -- \"$self_dir/..\" && pwd)\nif [ -d \"$git_root/libexec/git-core\" ]; then\n    GIT_EXEC_PATH=\"$git_root/libexec/git-core\"\n    export GIT_EXEC_PATH\nfi\nif [ -d \"$git_root/share/git-core/templates\" ]; then\n    GIT_TEMPLATE_DIR=\"$git_root/share/git-core/templates\"\n    export GIT_TEMPLATE_DIR\nfi\nexec \"$git_root/libexec/git-core/git\" \"$@\"\n";
    fs::write(&launcher, source)
        .map_err(|error| format!("failed to write '{}': {error}", launcher.display()))?;
    make_executable(&launcher)
}

fn is_healthy_git(path: &Path, app_root: &Path) -> bool {
    !is_delegating_git_script(path) && is_healthy_executable(path, app_root)
}

fn is_delegating_git_script(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if metadata.len() > 4096 {
        return false;
    }
    let Ok(source) = fs::read_to_string(path) else {
        return false;
    };
    source.starts_with("#!")
        && source.contains("exec")
        && (source.contains("/usr/bin/git") || source.contains(" git"))
}
