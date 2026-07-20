//! Public installer data types.
//!
//! These structs are intentionally small value objects. The command-line UI and
//! future GUI can render them without knowing how acquisition, extraction, or
//! cleanup is implemented.

use std::path::{Path, PathBuf};

/// Relative app-root installer log path.
pub const INSTALLER_LOG: &str = ".vapor/logs/installer.log";

/// Options shared by status and install operations.
#[derive(Debug, Clone, Default)]
pub struct InstallerOptions {
    /// Explicit Steam app root. When absent, the installer derives it from the
    /// running binary or the current directory.
    pub app_root: Option<PathBuf>,
    /// Preview planned changes without mutating the app root.
    pub dry_run: bool,
}

/// Status for the default app-root player-mode install.
#[derive(Debug, Clone)]
pub struct PlayerStatus {
    app_root: PathBuf,
    steamcmd: ComponentStatus,
    directories: ComponentStatus,
}

impl PlayerStatus {
    pub(crate) fn new(
        app_root: PathBuf,
        steamcmd: ComponentStatus,
        directories: ComponentStatus,
    ) -> Self {
        Self {
            app_root,
            steamcmd,
            directories,
        }
    }

    /// App root inspected by this status report.
    pub fn app_root(&self) -> &Path {
        &self.app_root
    }

    /// App-local SteamCMD status.
    pub fn steamcmd(&self) -> &ComponentStatus {
        &self.steamcmd
    }

    /// Required generated directory status.
    pub fn directories(&self) -> &ComponentStatus {
        &self.directories
    }

    /// Whether player-mode tooling is ready for normal closed-alpha runtime use.
    pub fn ready(&self) -> bool {
        self.steamcmd.ready() && self.directories.ready()
    }
}

/// Status for explicit development-environment tooling.
#[derive(Debug, Clone)]
pub struct DevEnvStatus {
    app_root: PathBuf,
    rust: ComponentStatus,
    cross: ComponentStatus,
}

impl DevEnvStatus {
    pub(crate) fn new(app_root: PathBuf, rust: ComponentStatus, cross: ComponentStatus) -> Self {
        Self {
            app_root,
            rust,
            cross,
        }
    }

    /// App root inspected by this status report.
    pub fn app_root(&self) -> &Path {
        &self.app_root
    }

    /// Rust/Cargo toolchain status.
    pub fn rust(&self) -> &ComponentStatus {
        &self.rust
    }

    /// Cross-build helper status.
    pub fn cross(&self) -> &ComponentStatus {
        &self.cross
    }

    /// Whether explicit development tooling is ready.
    pub fn ready(&self) -> bool {
        self.rust.ready() && self.cross.ready()
    }
}

/// Status for one installer-managed component.
#[derive(Debug, Clone)]
pub struct ComponentStatus {
    label: &'static str,
    ready: bool,
    path: PathBuf,
    missing: Vec<String>,
}

impl ComponentStatus {
    pub(crate) fn new(
        label: &'static str,
        ready: bool,
        path: PathBuf,
        missing: Vec<String>,
    ) -> Self {
        Self {
            label,
            ready,
            path,
            missing,
        }
    }

    /// Human-readable component name.
    pub fn label(&self) -> &'static str {
        self.label
    }

    /// Whether the component is ready.
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// Primary path associated with this component.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Missing entries or failed checks.
    pub fn missing(&self) -> &[String] {
        &self.missing
    }
}

/// Report for one installer mutation or dry-run.
#[derive(Debug, Clone)]
pub struct InstallerReport {
    app_root: PathBuf,
    dry_run: bool,
    actions: Vec<String>,
}

impl InstallerReport {
    pub(crate) fn new(app_root: PathBuf, dry_run: bool, actions: Vec<String>) -> Self {
        Self {
            app_root,
            dry_run,
            actions,
        }
    }

    /// App root targeted by the operation.
    pub fn app_root(&self) -> &Path {
        &self.app_root
    }

    /// Whether this was a dry-run.
    pub fn dry_run(&self) -> bool {
        self.dry_run
    }

    /// Actions performed or previewed.
    pub fn actions(&self) -> &[String] {
        &self.actions
    }
}
