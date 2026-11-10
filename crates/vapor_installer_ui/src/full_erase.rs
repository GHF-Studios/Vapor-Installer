#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone)]
pub(crate) struct EraseTarget {
    pub label: &'static str,
    pub path: PathBuf,
}

#[cfg(windows)]
pub(crate) fn inventory(home: &Path) -> Result<Vec<EraseTarget>, String> {
    let profile = env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .ok_or_else(|| "Windows user profile could not be resolved.".to_owned())?;
    let expected_home = env::var_os(vapor_core::VAPOR_HOME_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| profile.join("Vapor"));
    validate_owned_directory(home, &expected_home, true)?;

    let mut targets = vec![EraseTarget {
        label: "Vapor Home: applications, tools, sources, caches, content, and settings",
        path: home.to_path_buf(),
    }];

    for (label, variable, suffix) in [
        (
            "Legacy local Vapor state and toolchains",
            "LOCALAPPDATA",
            "Vapor",
        ),
        ("Legacy roaming Vapor settings", "APPDATA", "Vapor"),
    ] {
        if let Some(base) = env::var_os(variable).map(PathBuf::from) {
            let path = base.join(suffix);
            if path.exists() {
                validate_owned_directory(&path, &base.join(suffix), false)?;
                targets.push(EraseTarget { label, path });
            }
        }
    }

    for (label, path) in [(
        "Vapor external tools and recovery staging",
        PathBuf::from(r"C:\VaporTools"),
    )] {
        if path.exists() {
            validate_owned_directory(&path, &path, false)?;
            targets.push(EraseTarget { label, path });
        }
    }

    Ok(targets)
}

pub(crate) fn preserved_user_rust_paths(home: &Path) -> Vec<PathBuf> {
    let profile = env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(PathBuf::from));
    let Some(profile) = profile else {
        return Vec::new();
    };

    [profile.join(".cargo"), profile.join(".rustup")]
        .into_iter()
        .filter(|path| path.exists())
        .filter(|path| !path.starts_with(home))
        .collect()
}

#[cfg(not(windows))]
pub(crate) fn inventory(_home: &Path) -> Result<Vec<EraseTarget>, String> {
    Err("Full Vapor erasure handoff is currently available on Windows only.".to_owned())
}

#[cfg(windows)]
pub(crate) fn schedule(home: &Path) -> Result<PathBuf, String> {
    let targets = inventory(home)?;
    let installation = vapor_core::VaporInstallation::from_explicit_root(home.to_path_buf())
        .map_err(|error| error.to_string())?;
    vapor_core::validate_superworkspace_for_erasure(&installation)
        .map_err(|error| format!("Source safety check refused the erase: {error}"))?;
    let temp = env::var_os("TEMP")
        .map(PathBuf::from)
        .ok_or_else(|| "Windows temporary directory could not be resolved.".to_owned())?;
    fs::create_dir_all(&temp)
        .map_err(|error| format!("Could not create the cleanup handoff directory: {error}"))?;
    let script_path = temp.join(format!("vapor-erase-{}.ps1", std::process::id()));
    let log_path = temp.join(format!("vapor-erase-{}.log", std::process::id()));
    let paths = targets
        .iter()
        .map(|target| format!("    {}", powershell_literal(&target.path.to_string_lossy())))
        .collect::<Vec<_>>()
        .join(",\n");
    let script = format!(
        r#"$ErrorActionPreference = 'Stop'
$parent = {parent}
$vaporHomePath = {home}
$targets = @(
{paths}
)
$log = {log}
try {{
    while (Get-Process -Id $parent -ErrorAction SilentlyContinue) {{ Start-Sleep -Milliseconds 300 }}
    $homeItem = Get-Item -LiteralPath $vaporHomePath -Force
    if (($homeItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {{ throw 'Vapor Home became a link after preflight.' }}
    $marker = Join-Path $vaporHomePath 'Vapor-Home.toml'
    if (-not (Test-Path -LiteralPath $marker) -or -not (Get-Content -LiteralPath $marker -Raw).Contains('id = "ghf-studios/vapor"')) {{ throw 'Vapor Home identity changed after preflight.' }}
    foreach ($target in $targets) {{
        if (Test-Path -LiteralPath $target) {{
            $item = Get-Item -LiteralPath $target -Force
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {{ throw "Refusing linked Vapor cleanup path $target" }}
            Remove-Item -LiteralPath $target -Recurse -Force
            if (Test-Path -LiteralPath $target) {{ throw "Vapor data remains at $target" }}
        }}
    }}
    [IO.File]::WriteAllText($log, 'Vapor data erasure completed.')
}} catch {{
    [IO.File]::WriteAllText($log, "Vapor data erasure failed: $($_.Exception.Message)")
}}
Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
"#,
        parent = std::process::id(),
        home = powershell_literal(&home.to_string_lossy()),
        log = powershell_literal(&log_path.to_string_lossy()),
    );
    fs::write(&script_path, script)
        .map_err(|error| format!("Could not stage the cleanup handoff: {error}"))?;

    if let Err(error) = vapor_core::uninstall_installation_for(
        &installation,
        vapor_core::UninstallOptions::default(),
    ) {
        let _ = fs::remove_file(&script_path);
        return Err(format!(
            "Could not remove Vapor command registration: {error}"
        ));
    }

    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script_path);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    if let Err(error) = command.spawn() {
        let _ = fs::remove_file(&script_path);
        return match vapor_core::install_installation_for(&installation) {
            Ok(_) => Err(format!(
                "Could not start the cleanup handoff: {error}. Vapor command registration was restored."
            )),
            Err(restore_error) => Err(format!(
                "Could not start the cleanup handoff: {error}. Restoring Vapor command registration also failed: {restore_error}. Reopen the installer from Vapor Home to repair it."
            )),
        };
    }
    Ok(log_path)
}

#[cfg(not(windows))]
pub(crate) fn schedule(_home: &Path) -> Result<PathBuf, String> {
    Err("Full Vapor erasure handoff is currently available on Windows only.".to_owned())
}

fn validate_owned_directory(
    path: &Path,
    expected: &Path,
    require_home_marker: bool,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect cleanup path {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "Refusing cleanup path that is not a real directory: {}",
            path.display()
        ));
    }
    let actual = fs::canonicalize(path)
        .map_err(|error| format!("Could not resolve cleanup path {}: {error}", path.display()))?;
    let expected = fs::canonicalize(expected).map_err(|error| {
        format!(
            "Could not resolve expected Vapor path {}: {error}",
            expected.display()
        )
    })?;
    if actual != expected {
        return Err(format!(
            "Refusing cleanup path outside its known Vapor location: {}",
            path.display()
        ));
    }
    if require_home_marker {
        let marker = actual.join("Vapor-Home.toml");
        let source = fs::read_to_string(&marker)
            .map_err(|error| format!("Could not verify {}: {error}", marker.display()))?;
        if !source.contains("id = \"ghf-studios/vapor\"") {
            return Err(format!(
                "Refusing to erase a directory without Vapor's home identity: {}",
                actual.display()
            ));
        }
    }
    Ok(())
}

fn powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
