//! Signed, per-user acquisition and activation of the standalone Vapor app.
//!
//! The Artifact manifest is authenticated before its URL or archive size is
//! trusted. Payloads are downloaded to same-volume staging, checked against
//! their signed size and digest, then extracted into a new immutable version
//! directory. The active pointer changes only after the new entrypoint exists.

use crate::release::{RELEASE_ORIGIN, verify_artifact, verify_manifest, VerifiedReleaseManifest};
use flate2::read::GzDecoder;
use reqwest::blocking::{Client, Response};
use semver::Version;
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tar::{Archive, EntryType};
use vapor_product_layout::{StandaloneProductLayout, current_platform_id};

const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_SIGNATURE_BYTES: u64 = 128;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_UNPACKED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct InstallReport {
    pub version: Version,
    pub channel: String,
    pub platform: String,
    pub release_root: PathBuf,
    pub previous_version: Option<String>,
    pub already_installed: bool,
}

/// Install the latest signed release for this platform and activate it.
pub fn install_latest(channel: &str) -> Result<InstallReport, String> {
    install_latest_inner(channel, false)
}

pub fn repair_latest() -> Result<InstallReport, String> {
    install_latest_inner("stable", true)
}

fn install_latest_inner(channel: &str, force_repair: bool) -> Result<InstallReport, String> {
    if !safe_segment(channel) {
        return Err("release channel must be a safe ASCII path segment".to_owned());
    }
    let platform = current_platform_id()
        .ok_or_else(|| "standalone Vapor is not released for this OS and architecture".to_owned())?;
    let layout = StandaloneProductLayout::discover()
        .ok_or_else(|| "could not determine Vapor's canonical user-data location".to_owned())?;
    ensure_product_root(&layout)?;

    let client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30 * 60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("could not initialize secure release transport: {error}"))?;
    let feed = format!("{RELEASE_ORIGIN}/api/updates/v1/vapor/{channel}/{platform}/latest.json");
    let manifest_bytes = get_bounded(&client, &feed, MAX_MANIFEST_BYTES)?;
    let signature_text = get_bounded(&client, &format!("{feed}.sig"), MAX_SIGNATURE_BYTES)?;
    let signature_text = std::str::from_utf8(&signature_text)
        .map_err(|_| "Artifact returned a non-text manifest signature".to_owned())?;
    let manifest = verify_manifest(
        &manifest_bytes,
        signature_text,
        channel,
        platform,
        &Version::parse(env!("CARGO_PKG_VERSION")).expect("package version is valid SemVer"),
    )
    .map_err(|error| format!("release manifest was rejected: {error}"))?;

    let previous_version = layout
        .active_version()
        .map_err(|error| error.to_string())?;
    let release_root = layout
        .release_root(&manifest.version.to_string())
        .map_err(|error| error.to_string())?;
    match fs::symlink_metadata(&release_root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(format!(
                "release path exists but is not a managed directory: {}",
                release_root.display()
            ));
        }
        Ok(_) if !force_repair => {
            let entrypoint = layout
                .entrypoint_path(&manifest.version.to_string())
                .map_err(|error| error.to_string())?;
            if !entrypoint.is_file() {
                return Err(format!(
                    "release {} already has an incomplete version directory at {}; it was left untouched",
                    manifest.version,
                    release_root.display()
                ));
            }
            layout
                .activate_version(&manifest.version.to_string())
                .map_err(|error| error.to_string())?;
            return Ok(InstallReport {
                version: manifest.version,
                channel: manifest.channel,
                platform: manifest.platform,
                release_root,
                previous_version,
                already_installed: true,
            });
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("could not inspect release path: {error}")),
    }

    let scratch = StageDirectory::create(&layout, &manifest.version.to_string())?;
    let archive_path = scratch.path().join("release.tar.gz");
    download_artifact(&client, &manifest, &archive_path)?;
    let payload_path = scratch.path().join("payload");
    fs::create_dir(&payload_path)
        .map_err(|error| format!("could not create release extraction staging directory: {error}"))?;
    extract_release(&archive_path, &payload_path)?;

    validate_release_payload(&payload_path)?;

    create_managed_directory(&layout.releases_root(), "Vapor releases")
        .map_err(|error| format!("could not create the Vapor releases directory: {error}"))?;
    let backup = if release_root.exists() {
        let backup = layout.releases_root().join(format!(
            ".{}.repair.{}.{}",
            manifest.version,
            std::process::id(),
            NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::rename(&release_root, &backup).map_err(|error| {
            format!("could not stage the current release for repair; close Vapor and retry: {error}")
        })?;
        Some(backup)
    } else {
        None
    };
    if let Err(error) = fs::rename(&payload_path, &release_root) {
        if let Some(backup) = &backup {
            let _ = fs::rename(backup, &release_root);
        }
        return Err(format!("could not commit staged Vapor release: {error}"));
    }
    if let Err(error) = layout.activate_version(&manifest.version.to_string()) {
        let _ = fs::remove_dir_all(&release_root);
        if let Some(backup) = &backup {
            let _ = fs::rename(backup, &release_root);
        }
        return Err(format!("could not activate the repaired release; the prior release was restored when possible: {error}"));
    }
    if let Some(backup) = backup {
        fs::remove_dir_all(&backup).map_err(|error| {
            format!("release was repaired, but the old backup remains at {}: {error}", backup.display())
        })?;
    }

    Ok(InstallReport {
        version: manifest.version,
        channel: manifest.channel,
        platform: manifest.platform,
        release_root,
        previous_version,
        already_installed: false,
    })
}

/// Remove the installed program versions while preserving Vapor User Data,
/// configuration, caches, logs, saves, and authored source checkouts.
pub fn uninstall_app() -> Result<Vec<PathBuf>, String> {
    let layout = StandaloneProductLayout::discover()
        .ok_or_else(|| "could not determine Vapor's canonical user-data location".to_owned())?;
    let _ = layout.active_version().map_err(|error| error.to_string())?;
    let root = layout.root();
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            vapor_core::remove_standalone_machine_integration()
                .map_err(|error| error.to_string())?;
            return Ok(Vec::new());
        }
        Err(error) => return Err(format!("could not inspect the Vapor app directory: {error}")),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(format!("refusing to remove unsafe Vapor app path: {}", root.display()));
        }
        Ok(_) => {}
    }
    let releases = layout.releases_root();
    let remove_releases = match fs::symlink_metadata(&releases) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(format!("refusing to remove unsafe release path: {}", releases.display()));
        }
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(format!("could not inspect installed Vapor releases: {error}")),
    };
    let pointer = layout.active_version_path();
    let remove_pointer = match fs::symlink_metadata(&pointer) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(format!("refusing to remove unsafe active-version pointer: {}", pointer.display()));
        }
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(format!("could not inspect active-version pointer: {error}")),
    };
    vapor_core::remove_standalone_machine_integration()
        .map_err(|error| format!("could not remove Vapor's shell integration: {error}"))?;
    if remove_releases {
        fs::remove_dir_all(&releases)
            .map_err(|error| format!("could not remove installed Vapor releases: {error}"))?;
    }
    if remove_pointer {
        fs::remove_file(&pointer)
            .map_err(|error| format!("could not remove active-version pointer: {error}"))?;
    }
    let mut preserved = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| format!("could not inspect Vapor app directory: {error}"))? {
        preserved.push(entry.map_err(|error| format!("could not inspect Vapor app entry: {error}"))?.path());
    }
    if preserved.is_empty() {
        fs::remove_dir(root).map_err(|error| format!("could not remove empty Vapor app directory: {error}"))?;
    }
    Ok(preserved)
}

pub fn installed_versions() -> Result<Vec<String>, String> {
    let layout = StandaloneProductLayout::discover()
        .ok_or_else(|| "could not determine Vapor's canonical user-data location".to_owned())?;
    layout.installed_versions().map_err(|error| error.to_string())
}

pub fn active_version() -> Result<Option<String>, String> {
    let layout = StandaloneProductLayout::discover()
        .ok_or_else(|| "could not determine Vapor's canonical user-data location".to_owned())?;
    layout.active_version().map_err(|error| error.to_string())
}

pub fn active_entrypoint() -> Result<PathBuf, String> {
    let layout = StandaloneProductLayout::discover()
        .ok_or_else(|| "could not determine Vapor's canonical user-data location".to_owned())?;
    let version = layout
        .active_version()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Vapor is not installed yet.".to_owned())?;
    let entrypoint = layout
        .entrypoint_path(&version)
        .map_err(|error| error.to_string())?;
    if !entrypoint.is_file() {
        return Err(format!("Vapor {version} is incomplete; its entrypoint is missing."));
    }
    Ok(entrypoint)
}

pub fn activate_installed_version(version: &str) -> Result<PathBuf, String> {
    let layout = StandaloneProductLayout::discover()
        .ok_or_else(|| "could not determine Vapor's canonical user-data location".to_owned())?;
    let entrypoint = layout
        .entrypoint_path(version)
        .map_err(|error| error.to_string())?;
    if !entrypoint.is_file() {
        return Err(format!("Vapor release {version} is not installed on this device"));
    }
    layout
        .activate_version(version)
        .map_err(|error| error.to_string())?;
    Ok(entrypoint)
}

fn get_bounded(client: &Client, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut response = successful_response(client.get(url).send(), url)?;
    if response.content_length().is_some_and(|length| length > limit) {
        return Err(format!("Artifact response exceeded the {limit}-byte metadata limit"));
    }
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read Artifact metadata: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("Artifact response exceeded the {limit}-byte metadata limit"));
    }
    Ok(bytes)
}

fn download_artifact(
    client: &Client,
    manifest: &VerifiedReleaseManifest,
    destination: &Path,
) -> Result<(), String> {
    let response = successful_response(
        client.get(&manifest.artifact.url).send(),
        &manifest.artifact.url,
    )?;
    if response
        .content_length()
        .is_some_and(|length| length != manifest.artifact.size)
    {
        return Err("Artifact's Content-Length does not match the signed release size".to_owned());
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| format!("could not create staged release archive: {error}"))?;
    let copied = io::copy(
        &mut response.take(manifest.artifact.size.saturating_add(1)),
        &mut output,
    )
    .map_err(|error| format!("could not download the signed release archive: {error}"))?;
    output
        .flush()
        .and_then(|_| output.sync_all())
        .map_err(|error| format!("could not flush staged release archive: {error}"))?;
    if copied != manifest.artifact.size {
        return Err("downloaded release size does not match the signed manifest".to_owned());
    }
    verify_artifact(
        File::open(destination)
            .map_err(|error| format!("could not reopen staged release archive: {error}"))?,
        &manifest.artifact,
    )
    .map_err(|error| format!("release archive failed integrity verification: {error}"))
}

fn successful_response(
    response: Result<Response, reqwest::Error>,
    url: &str,
) -> Result<Response, String> {
    let response = response.map_err(|error| format!("could not contact Artifact at {url}: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Artifact returned HTTP {} for {url}", response.status()));
    }
    Ok(response)
}

fn extract_release(archive_path: &Path, destination: &Path) -> Result<(), String> {
    let archive = File::open(archive_path)
        .map_err(|error| format!("could not open verified release archive: {error}"))?;
    let decoder = GzDecoder::new(archive);
    let mut archive = Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| format!("release archive is not a valid gzip tar package: {error}"))?;
    let mut seen = HashSet::new();
    let mut count = 0_usize;
    let mut unpacked = 0_u64;
    let mut has_file = false;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("could not read release archive entry: {error}"))?;
        count += 1;
        if count > MAX_ARCHIVE_ENTRIES {
            return Err("release archive contains too many entries".to_owned());
        }
        let entry_type = entry.header().entry_type();
        if entry_type != EntryType::Regular && entry_type != EntryType::Directory {
            return Err("release archive contains a link or unsupported filesystem entry".to_owned());
        }
        let path = entry
            .path()
            .map_err(|error| format!("release archive contains an invalid path: {error}"))?;
        let Some(relative) = safe_archive_path(&path)? else {
            if entry_type == EntryType::Directory {
                continue;
            }
            return Err("release archive contains a file with an empty path".to_owned());
        };
        if !seen.insert(relative.to_string_lossy().to_lowercase()) {
            return Err("release archive contains duplicate or case-colliding paths".to_owned());
        }
        if contains_game_payload(&relative) {
            return Err("release archive contains game files outside the Vapor application boundary".to_owned());
        }
        let size = entry
            .header()
            .size()
            .map_err(|error| format!("release archive contains an invalid file size: {error}"))?;
        unpacked = unpacked
            .checked_add(size)
            .filter(|total| *total <= MAX_UNPACKED_BYTES)
            .ok_or_else(|| "release archive expands beyond the 8 GiB package limit".to_owned())?;
        if entry_type == EntryType::Regular {
            has_file = true;
        }
        let unpacked_safely = entry
            .unpack_in(destination)
            .map_err(|error| format!("could not extract a verified release entry: {error}"))?;
        if !unpacked_safely {
            return Err("release archive contains a path that escapes its staging directory".to_owned());
        }
    }
    if count == 0 || !has_file {
        return Err("release archive contains no application files".to_owned());
    }
    Ok(())
}

fn validate_release_payload(root: &Path) -> Result<(), String> {
    let target = target_triple()?;
    let bin = root.join("bin").join(target);
    let mut required = vec![
        format!("vapor-entrypoint{}", std::env::consts::EXE_SUFFIX),
        format!("vapor{}", std::env::consts::EXE_SUFFIX),
        format!("vapor-installer{}", std::env::consts::EXE_SUFFIX),
        format!("vapor-gui{}", std::env::consts::EXE_SUFFIX),
        format!("vapor-client-service{}", std::env::consts::EXE_SUFFIX),
        format!("vapor-ide-host{}", std::env::consts::EXE_SUFFIX),
    ];
    #[cfg(target_os = "linux")]
    required.push("libsteam_api.so".to_owned());
    #[cfg(target_os = "windows")]
    required.extend(["steam_api64.dll".to_owned(), "libunwind.dll".to_owned()]);

    let missing = required
        .into_iter()
        .filter(|name| !bin.join(name).is_file())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "signed release is incomplete for {target}; required runtime files are missing: {}",
            missing.join(", ")
        ));
    }
    let app_manifest = root.join("App.vapor.toml");
    let source = fs::read_to_string(&app_manifest).map_err(|error| {
        format!("standalone release is missing its Vapor App Instance manifest: {error}")
    })?;
    let manifest: toml::Value = toml::from_str(&source)
        .map_err(|error| format!("standalone App.vapor.toml is invalid: {error}"))?;
    let root_manifest = manifest
        .get("root")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "standalone App.vapor.toml does not declare [root]".to_owned())?;
    if root_manifest.get("name").and_then(toml::Value::as_str) != Some("vapor-client")
        || root_manifest.get("organization").and_then(toml::Value::as_str)
            != Some("ghf-studios")
        || root_manifest.contains_key("steam")
        || manifest.get("content").is_some()
    {
        return Err(
            "standalone App.vapor.toml must identify Vapor and must not contain Steam or game content metadata"
                .to_owned(),
        );
    }
    Ok(())
}

fn safe_archive_path(path: &Path) -> Result<Option<PathBuf>, String> {
    let text = path
        .to_str()
        .ok_or_else(|| "release archive contains a non-UTF-8 path".to_owned())?;
    if text.is_empty()
        || text.contains('\\')
        || text.starts_with('/')
        || text.as_bytes().get(1) == Some(&b':')
    {
        return Err("release archive contains an absolute or non-portable path".to_owned());
    }
    let mut relative = PathBuf::new();
    for part in text.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err("release archive contains a parent-directory path".to_owned()),
            value if value.ends_with('.') || value.ends_with(' ') || value.contains(':') => {
                return Err("release archive contains a non-portable Windows path".to_owned());
            }
            value => relative.push(value),
        }
    }
    if relative.as_os_str().is_empty() {
        Ok(None)
    } else {
        Ok(Some(relative))
    }
}

fn contains_game_payload(path: &Path) -> bool {
    let components = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
        .collect::<Vec<_>>();
    components.iter().any(|part| {
        matches!(
            part.as_str(),
            "madmax" | "mad max" | "madmaxmodding" | "steamapps"
        )
    }) || (components.first().is_some_and(|part| part == "content")
        && components.get(1).is_some_and(|part| {
            matches!(part.as_str(), "default" | "installed" | "packages")
        }))
        || matches!(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase()
                .as_str(),
            "madmax.exe" | "madmax_dx11.exe" | "madmaxmodding.asi"
        )
}

fn ensure_product_root(layout: &StandaloneProductLayout) -> Result<(), String> {
    let root = layout.root();
    let parent = root
        .parent()
        .ok_or_else(|| "Vapor product path has no parent".to_owned())?;
    let user_data_root = parent
        .parent()
        .ok_or_else(|| "Vapor User Data path has no parent".to_owned())?;
    fs::create_dir_all(user_data_root).map_err(|error| {
        format!("could not create Vapor User Data directory {}: {error}", user_data_root.display())
    })?;
    let metadata = fs::symlink_metadata(user_data_root).map_err(|error| {
        format!("could not inspect Vapor User Data directory {}: {error}", user_data_root.display())
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!("Vapor User Data path is not a real directory: {}", user_data_root.display()));
    }
    create_managed_directory(parent, "Vapor apps")?;
    create_managed_directory(root, "Vapor product root")
}

fn create_managed_directory(path: &Path, label: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => Err(format!(
            "{label} path is not a real directory: {}",
            path.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path).or_else(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    Ok(())
                } else {
                    Err(error)
                }
            }).map_err(|error| format!("could not create {label} directory {}: {error}", path.display()))?;
            let metadata = fs::symlink_metadata(path).map_err(|error| {
                format!("could not inspect {label} directory {}: {error}", path.display())
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                Err(format!("{label} path is not a real directory: {}", path.display()))
            } else {
                Ok(())
            }
        }
        Err(error) => Err(format!(
            "could not inspect {label} path {}: {error}",
            path.display()
        )),
    }
}

fn target_triple() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("x86_64-pc-windows-gnullvm"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        _ => Err("standalone Vapor is not released for this OS and architecture".to_owned()),
    }
}

fn safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+'))
}

struct StageDirectory {
    path: PathBuf,
}

impl StageDirectory {
    fn create(layout: &StandaloneProductLayout, version: &str) -> Result<Self, String> {
        let parent = layout.root().join("staging");
        match fs::symlink_metadata(&parent) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(format!("Vapor staging path is not a real directory: {}", parent.display()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create_managed_directory(&parent, "Vapor staging")?;
            }
            Err(error) => return Err(format!("could not inspect Vapor staging directory: {error}")),
        }
        for _ in 0..32 {
            let sequence = NEXT_STAGE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("{version}.{}.{}", std::process::id(), sequence));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("could not create release staging directory: {error}")),
            }
        }
        Err("could not allocate a unique release staging directory".to_owned())
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StageDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
