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
    revision: Option<String>,
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
        "app" => run_standalone_app(args),
        "install" => install(parse_options(args)?),
        "uninstall" => run_tool(ToolScript::TeardownPlayer, parse_options(args)?, quiet),
        "erase-all" => erase_all(args),
        "dev-env" => run_dev_env(args, quiet),
        "toolchain" => run_toolchain(args),
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
        "Precompiled setup, teardown, maintenance, and IDE workflow launch for the installed Vapor app.",
        "Provider tools such as Rust, Cargo, Git, and SteamCMD are managed capabilities, not prerequisites for this control surface.",
        "",
        "Usage",
        "  vapor-installer status [--app-root PATH]",
        "  vapor-installer app install [--channel CHANNEL]",
        "  vapor-installer app update [--channel CHANNEL]",
        "  vapor-installer app repair",
        "  vapor-installer app versions",
        "  vapor-installer app activate VERSION",
        "  vapor-installer app uninstall --confirm",
        "  vapor-installer app erase-all [--confirm]",
        "  vapor-installer install [--app-root PATH] [--dry-run]",
        "  vapor-installer uninstall [--app-root PATH] [--dry-run]",
        "  vapor-installer erase-all [--app-root PATH] [--confirm]",
        "  vapor-installer dev-env status [--app-root PATH]",
        "  vapor-installer dev-env install [--app-root PATH] [--dry-run]",
        "  vapor-installer dev-env uninstall [--app-root PATH] [--dry-run]",
        "  vapor-installer toolchain install [--dry-run]",
        "  vapor-installer superworkspace create [--path PATH] [--app-root PATH] [--dry-run]",
        "  vapor-installer source status --super-workspace PATH [--app-root PATH]",
        "  vapor-installer source clone SOURCE_NAME [--super-workspace PATH] [--revision COMMIT] [--app-root PATH] [--dry-run]",
        "  vapor-installer ide patch-rustrover --super-workspace PATH [--app-root PATH]",
        "  vapor-installer ide run --workflow WORKFLOW --super-workspace PATH [--app-root PATH]",
        "  vapor-installer post-install [--app-root PATH]",
        "  vapor-installer pre-uninstall",
        "",
        "Runtime shape",
        "  `app` manages independently versioned Vapor releases in canonical Vapor User Data and does not require a Steam app root or developer tools.",
        "  Legacy app-instance setup and IDE workflow commands call the shared Vapor tool implementation in-process.",
    ]
    .join("\n")
}

fn run_standalone_app(mut args: Vec<String>) -> Result<(), String> {
    let command = take_subcommand(&mut args)
        .ok_or_else(|| "usage: vapor-installer app <install|update|repair|versions|activate|uninstall|erase-all>".to_owned())?;
    match command.as_str() {
        "install" | "update" => {
            let channel = parse_channel(args)?;
            let report = crate::standalone::install_latest(&channel)?;
            println!(
                "Vapor {} is active from {} ({} / {}).{}",
                report.version,
                report.release_root.display(),
                report.channel,
                report.platform,
                if report.already_installed {
                    " The verified version was already installed."
                } else {
                    ""
                }
            );
            Ok(())
        }
        "repair" => {
            reject_positionals(&args, "app repair")?;
            let report = crate::standalone::repair_latest()?;
            println!("Vapor {} was repaired and activated at {}.", report.version, report.release_root.display());
            Ok(())
        }
        "versions" => {
            reject_positionals(&args, "app versions")?;
            let active = crate::standalone::active_version()?;
            let versions = crate::standalone::installed_versions()?;
            if versions.is_empty() {
                println!("No standalone Vapor releases are installed.");
            } else {
                for version in versions {
                    println!(
                        "{}{}",
                        if active.as_deref() == Some(version.as_str()) {
                            "* "
                        } else {
                            "  "
                        },
                        version
                    );
                }
            }
            Ok(())
        }
        "activate" => {
            let version = single_positional_value(&args)
                .ok_or_else(|| "usage: vapor-installer app activate VERSION".to_owned())?;
            let path = crate::standalone::activate_installed_version(version)?;
            println!("Vapor {version} is active: {}", path.display());
            Ok(())
        }
        "uninstall" => {
            if args.as_slice() != ["--confirm"] {
                return Err("No data was removed. Repeat `vapor-installer app uninstall --confirm` to remove Vapor's installed program versions while preserving user data and source.".to_owned());
            }
            let preserved = crate::standalone::uninstall_app()?;
            println!("Vapor program versions and shell integration were removed. User data and source were preserved.");
            for path in preserved {
                println!("Preserved unrecognized app entry: {}", path.display());
            }
            Ok(())
        }
        "erase-all" => erase_standalone_vapor_data(args),
        other => Err(format!(
            "unknown app subcommand '{other}'; use install, update, repair, versions, activate, uninstall, or erase-all"
        )),
    }
}

fn parse_channel(args: Vec<String>) -> Result<String, String> {
    let mut channel = "stable".to_owned();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--channel" => {
                channel = iter
                    .next()
                    .ok_or_else(|| "--channel requires a name".to_owned())?;
            }
            other => return Err(format!("unknown app install option '{other}'")),
        }
    }
    Ok(channel)
}

fn erase_all(args: Vec<String>) -> Result<(), String> {
    let mut app_root = None;
    let mut confirmed = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--app-root" => {
                app_root = Some(PathBuf::from(
                    iter.next().ok_or_else(|| "--app-root requires a path".to_owned())?,
                ));
            }
            "--confirm" => confirmed = true,
            other => return Err(format!("unknown erase-all option '{other}'")),
        }
    }
    if app_root.is_none() {
        return erase_standalone_vapor_data(if confirmed { vec!["--confirm".to_owned()] } else { Vec::new() });
    }
    let installation = match app_root {
        Some(root) => vapor_core::VaporInstallation::from_explicit_root(root),
        None => vapor_core::VaporInstallation::discover(),
    }
    .map_err(|error| error.to_string())?;
    println!("Vapor User Data: {}", installation.user_data_root().display());
    if let Ok(state) = vapor_core::source_state(&installation) {
        let registered_root = state
            .superworkspace
            .or(state.active)
            .or_else(|| (state.known.len() == 1).then(|| state.known[0].clone()));
        if let Some(root) = registered_root {
        println!("Registered source workspace: {}", root.display());
        for name in [".idea", ".run"] {
            let generated = root.join(name);
            if generated.exists() {
                println!("  generated IDE state: {}", generated.display());
            }
        }
        if let Ok(workspace) = vapor_core::VaporSuperworkspace::discover_from(&root) {
            for repository in workspace.repositories {
                println!("  source checkout: {}", repository.root.display());
            }
        }
        println!("Unrecognized neighboring files in this workspace are preserved.");
        }
    }
    for name in ["state", "rustup-home", "cargo-home", "development"] {
        let legacy = installation.root.join(name);
        if legacy.exists() {
            println!("Legacy Vapor state: {}", legacy.display());
        }
    }
    if !confirmed {
        return Err("No data was removed. Review the paths above, then repeat with --confirm to permanently erase Vapor data.".to_owned());
    }
    let report = vapor_core::uninstall_installation_for(
        &installation,
        vapor_core::UninstallOptions {
            erase_all: true,
            ..Default::default()
        },
    )
    .map_err(|error| error.to_string())?;
    println!("Vapor data erased. Machine integration removed.");
    for path in report.preserved_superworkspace_entries {
        println!("Preserved: {}", path.display());
    }
    Ok(())
}

fn erase_standalone_vapor_data(args: Vec<String>) -> Result<(), String> {
    let confirmed = match args.as_slice() {
        [] => false,
        [flag] if flag == "--confirm" => true,
        _ => return Err("usage: vapor-installer app erase-all [--confirm]".to_owned()),
    };
    let plan = vapor_core::plan_vapor_data_erasure().map_err(|error| error.to_string())?;
    println!("Vapor User Data to erase: {}", plan.user_data_root.display());
    if let Some(root) = &plan.superworkspace_root {
        println!("Registered source workspace: {}", root.display());
    }
    for path in &plan.source_repositories {
        println!("  source checkout: {}", path.display());
    }
    for path in &plan.superworkspace_data {
        println!("  Vapor-managed workspace data: {}", path.display());
    }
    for path in &plan.preserved_superworkspace_entries {
        println!("  preserved neighboring entry: {}", path.display());
    }
    if !confirmed {
        return Err("No data was removed. Review the paths above, then repeat with `--confirm` to permanently erase Vapor data.".to_owned());
    }
    let report = vapor_core::erase_all_vapor_data_from_plan(&plan)
        .map_err(|error| error.to_string())?;
    println!("Vapor User Data and registered source checkouts were erased. Steam-managed Loo-Cast remains installed.");
    for path in report.preserved_superworkspace_entries {
        println!("Preserved neighboring entry: {}", path.display());
    }
    Ok(())
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

fn run_toolchain(mut args: Vec<String>) -> Result<(), String> {
    let Some(command) = take_subcommand(&mut args) else {
        return Err("missing toolchain subcommand: install".to_owned());
    };
    match command.as_str() {
        "install" if args.as_slice() == ["--dry-run"] => {
            println!("dry-run: would install Rust/Cargo and cross tools in Vapor User Data");
            Ok(())
        }
        "install" if args.is_empty() => script_bridge::install_managed_toolchain(),
        "install" => Err("usage: vapor-installer toolchain install [--dry-run]".to_owned()),
        other => Err(format!("unknown toolchain subcommand '{other}'; use install")),
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
                .unwrap_or_else(canonical_superworkspace_root);
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
            if options.revision.is_some() {
                return Err("--revision is only valid for source clone".to_owned());
            }
            reject_positionals(&options.positionals, "source status")?;
            script_bridge::source_status(
                options.super_workspace.as_deref(),
                options.app_root.as_deref(),
            )
        }
        "clone" => {
            let options = parse_workspace_options(args)?;
            let source_name = single_positional_value(&options.positionals).ok_or_else(|| {
                "usage: vapor-installer source clone SOURCE_NAME [--super-workspace PATH] [--revision COMMIT] [--app-root PATH]"
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
                if let Some(revision) = &options.revision {
                    println!("  revision: {revision}");
                }
                return Ok(());
            }
            script_bridge::clone_source(
                options.super_workspace.as_deref(),
                options.app_root.as_deref(),
                source_name,
                options.revision.as_deref(),
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

fn install(options: ToolOptions) -> Result<(), String> {
    if options.dry_run {
        println!("dry-run: would establish Vapor integration and Player setup");
        return Ok(());
    }
    let installation = match options.app_root {
        Some(root) => vapor_core::VaporInstallation::from_explicit_root(root),
        None => vapor_core::VaporInstallation::discover(),
    }
    .map_err(|error| error.to_string())?;
    let report = crate::prepare_player_installation(&installation)?;
    println!("Vapor ready at {}", report.installation_root.display());
    Ok(())
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
            "--revision" => {
                options.revision = Some(
                    iter.next()
                        .ok_or_else(|| "--revision requires a commit SHA".to_owned())?,
                );
            }
            "--dry-run" => options.dry_run = true,
            other if other.starts_with("--") => return Err(format!("unknown option '{other}'")),
            positional => options.positionals.push(positional.to_owned()),
        }
    }
    Ok(options)
}

fn canonical_superworkspace_root() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(|home| home.join("Vapor"))
        .unwrap_or_else(|| PathBuf::from("Vapor"))
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
