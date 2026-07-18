//! Command-line and visual TUI front-end.
//!
//! The installer library exposes structured operations. This module is only the
//! human-facing adapter used by `vapor-installer`.

use crate::{
    BootstrapStatus, BootstrapUninstallOptions, ComponentStatus, DevEnvStatus, INSTALLER_LOG,
    InstallerOptions, InstallerReport, bootstrap_install, bootstrap_status, bootstrap_uninstall,
    dev_env_install, dev_env_status, dev_env_uninstall,
};
use std::{
    env,
    io::{self, Write},
    path::PathBuf,
};

/// Run the installer using process arguments from the current environment.
///
/// # Errors
///
/// Returns a user-facing error when command parsing or installer execution
/// fails.
pub fn run_from_env() -> Result<(), String> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let quiet = remove_flag(&mut args, "--quiet");
    if args.is_empty() {
        return run_wizard();
    }
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print_help();
        return Ok(());
    }
    let command = args.remove(0);
    match command.as_str() {
        "status" => {
            let options = parse_options(args)?;
            let bootstrap = bootstrap_status(&options)?;
            let dev_env = dev_env_status(&options)?;
            print_status(&bootstrap, &dev_env);
        }
        "bootstrap" => run_bootstrap(args, quiet)?,
        "dev-env" | "devenv" => run_dev_env(args, quiet)?,
        other => {
            return Err(format!(
                "unknown command '{other}'\nhelp: run `vapor-installer --help`"
            ));
        }
    }
    Ok(())
}

fn run_wizard() -> Result<(), String> {
    let app_root = resolve_wizard_app_root()?;

    loop {
        clear_screen();
        print_wizard_frame(
            &bootstrap_status(&InstallerOptions {
                app_root: Some(app_root.clone()),
                dry_run: false,
            })?,
            &dev_env_status(&InstallerOptions {
                app_root: Some(app_root.clone()),
                dry_run: false,
            })?,
        );

        match prompt("Select an action")?.trim() {
            "1" => {
                preview_report(
                    "Runtime Bootstrap",
                    &bootstrap_install(&InstallerOptions {
                        app_root: Some(app_root.clone()),
                        dry_run: true,
                    })?,
                );
                if confirm("Install/reconcile runtime bootstrap now?")? {
                    let report = bootstrap_install(&InstallerOptions {
                        app_root: Some(app_root.clone()),
                        dry_run: false,
                    })?;
                    print_report("Runtime Bootstrap", &report);
                    pause()?;
                }
            }
            "2" => {
                preview_report(
                    "Development Environment",
                    &dev_env_install(&InstallerOptions {
                        app_root: Some(app_root.clone()),
                        dry_run: true,
                    })?,
                );
                if confirm("Install/reconcile development environment now?")? {
                    let report = dev_env_install(&InstallerOptions {
                        app_root: Some(app_root.clone()),
                        dry_run: false,
                    })?;
                    print_report("Development Environment", &report);
                    pause()?;
                }
            }
            "3" => {
                let options = BootstrapUninstallOptions {
                    app_root: Some(app_root.clone()),
                    dry_run: true,
                    include_content_cache: false,
                };
                preview_report("Remove Runtime Bootstrap", &bootstrap_uninstall(&options)?);
                if confirm("Remove runtime bootstrap tooling and generated installer state?")? {
                    let report = bootstrap_uninstall(&BootstrapUninstallOptions {
                        app_root: Some(app_root.clone()),
                        dry_run: false,
                        include_content_cache: false,
                    })?;
                    print_report("Remove Runtime Bootstrap", &report);
                    pause()?;
                }
            }
            "4" => {
                preview_report(
                    "Remove Development Environment",
                    &dev_env_uninstall(&InstallerOptions {
                        app_root: Some(app_root.clone()),
                        dry_run: true,
                    })?,
                );
                if confirm("Remove Rust/Cargo and cross-build development tooling?")? {
                    let report = dev_env_uninstall(&InstallerOptions {
                        app_root: Some(app_root.clone()),
                        dry_run: false,
                    })?;
                    print_report("Remove Development Environment", &report);
                    pause()?;
                }
            }
            "5" => {
                println!();
                print_status(
                    &bootstrap_status(&InstallerOptions {
                        app_root: Some(app_root.clone()),
                        dry_run: false,
                    })?,
                    &dev_env_status(&InstallerOptions {
                        app_root: Some(app_root.clone()),
                        dry_run: false,
                    })?,
                );
                pause()?;
            }
            "q" | "Q" | "x" | "X" => return Ok(()),
            _ => {
                println!("Unknown selection.");
                pause()?;
            }
        }
    }
}

fn resolve_wizard_app_root() -> Result<PathBuf, String> {
    match bootstrap_status(&InstallerOptions::default()) {
        Ok(status) => Ok(status.app_root().to_path_buf()),
        Err(error) => {
            clear_screen();
            println!("╔══════════════════════════════════════════════════════════════╗");
            println!("║                      Vapor Installer                       ║");
            println!("╚══════════════════════════════════════════════════════════════╝");
            println!();
            println!("App root");
            println!("  I could not infer the Steam app root automatically.");
            println!("  {error}");
            println!();
            println!("Enter the Steam app root path.");
            let value = prompt("App root")?;
            let app_root = PathBuf::from(value.trim());
            let status = bootstrap_status(&InstallerOptions {
                app_root: Some(app_root),
                dry_run: false,
            })?;
            Ok(status.app_root().to_path_buf())
        }
    }
}

fn run_bootstrap(mut args: Vec<String>, quiet: bool) -> Result<(), String> {
    let Some(command) = take_subcommand(&mut args) else {
        return Err("missing bootstrap subcommand: status, install, or uninstall".to_owned());
    };
    match command.as_str() {
        "status" => {
            let options = parse_options(args)?;
            print_bootstrap_status(&bootstrap_status(&options)?);
        }
        "install" => {
            let options = parse_options(args)?;
            let report = bootstrap_install(&options)?;
            if !quiet {
                print_report("Bootstrap Install", &report);
            }
        }
        "uninstall" => {
            let options = parse_bootstrap_uninstall_options(args)?;
            let report = bootstrap_uninstall(&options)?;
            if !quiet {
                print_report("Bootstrap Uninstall", &report);
            }
        }
        other => return Err(format!("unknown bootstrap subcommand '{other}'")),
    }
    Ok(())
}

fn run_dev_env(mut args: Vec<String>, quiet: bool) -> Result<(), String> {
    let Some(command) = take_subcommand(&mut args) else {
        return Err("missing dev-env subcommand: status, install, or uninstall".to_owned());
    };
    match command.as_str() {
        "status" => {
            let options = parse_options(args)?;
            print_dev_env_status(&dev_env_status(&options)?);
        }
        "install" => {
            let options = parse_options(args)?;
            let report = dev_env_install(&options)?;
            if !quiet {
                print_report("Development Environment Install", &report);
            }
        }
        "uninstall" => {
            let options = parse_options(args)?;
            let report = dev_env_uninstall(&options)?;
            if !quiet {
                print_report("Development Environment Uninstall", &report);
            }
        }
        other => return Err(format!("unknown dev-env subcommand '{other}'")),
    }
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

fn parse_options(args: Vec<String>) -> Result<InstallerOptions, String> {
    let mut app_root = None;
    let mut dry_run = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--app-root" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--app-root requires a path".to_owned())?;
                app_root = Some(PathBuf::from(value));
            }
            "--dry-run" => dry_run = true,
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(InstallerOptions { app_root, dry_run })
}

fn parse_bootstrap_uninstall_options(
    args: Vec<String>,
) -> Result<BootstrapUninstallOptions, String> {
    let mut app_root = None;
    let mut dry_run = false;
    let mut include_content_cache = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--app-root" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--app-root requires a path".to_owned())?;
                app_root = Some(PathBuf::from(value));
            }
            "--dry-run" => dry_run = true,
            "--include-content-cache" => include_content_cache = true,
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(BootstrapUninstallOptions {
        app_root,
        dry_run,
        include_content_cache,
    })
}

fn print_help() {
    println!("Vapor Installer");
    println!();
    println!("Run `vapor-installer` with no arguments to open the visual installer.");
    println!();
    println!("Usage");
    println!("  vapor-installer status [--app-root PATH]");
    println!("  vapor-installer bootstrap status [--app-root PATH]");
    println!("  vapor-installer bootstrap install [--app-root PATH] [--dry-run]");
    println!(
        "  vapor-installer bootstrap uninstall [--app-root PATH] [--dry-run] [--include-content-cache]"
    );
    println!("  vapor-installer dev-env status [--app-root PATH]");
    println!("  vapor-installer dev-env install [--app-root PATH] [--dry-run]");
    println!("  vapor-installer dev-env uninstall [--app-root PATH] [--dry-run]");
    println!();
    println!("Global options");
    println!("  --quiet  Suppress success output for launch-time installer calls.");
    println!();
    println!("Notes");
    println!(
        "  Bootstrap prepares app-local Git, SteamCMD, Vapor-Registry, and disposable app-root state."
    );
    println!("  Development environment install/uninstall is explicit and installer-owned.");
    println!("  Log: <app-root>/{INSTALLER_LOG}");
}

fn print_wizard_frame(bootstrap: &BootstrapStatus, dev_env: &DevEnvStatus) {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║                      Vapor Installer                       ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!("App root");
    println!("  {}", bootstrap.app_root().display());
    println!();
    println!("Readiness");
    println!(
        "  Runtime bootstrap:       {}",
        readiness_label(bootstrap.ready())
    );
    println!(
        "    Git:                   {}",
        readiness_label(bootstrap.git().ready())
    );
    println!(
        "    SteamCMD:              {}",
        readiness_label(bootstrap.steamcmd().ready())
    );
    println!(
        "    Vapor-Registry:        {}",
        readiness_label(bootstrap.registry().ready())
    );
    println!(
        "    Generated directories: {}",
        readiness_label(bootstrap.directories().ready())
    );
    println!(
        "  Development environment: {}",
        readiness_label(dev_env.ready())
    );
    println!(
        "    Rust/Cargo:            {}",
        readiness_label(dev_env.rust().ready())
    );
    println!(
        "    Cross-build tools:     {}",
        readiness_label(dev_env.cross().ready())
    );
    println!();
    println!("Actions");
    println!("  [1] Install / update runtime bootstrap");
    println!("  [2] Install / update development environment");
    println!("  [3] Uninstall runtime bootstrap");
    println!("  [4] Uninstall development environment");
    println!("  [5] Show detailed status");
    println!("  [Q] Exit");
    println!();
    println!("Policy");
    println!("  The Steam app root is disposable. Reinstalling the app is safe by design.");
    println!("  User progress and authority must live outside the app root.");
    println!("  Log: <app-root>/{INSTALLER_LOG}");
    println!();
}

fn print_status(bootstrap: &BootstrapStatus, dev_env: &DevEnvStatus) {
    println!("Vapor Installer Status");
    println!();
    print_bootstrap_status(bootstrap);
    println!();
    print_dev_env_status(dev_env);
}

fn print_bootstrap_status(status: &BootstrapStatus) {
    println!("Bootstrap");
    println!("  App root: {}", status.app_root().display());
    println!("  Ready: {}", yes_no(status.ready()));
    print_component(status.git());
    print_component(status.steamcmd());
    print_component(status.registry());
    print_component(status.directories());
    if !status.ready() {
        println!(
            "  Next: vapor-installer bootstrap install --app-root {}",
            status.app_root().display()
        );
    }
}

fn print_dev_env_status(status: &DevEnvStatus) {
    println!("Development Environment");
    println!("  App root: {}", status.app_root().display());
    println!("  Ready: {}", yes_no(status.ready()));
    print_component(status.rust());
    print_component(status.cross());
    if !status.ready() {
        println!(
            "  Next: vapor-installer dev-env install --app-root {}",
            status.app_root().display()
        );
    }
}

fn print_component(status: &ComponentStatus) {
    println!("  {}: {}", status.label(), yes_no(status.ready()));
    println!("    path: {}", status.path().display());
    for missing in status.missing() {
        println!("    missing: {missing}");
    }
}

fn print_report(title: &str, report: &InstallerReport) {
    println!("{title}");
    println!();
    println!("Status");
    println!("  App root: {}", report.app_root().display());
    println!(
        "  Mode: {}",
        if report.dry_run() {
            "dry-run"
        } else {
            "applied"
        }
    );
    println!("  Log: {}", report.app_root().join(INSTALLER_LOG).display());
    println!();
    println!("Actions");
    for action in report.actions() {
        println!("  - {action}");
    }
}

fn preview_report(title: &str, report: &InstallerReport) {
    println!();
    print_report(&format!("{title} Preview"), report);
    println!();
}

fn clear_screen() {
    print!("\x1b[2J\x1b[H");
    let _ = io::stdout().flush();
}

fn prompt(label: &str) -> Result<String, String> {
    print!("{label}> ");
    io::stdout()
        .flush()
        .map_err(|error| format!("failed to flush stdout: {error}"))?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|error| format!("failed to read input: {error}"))?;
    Ok(input)
}

fn confirm(question: &str) -> Result<bool, String> {
    println!();
    let answer = prompt(&format!("{question} [y/N]"))?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "YES" | "Yes"))
}

fn pause() -> Result<(), String> {
    println!();
    let _ = prompt("Press Enter to continue")?;
    Ok(())
}

fn readiness_label(value: bool) -> &'static str {
    if value { "ready" } else { "needs action" }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
