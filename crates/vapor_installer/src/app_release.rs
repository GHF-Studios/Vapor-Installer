//! Verification primitives for independently distributed Vapor application releases.
//!
//! Manifest signatures cover the exact UTF-8 bytes returned by the release feed;
//! the client deliberately does not deserialize and re-serialize before checking
//! the signature.

use ring::{digest, signature};
use semver::Version;
use serde::Deserialize;
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MANIFEST_SCHEMA: u32 = 1;
const MAX_MANIFEST_SIZE: usize = 64 * 1024;
const RELEASE_ORIGIN: &str = "https://vapor.ghf-studios.site/api/updates/v1/vapor/releases/";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VaporAppRelease {
    pub schema: u32,
    pub key_id: String,
    pub version: Version,
    pub channel: String,
    pub platform: String,
    pub minimum_updater_version: Version,
    pub artifact: ReleaseArtifact,
    #[serde(default)]
    pub release_notes_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ReleaseArtifact {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedReleaseKey {
    pub key_id: &'static str,
    pub public_key: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRelease {
    manifest: VaporAppRelease,
}

impl VerifiedRelease {
    pub fn manifest(&self) -> &VaporAppRelease {
        &self.manifest
    }
}

/// Fetch and verify the channel's latest signed manifest from the Vapor feed. This blocks; call it on a worker thread.
pub fn fetch_latest_release(
    client: &reqwest::blocking::Client,
    channel: &str,
    platform: &str,
    trusted_keys: &[TrustedReleaseKey],
    updater_version: &Version,
    installed_version: Option<&Version>,
) -> Result<VerifiedRelease, ReleaseVerificationError> {
    if !valid_path_segment(channel) || !valid_path_segment(platform) {
        return Err(ReleaseVerificationError::InvalidEndpointSegment);
    }
    let endpoint = format!(
        "https://vapor.ghf-studios.site/api/updates/v1/vapor/{channel}/{platform}/latest.json"
    );
    let manifest_response = client
        .get(endpoint)
        .send()
        .map_err(ReleaseVerificationError::Network)?
        .error_for_status()
        .map_err(ReleaseVerificationError::Network)?;
    require_trusted_feed_host(manifest_response.url())?;
    let manifest_bytes = read_limited(manifest_response, MAX_MANIFEST_SIZE)?;

    let signature_endpoint = format!(
        "https://vapor.ghf-studios.site/api/updates/v1/vapor/{channel}/{platform}/latest.json.sig"
    );
    let signature_response = client
        .get(signature_endpoint)
        .send()
        .map_err(ReleaseVerificationError::Network)?
        .error_for_status()
        .map_err(ReleaseVerificationError::Network)?;
    require_trusted_feed_host(signature_response.url())?;
    let signature_bytes = read_limited(signature_response, 128)?;
    let signature_hex = std::str::from_utf8(&signature_bytes)
        .map_err(|_| ReleaseVerificationError::InvalidSignatureEncoding)?;

    verify_manifest(
        &manifest_bytes,
        signature_hex,
        trusted_keys,
        channel,
        platform,
        updater_version,
        installed_version,
    )
}

/// Download a verified release to a caller-selected staging file. This blocks; call it on a worker thread.
///
/// The destination must not already exist. A partial or invalid file is removed
/// on failure; the active installation is never touched by this operation.
pub fn download_verified_archive(
    client: &reqwest::blocking::Client,
    release: &VerifiedRelease,
    destination: &Path,
) -> Result<(), ReleaseVerificationError> {
    let mut created_destination = false;
    let result = (|| {
        let mut response = client
            .get(&release.manifest.artifact.url)
            .send()
            .map_err(ReleaseVerificationError::Network)?
            .error_for_status()
            .map_err(ReleaseVerificationError::Network)?;
        require_trusted_feed_host(response.url())?;
        if let Some(content_length) = response.content_length() {
            if content_length != release.manifest.artifact.size {
                return Err(ReleaseVerificationError::SizeMismatch {
                    expected: release.manifest.artifact.size,
                    actual: content_length,
                });
            }
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .map_err(|source| ReleaseVerificationError::FileIo {
                path: destination.to_owned(),
                source,
            })?;
        created_destination = true;
        let mut total = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = response
                .read(&mut buffer)
                .map_err(ReleaseVerificationError::ResponseRead)?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or(ReleaseVerificationError::ArtifactTooLarge)?;
            if total > release.manifest.artifact.size {
                return Err(ReleaseVerificationError::SizeMismatch {
                    expected: release.manifest.artifact.size,
                    actual: total,
                });
            }
            output.write_all(&buffer[..count]).map_err(|source| {
                ReleaseVerificationError::FileIo {
                    path: destination.to_owned(),
                    source,
                }
            })?;
        }
        output
            .sync_all()
            .map_err(|source| ReleaseVerificationError::FileIo {
                path: destination.to_owned(),
                source,
            })?;
        drop(output);
        let archive =
            File::open(destination).map_err(|source| ReleaseVerificationError::FileIo {
                path: destination.to_owned(),
                source,
            })?;
        verify_archive(release, archive)
    })();

    if result.is_err() && created_destination {
        let _ = fs::remove_file(destination);
    }
    result
}

fn valid_path_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn require_trusted_feed_host(url: &reqwest::Url) -> Result<(), ReleaseVerificationError> {
    if url.scheme() != "https" || url.host_str() != Some("vapor.ghf-studios.site") {
        return Err(ReleaseVerificationError::UnexpectedRedirect);
    }
    Ok(())
}

fn read_limited(
    response: reqwest::blocking::Response,
    limit: usize,
) -> Result<Vec<u8>, ReleaseVerificationError> {
    let maximum = limit as u64 + 1;
    let mut bytes = Vec::with_capacity(limit.min(4096));
    response
        .take(maximum)
        .read_to_end(&mut bytes)
        .map_err(ReleaseVerificationError::ResponseRead)?;
    if bytes.len() > limit {
        return Err(ReleaseVerificationError::ManifestTooLarge);
    }
    Ok(bytes)
}
/// Verify the signed manifest and enforce the requested release policy.
///
/// The trust list is supplied by the caller so development keys are never
/// confused with a production key. A production build must pass its reviewed,
/// embedded public key here; an empty or non-matching list fails closed.
pub fn verify_manifest(
    manifest_bytes: &[u8],
    signature_hex: &str,
    trusted_keys: &[TrustedReleaseKey],
    expected_channel: &str,
    expected_platform: &str,
    updater_version: &Version,
    installed_version: Option<&Version>,
) -> Result<VerifiedRelease, ReleaseVerificationError> {
    if manifest_bytes.len() > MAX_MANIFEST_SIZE {
        return Err(ReleaseVerificationError::ManifestTooLarge);
    }
    let manifest: VaporAppRelease = serde_json::from_slice(manifest_bytes)
        .map_err(ReleaseVerificationError::InvalidManifest)?;
    if manifest.schema != MANIFEST_SCHEMA {
        return Err(ReleaseVerificationError::UnsupportedSchema(manifest.schema));
    }
    if manifest.channel != expected_channel {
        return Err(ReleaseVerificationError::ChannelMismatch {
            expected: expected_channel.to_owned(),
            actual: manifest.channel,
        });
    }
    if manifest.platform != expected_platform {
        return Err(ReleaseVerificationError::PlatformMismatch {
            expected: expected_platform.to_owned(),
            actual: manifest.platform,
        });
    }
    if manifest.minimum_updater_version > *updater_version {
        return Err(ReleaseVerificationError::UpdaterTooOld {
            required: manifest.minimum_updater_version,
            current: updater_version.clone(),
        });
    }
    if let Some(installed) = installed_version {
        if manifest.version <= *installed {
            return Err(ReleaseVerificationError::NotNewer {
                offered: manifest.version,
                installed: installed.clone(),
            });
        }
    }
    validate_artifact_url(
        &manifest.artifact.url,
        &manifest.version,
        &manifest.platform,
    )?;
    if manifest.artifact.size == 0 {
        return Err(ReleaseVerificationError::InvalidArtifactSize);
    }
    validate_hex_digest(&manifest.artifact.sha256)?;

    let key = trusted_keys
        .iter()
        .find(|key| key.key_id == manifest.key_id)
        .ok_or_else(|| ReleaseVerificationError::UnknownKey(manifest.key_id.clone()))?;
    let signature_bytes = decode_hex(signature_hex)?;
    signature::UnparsedPublicKey::new(&signature::ED25519, key.public_key)
        .verify(manifest_bytes, &signature_bytes)
        .map_err(|_| ReleaseVerificationError::BadSignature)?;

    Ok(VerifiedRelease { manifest })
}

/// Stream and verify downloaded archive bytes without loading the whole archive into memory.
pub fn verify_archive<R: std::io::Read>(
    release: &VerifiedRelease,
    mut reader: R,
) -> Result<(), ReleaseVerificationError> {
    let expected_size = release.manifest.artifact.size;
    let mut context = digest::Context::new(&digest::SHA256);
    let mut buffer = [0_u8; 64 * 1024];
    let mut actual_size = 0_u64;
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(ReleaseVerificationError::ArchiveRead)?;
        if count == 0 {
            break;
        }
        actual_size = actual_size
            .checked_add(count as u64)
            .ok_or(ReleaseVerificationError::ArtifactTooLarge)?;
        if actual_size > expected_size {
            return Err(ReleaseVerificationError::SizeMismatch {
                expected: expected_size,
                actual: actual_size,
            });
        }
        context.update(&buffer[..count]);
    }
    if actual_size != expected_size {
        return Err(ReleaseVerificationError::SizeMismatch {
            expected: expected_size,
            actual: actual_size,
        });
    }
    let actual_hex = encode_hex(context.finish().as_ref());
    if !actual_hex.eq_ignore_ascii_case(&release.manifest.artifact.sha256) {
        return Err(ReleaseVerificationError::DigestMismatch);
    }
    Ok(())
}
fn validate_artifact_url(
    url: &str,
    version: &Version,
    platform: &str,
) -> Result<(), ReleaseVerificationError> {
    let expected = format!("{RELEASE_ORIGIN}{version}/{platform}/vapor.tar.gz");
    if url != expected {
        return Err(ReleaseVerificationError::InvalidArtifactUrl);
    }
    Ok(())
}

fn validate_hex_digest(hex: &str) -> Result<(), ReleaseVerificationError> {
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ReleaseVerificationError::InvalidDigest);
    }
    Ok(())
}

fn decode_hex(hex: &str) -> Result<Vec<u8>, ReleaseVerificationError> {
    if hex.len() != 128 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ReleaseVerificationError::InvalidSignatureEncoding);
    }
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high =
                hex_nibble(pair[0]).ok_or(ReleaseVerificationError::InvalidSignatureEncoding)?;
            let low =
                hex_nibble(pair[1]).ok_or(ReleaseVerificationError::InvalidSignatureEncoding)?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[derive(Debug)]
pub enum ReleaseVerificationError {
    InvalidManifest(serde_json::Error),
    ManifestTooLarge,
    InvalidEndpointSegment,
    UnexpectedRedirect,
    Network(reqwest::Error),
    ResponseRead(std::io::Error),
    FileIo {
        path: PathBuf,
        source: std::io::Error,
    },
    UnsupportedSchema(u32),
    ChannelMismatch {
        expected: String,
        actual: String,
    },
    PlatformMismatch {
        expected: String,
        actual: String,
    },
    UpdaterTooOld {
        required: Version,
        current: Version,
    },
    NotNewer {
        offered: Version,
        installed: Version,
    },
    InvalidArtifactUrl,
    InvalidDigest,
    InvalidArtifactSize,
    UnknownKey(String),
    InvalidSignatureEncoding,
    BadSignature,
    ArtifactTooLarge,
    ArchiveRead(std::io::Error),
    SizeMismatch {
        expected: u64,
        actual: u64,
    },
    DigestMismatch,
}

impl fmt::Display for ReleaseVerificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidManifest(error) => {
                write!(formatter, "release manifest is invalid: {error}")
            }
            Self::ManifestTooLarge => {
                formatter.write_str("release response exceeds its size limit")
            }
            Self::InvalidEndpointSegment => {
                formatter.write_str("release channel or platform contains unsafe path characters")
            }
            Self::UnexpectedRedirect => {
                formatter.write_str("release request left the trusted Vapor HTTPS origin")
            }
            Self::Network(error) => write!(formatter, "release network request failed: {error}"),
            Self::ResponseRead(error) => {
                write!(formatter, "could not read release response: {error}")
            }
            Self::FileIo { path, source } => write!(
                formatter,
                "could not use staged file {}: {source}",
                path.display()
            ),
            Self::UnsupportedSchema(schema) => {
                write!(formatter, "unsupported release schema {schema}")
            }
            Self::ChannelMismatch { expected, actual } => write!(
                formatter,
                "release channel mismatch: expected {expected}, got {actual}"
            ),
            Self::PlatformMismatch { expected, actual } => write!(
                formatter,
                "release platform mismatch: expected {expected}, got {actual}"
            ),
            Self::UpdaterTooOld { required, current } => write!(
                formatter,
                "release requires updater {required} or newer; current updater is {current}"
            ),
            Self::NotNewer { offered, installed } => write!(
                formatter,
                "release {offered} is not newer than installed version {installed}"
            ),
            Self::InvalidArtifactUrl => {
                formatter.write_str("artifact URL is outside the trusted Vapor release origin")
            }
            Self::InvalidDigest => {
                formatter.write_str("artifact SHA-256 must be 64 hexadecimal characters")
            }
            Self::InvalidArtifactSize => {
                formatter.write_str("artifact size must be greater than zero")
            }
            Self::UnknownKey(key) => {
                write!(formatter, "release signing key '{key}' is not trusted")
            }
            Self::InvalidSignatureEncoding => {
                formatter.write_str("release signature must be 128 hexadecimal characters")
            }
            Self::BadSignature => formatter.write_str("release manifest signature is invalid"),
            Self::ArtifactTooLarge => {
                formatter.write_str("artifact size cannot be represented on this platform")
            }
            Self::ArchiveRead(error) => {
                write!(formatter, "could not read downloaded archive: {error}")
            }
            Self::SizeMismatch { expected, actual } => write!(
                formatter,
                "artifact size mismatch: expected {expected} bytes, got {actual}"
            ),
            Self::DigestMismatch => {
                formatter.write_str("artifact SHA-256 does not match the signed manifest")
            }
        }
    }
}

impl std::error::Error for ReleaseVerificationError {}
