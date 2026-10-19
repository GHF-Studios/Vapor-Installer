//! Precompiled command-line front-end for Vapor app-root maintenance and workflows.

use crate::script_bridge::{self, ToolScript};
use std::{env, path::PathBuf};

#[derive(Debug, Default)]
struct ToolOptions {
    app_root: Option<PathBuf>,
    dry_run: bool,
}

#[derive(Debug, Default)]
struct WorkspaceOptions {
    app_root: Option<PathBuf>,
    super_workspace: Option<PathBuf>,
    path: Option<PathBuf>,
    dry_run: bool,
    positionals: Vec<String>,
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
        "superworkspace" => run_superworkspace(args),
        "source" => run_source(args),
        "ide" => run_ide(args),
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
        "Vapor App Maintenance",
        "",
        "Precompiled setup, teardown, app-root maintenance, and IDE workflow launch for the installed Vapor app.",
        "Provider tools such as Rust, Cargo, Git, and SteamCMD are managed capabilities, not prerequisites for this control surface.",
        "",
        "Usage",
        "  vapor-installer status [--app-root PATH]",
        "  vapor-installer install [--app-root PATH] [--dry-run]",
        "  vapor-installer uninstall [--app-root PATH] [--dry-run]",
        "  vapor-installer dev-env status [--app-root PATH]",
        "  vapor-installer dev-env install [--app-root PATH] [--dry-run]",
        "  vapor-installer dev-env uninstall [--app-root PATH] [--dry-run]",
        "  vapor-installer superworkspace create --path PATH [--app-root PATH] [--dry-run]",
        "  vapor-installer source status --super-workspace PATH [--app-root PATH]",
        "  vapor-installer source clone SOURCE_NAME --super-workspace PATH [--app-root PATH] [--dry-run]",
        "  vapor-installer ide patch-rustrover --super-workspace PATH [--app-root PATH]",
        "  vapor-installer ide run --workflow WORKFLOW --super-workspace PATH [--app-root PATH]",
        "  vapor-installer post-install [--app-root PATH]",
        "  vapor-installer pre-uninstall",
        "",
        "Runtime shape",
        "  The shipped binary calls the shared app-root tool implementation in-process so setup and IDE workflow launch do not require Rust to already be installed.",
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

fn run_superworkspace(mut args: Vec<String>) -> Result<(), String> {
    let Some(command) = take_subcommand(&mut args) else {
        return Err("missing superworkspace subcommand: create".to_owned());
    };
    match command.as_str() {
        "create" => {
            let options = parse_workspace_options(args)?;
            let root = options
                .path
                .or_else(|| single_positional_path(&options.positionals))
                .ok_or_else(|| {
                    "usage: vapor-installer superworkspace create --path PATH [--app-root PATH]"
                        .to_owned()
                })?;
            if options.dry_run {
                println!("dry-run: would create SuperWorkspace at {}", root.display());
                return Ok(());
            }
            script_bridge::create_superworkspace(&root, options.app_root.as_deref())
        }
        other => Err(format!("unknown superworkspace subcommand '{other}'")),
    }
}

fn run_source(mut args: Vec<String>) -> Result<(), String> {
    let Some(command) = take_subcommand(&mut args) else {
        return Err("missing source subcommand: status or clone".to_owned());
    };
    match command.as_str() {
        "status" => {
            let options = parse_workspace_options(args)?;
            reject_positionals(&options.positionals, "source status")?;
            script_bridge::source_status(
                options.super_workspace.as_deref(),
                options.app_root.as_deref(),
            )
        }
        "clone" => {
            let options = parse_workspace_options(args)?;
            let source_name = single_positional_value(&options.positionals).ok_or_else(|| {
                "usage: vapor-installer source clone SOURCE_NAME --super-workspace PATH [--app-root PATH]"
                    .to_owned()
            })?;
            if options.dry_run {
                println!("dry-run: would clone source {source_name}");
                if let Some(super_workspace) = &options.super_workspace {
                    println!("  super-workspace: {}", super_workspace.display());
                }
                if let Some(app_root) = &options.app_root {
                    println!("  app-root: {}", app_root.display());
                }
                return Ok(());
            }
            script_bridge::clone_source(
                options.super_workspace.as_deref(),
                options.app_root.as_deref(),
                source_name,
            )
        }
        other => Err(format!("unknown source subcommand '{other}'")),
    }
}

fn run_ide(mut args: Vec<String>) -> Result<(), String> {
    let Some(command) = take_subcommand(&mut args) else {
        return Err("missing ide subcommand: patch-rustrover or run".to_owned());
    };
    match command.as_str() {
        "patch-rustrover" => script_bridge::patch_rustrover(),
        "run" => script_bridge::run_ide_workflow(),
        other => Err(format!("unknown ide subcommand '{other}'")),
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

fn parse_workspace_options(args: Vec<String>) -> Result<WorkspaceOptions, String> {
    let mut options = WorkspaceOptions::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--app-root" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--app-root requires a path".to_owned())?;
                options.app_root = Some(PathBuf::from(value));
            }
            "--super-workspace" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--super-workspace requires a path".to_owned())?;
                options.super_workspace = Some(PathBuf::from(value));
            }
            "--path" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--path requires a path".to_owned())?;
                options.path = Some(PathBuf::from(value));
            }
            "--dry-run" => options.dry_run = true,
            other if other.starts_with("--") => return Err(format!("unknown option '{other}'")),
            positional => options.positionals.push(positional.to_owned()),
        }
    }
    Ok(options)
}

fn single_positional_path(positionals: &[String]) -> Option<PathBuf> {
    single_positional_value(positionals).map(PathBuf::from)
}

fn single_positional_value(positionals: &[String]) -> Option<&str> {
    if positionals.len() == 1 {
        positionals.first().map(String::as_str)
    } else {
        None
    }
}

fn reject_positionals(positionals: &[String], command: &str) -> Result<(), String> {
    if positionals.is_empty() {
        Ok(())
    } else {
        Err(format!("{command} does not accept positional arguments"))
    }
}

fn print_help() {
    println!("{}", help_text());
}
