//! Small in-process bridge from `vapor-installer` to the app-root tool layer.
//!
//! The shipped binary cannot assume Rust, Cargo, Git, or SteamCMD already exist
//! on a tester machine, so the app-root launcher compiles the shared tool
//! implementation and dispatches to it directly.

#[path = "../../../../resources/vapor/tools/_shared/vapor_tools.rs"]
mod vapor_tools;

use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub(crate) enum ToolScript {
    Status,
    SetupPlayer,
    TeardownPlayer,
    SetupDevelopment,
    TeardownDevelopment,
    PostInstall,
    PreUninstall,
}

impl ToolScript {
    fn label(self) -> &'static str {
        match self {
            Self::Status => "app-root status",
            Self::SetupPlayer => "player setup",
            Self::TeardownPlayer => "player teardown",
            Self::SetupDevelopment => "development setup",
            Self::TeardownDevelopment => "development teardown",
            Self::PostInstall => "post-install anchor",
            Self::PreUninstall => "pre-uninstall cleanup",
        }
    }
}

pub(crate) fn run(
    script: ToolScript,
    _app_root: Option<&Path>,
    _extra_args: &[String],
    _quiet: bool,
) -> Result<(), String> {
    match script {
        ToolScript::Status => vapor_tools::status_main(),
        ToolScript::SetupPlayer => vapor_tools::setup_player_main(),
        ToolScript::TeardownPlayer => vapor_tools::teardown_player_main(),
        ToolScript::SetupDevelopment => vapor_tools::setup_development_main(),
        ToolScript::TeardownDevelopment => vapor_tools::teardown_development_main(),
        ToolScript::PostInstall => vapor_tools::post_install_main(),
        ToolScript::PreUninstall => vapor_tools::pre_uninstall_main(),
    }
}

pub(crate) fn run_ide_workflow() -> Result<(), String> {
    vapor_tools::ide_run_main()
}

pub(crate) fn patch_rustrover() -> Result<(), String> {
    vapor_tools::patch_rustrover_main()
}

pub(crate) fn setup_player_for(app_root: &Path) -> Result<(), String> {
    vapor_tools::setup_player_for(app_root)
}

pub(crate) fn create_superworkspace(root: &Path, app_root: Option<&Path>) -> Result<(), String> {
    vapor_tools::create_superworkspace_at(root, app_root)
}

pub(crate) fn source_status(
    super_workspace: Option<&Path>,
    app_root: Option<&Path>,
) -> Result<(), String> {
    vapor_tools::source_status_at(super_workspace, app_root)
}

pub(crate) fn clone_source(
    super_workspace: Option<&Path>,
    app_root: Option<&Path>,
    source_name: &str,
) -> Result<(), String> {
    vapor_tools::clone_source_into_superworkspace(super_workspace, app_root, source_name)
}

pub(crate) fn print_dry_run(script: ToolScript, app_root: Option<&Path>, extra_args: &[String]) {
    let mut parts = vec![script.label().to_owned()];
    if let Some(app_root) = app_root {
        parts.push(format!("app-root={}", shell_quote_path(app_root)));
    }
    parts.extend(extra_args.iter().map(|arg| shell_quote(arg)));
    println!("dry-run: would run precompiled Vapor maintenance operation");
    println!("  {}", parts.join(" "));
}

pub(crate) fn shell_quote(value: &str) -> String {
    let safe = !value.is_empty()
        && value.bytes().all(|byte| {
            matches!(
                byte,
                b'a'..=b'z'
                    | b'A'..=b'Z'
                    | b'0'..=b'9'
                    | b'_'
                    | b'@'
                    | b'%'
                    | b'+'
                    | b'='
                    | b':'
                    | b','
                    | b'.'
                    | b'/'
                    | b'-'
            )
        });
    if safe {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn shell_quote_path(path: &Path) -> String {
    shell_quote(&path.display().to_string())
}
