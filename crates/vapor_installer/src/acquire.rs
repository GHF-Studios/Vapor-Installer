//! Download, extraction, and checksum helpers.
//!
//! Network acquisition is intentionally centralized so later GUI progress,
//! checksum policy, mirrors, and resumable downloads have one implementation
//! boundary.

use crate::fsutil::{Logger, ensure_contained};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub(crate) fn downloads_dir(app_root: &Path) -> Result<PathBuf, String> {
    let path = app_root.join(".vapor/downloads");
    ensure_contained(app_root, &path)?;
    fs::create_dir_all(&path)
        .map_err(|error| format!("failed to create '{}': {error}", path.display()))?;
    Ok(path)
}

pub(crate) fn download(url: &str, destination: &Path, logger: &mut Logger) -> Result<(), String> {
    logger.log(format!("downloading {url} -> {}", destination.display()));
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
    }
    let mut curl = Command::new("curl");
    curl.args([
        "--proto",
        "=https",
        "--tlsv1.2",
        "--fail",
        "--location",
        "--silent",
        "--show-error",
        "--output",
    ])
    .arg(destination)
    .arg(url);
    logger.attach_command_output(&mut curl);
    let curl_status = curl.status();
    match curl_status {
        Ok(status) if status.success() => return Ok(()),
        Ok(status) => logger.log(format!("curl exited with {status}; trying wget")),
        Err(error) => logger.log(format!("failed to start curl: {error}; trying wget")),
    }
    let mut wget = Command::new("wget");
    wget.arg("-O").arg(destination).arg(url);
    logger.attach_command_output(&mut wget);
    let wget_status = wget.status();
    match wget_status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) if cfg!(target_os = "windows") => {
            logger.log(format!("wget exited with {status}; trying PowerShell"));
            powershell_download(url, destination, logger)
        }
        Ok(status) => Err(format!(
            "failed to download '{url}': wget exited with {status}"
        )),
        Err(error) if cfg!(target_os = "windows") => {
            logger.log(format!("failed to start wget: {error}; trying PowerShell"));
            powershell_download(url, destination, logger)
        }
        Err(error) => Err(format!("failed to start curl or wget for '{url}': {error}")),
    }
}

pub(crate) fn extract_zip(
    archive: &Path,
    target: &Path,
    label: &str,
    logger: &mut Logger,
) -> Result<(), String> {
    logger.log(format!(
        "extracting {label}: {} -> {}",
        archive.display(),
        target.display()
    ));
    let command = format!(
        "Expand-Archive -LiteralPath {} -DestinationPath {} -Force",
        powershell_literal(&archive.display().to_string()),
        powershell_literal(&target.display().to_string())
    );
    let mut powershell = Command::new("powershell");
    powershell
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
        ])
        .arg(command);
    logger.attach_command_output(&mut powershell);
    let status = powershell
        .status()
        .map_err(|error| format!("failed to start PowerShell for {label}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{label} extraction exited with {status}"))
    }
}

pub(crate) fn extract_tar_xz(
    archive: &Path,
    target: &Path,
    label: &str,
    logger: &Logger,
) -> Result<(), String> {
    let mut tar = Command::new("tar");
    tar.args(["-xJf"]).arg(archive).arg("-C").arg(target);
    logger.attach_command_output(&mut tar);
    let status = tar
        .status()
        .map_err(|error| format!("failed to start tar for {label}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{label} extraction exited with {status}"))
    }
}

pub(crate) fn verify_sha256_with_powershell(path: &Path, expected: &str) -> Result<(), String> {
    if !cfg!(target_os = "windows") {
        return Ok(());
    }
    let command = format!(
        "(Get-FileHash -Algorithm SHA256 -LiteralPath {}).Hash",
        powershell_literal(&path.display().to_string())
    );
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
        ])
        .arg(command)
        .output()
        .map_err(|error| {
            format!(
                "failed to start PowerShell checksum verification for '{}': {error}",
                path.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "checksum verification for '{}' exited with {}",
            path.display(),
            output.status
        ));
    }
    let actual = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "checksum mismatch for '{}'\n  expected: {expected}\n  actual:   {actual}",
            path.display()
        ))
    }
}

pub(crate) fn verify_sha256_with_sha256sum(path: &Path, expected: &str) -> Result<(), String> {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|error| {
            format!(
                "failed to start sha256sum for '{}': {error}",
                path.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "checksum verification for '{}' exited with {}",
            path.display(),
            output.status
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let actual = stdout.split_whitespace().next().unwrap_or("");
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "checksum mismatch for '{}'\n  expected: {expected}\n  actual:   {actual}",
            path.display()
        ))
    }
}

fn powershell_download(url: &str, destination: &Path, logger: &Logger) -> Result<(), String> {
    let command = format!(
        "$ProgressPreference = 'SilentlyContinue'; Invoke-WebRequest -Uri {} -OutFile {}",
        powershell_literal(url),
        powershell_literal(&destination.display().to_string())
    );
    let mut powershell = Command::new("powershell");
    powershell
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
        ])
        .arg(command);
    logger.attach_command_output(&mut powershell);
    let status = powershell
        .status()
        .map_err(|error| format!("failed to start PowerShell download for '{url}': {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "failed to download '{url}': PowerShell exited with {status}"
        ))
    }
}

fn powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
