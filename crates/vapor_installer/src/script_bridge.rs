//! Small in-process bridge from `vapor-installer` to the app-root tool layer.
//!
//! Direct human usage stays script-first:
//!
//! ```text
//! rust-script --force <app-root>/resources/vapor/tools/production/app_setup/setup_player.rs
//! ```
//!
//! The shipped binary cannot assume `rust-script` or Rust already exist on a
//! tester machine, so the app-root launcher compiles the same shared tool
//! implementation and dispatches to it directly.

#[allow(dead_code, unused_variables)]
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
    fn direct_script_path(self) -> &'static str {
        match self {
            Self::Status => "resources/vapor/tools/production/app_setup/status.rs",
            Self::SetupPlayer => "resources/vapor/tools/production/app_setup/setup_player.rs",
            Self::TeardownPlayer => "resources/vapor/tools/production/app_setup/teardown_player.rs",
            Self::SetupDevelopment => {
                "resources/vapor/tools/production/app_setup/setup_development.rs"
            }
            Self::TeardownDevelopment => {
                "resources/vapor/tools/production/app_setup/teardown_development.rs"
            }
            Self::PostInstall => "resources/vapor/tools/production/app_setup/post_install.rs",
            Self::PreUninstall => "resources/vapor/tools/production/app_setup/pre_uninstall.rs",
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

pub(crate) fn print_dry_run(script: ToolScript, app_root: Option<&Path>, extra_args: &[String]) {
    let mut parts = vec![
        "rust-script".to_owned(),
        script.direct_script_path().to_owned(),
    ];
    if let Some(app_root) = app_root {
        parts.push("--app-root".to_owned());
        parts.push(shell_quote_path(app_root));
    }
    parts.extend(extra_args.iter().map(|arg| shell_quote(arg)));
    println!("dry-run: would delegate to script");
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
