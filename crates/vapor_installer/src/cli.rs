//! Command-line compatibility front-end for the script-owned setup tools.

use crate::script_bridge::{self, ToolScript};
use std::{
    env,
    path::{Path, PathBuf},
};

#[derive(Debug, Default)]
struct ToolOptions {
    app_root: Option<PathBuf>,
    dry_run: bool,
}

/// Run the installer shim using process arguments from the current environment.
///
/// # Errors
///
/// Returns a user-facing error when command parsing fails, when the matching
/// script cannot be found, or when the delegated script exits unsuccessfully.
pub fn run_from_env() -> Result<(), String> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let quiet = remove_flag(&mut args, "--quiet");
    if args.is_empty() {
        print_help();
        println!();
        return script_bridge::run(ToolScript::Status, None, &[], quiet);
    }
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print_help();
        return Ok(());
    }

    let command = args.remove(0);
    match command.as_str() {
        "status" => run_tool(ToolScript::Status, parse_options(args)?, quiet),
        "install" => run_tool(ToolScript::SetupPlayer, parse_options(args)?, quiet),
        "uninstall" => run_tool(ToolScript::TeardownPlayer, parse_options(args)?, quiet),
        "dev-env" => run_dev_env(args, quiet),
        "post-install" => run_tool(ToolScript::PostInstall, parse_options(args)?, quiet),
        "pre-uninstall" => run_tool(ToolScript::PreUninstall, parse_options(args)?, quiet),
        other => Err(format!(
            "unknown command '{other}'\nhelp: run `vapor-installer --help`"
        )),
    }
}

/// Help text used by the binary and by lightweight compatibility tests.
pub fn help_text() -> String {
    [
        "Vapor Installer Shim",
        "",
        "Setup, teardown, app-root, and toolchain behavior lives in the source-visible scripts under tools/production/app_setup.",
        "This binary exists only as a compatibility entrypoint for Steam launch wrappers and older command habits.",
        "",
        "Usage",
        "  vapor-installer status [--app-root PATH]",
        "  vapor-installer install [--app-root PATH] [--dry-run]",
        "  vapor-installer uninstall [--app-root PATH] [--dry-run]",
        "  vapor-installer dev-env status [--app-root PATH]",
        "  vapor-installer dev-env install [--app-root PATH] [--dry-run]",
        "  vapor-installer dev-env uninstall [--app-root PATH] [--dry-run]",
        "  vapor-installer post-install [--app-root PATH]",
        "  vapor-installer pre-uninstall",
        "",
        "Script equivalents",
        "  rust-script --force tools/production/app_setup/status.rs",
        "  rust-script --force tools/production/app_setup/setup_player.rs --app-root /path/to/app",
        "  rust-script --force tools/production/app_setup/teardown_player.rs --app-root /path/to/app",
        "  rust-script --force tools/production/app_setup/setup_development.rs --app-root /path/to/app",
        "  rust-script --force tools/production/app_setup/teardown_development.rs --app-root /path/to/app",
        "",
        "Runtime shape",
        "  Source checkouts should run scripts directly. The shipped shim calls the same shared script implementation in-process.",
    ]
    .join("\n")
}

fn run_dev_env(mut args: Vec<String>, quiet: bool) -> Result<(), String> {
    let Some(command) = take_subcommand(&mut args) else {
        return Err("missing dev-env subcommand: status, install, or uninstall".to_owned());
    };
    match command.as_str() {
        "status" => run_tool(ToolScript::Status, parse_options(args)?, quiet),
        "install" => run_tool(ToolScript::SetupDevelopment, parse_options(args)?, quiet),
        "uninstall" => run_tool(ToolScript::TeardownDevelopment, parse_options(args)?, quiet),
        other => Err(format!("unknown dev-env subcommand '{other}'")),
    }
}

fn run_tool(script: ToolScript, options: ToolOptions, quiet: bool) -> Result<(), String> {
    if options.dry_run {
        script_bridge::print_dry_run(script, options.app_root.as_deref(), &[]);
        return Ok(());
    }
    script_bridge::run(script, options.app_root.as_deref(), &[], quiet)
}

fn remove_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let mut found = false;
    args.retain(|arg| {
        if arg == flag {
            found = true;
            false
        } else {
            true
        }
    });
    found
}

fn take_subcommand(args: &mut Vec<String>) -> Option<String> {
    if args.is_empty() {
        None
    } else {
        Some(args.remove(0))
    }
}

fn parse_options(args: Vec<String>) -> Result<ToolOptions, String> {
    let mut options = ToolOptions::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--app-root" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--app-root requires a path".to_owned())?;
                options.app_root = Some(PathBuf::from(value));
            }
            "--dry-run" => options.dry_run = true,
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(options)
}

fn print_help() {
    println!("{}", help_text());
}

#[allow(dead_code)]
fn display_command_argument(path: &Path) -> String {
    script_bridge::shell_quote(&path.display().to_string())
}
