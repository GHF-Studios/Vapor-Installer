#[cfg(unix)]
use std::path::Path;
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use vapor_installer::{InstallerOptions, dev_env_uninstall, install, uninstall};
#[cfg(unix)]
use vapor_installer::{player_install, player_uninstall};

struct TestTree {
    root: PathBuf,
}

impl TestTree {
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "vapor-installer-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create test root");
        Self { root }
    }

    fn app_root(&self) -> PathBuf {
        let app_root = self.root.join("app");
        fs::create_dir_all(&app_root).expect("create app root");
        fs::write(app_root.join("App.vapor.toml"), "[root]\n").expect("write root manifest");
        app_root
    }
}

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn install_dry_run_does_not_mutate_app_root() {
    let tree = TestTree::new("install-dry-run");
    let app_root = tree.app_root();

    let report = install(&InstallerOptions {
        app_root: Some(app_root.clone()),
        dry_run: true,
    })
    .expect("dry-run install");

    assert!(report.dry_run());
    assert!(!app_root.join(".vapor").exists());
    assert!(report.actions().iter().any(|action| action.contains("Git")));
    assert!(
        report
            .actions()
            .iter()
            .any(|action| action.contains("SteamCMD"))
    );
}

#[cfg(unix)]
#[test]
fn install_creates_dirs_when_tools_are_already_present() {
    let tree = TestTree::new("install-dirs");
    let app_root = tree.app_root();
    write_registry_checkout(&app_root);
    write_tool(&app_root.join("tools/git/bin/git"));
    write_tool(&app_root.join("tools/steamcmd/steamcmd"));

    let report = player_install(&InstallerOptions {
        app_root: Some(app_root.clone()),
        dry_run: false,
    })
    .expect("player install");

    assert!(!report.dry_run());
    for relative in [
        ".vapor/state",
        ".vapor/state/installer",
        ".vapor/logs",
        ".vapor/diagnostics/runs",
        ".vapor/downloads",
        "content/cache/packages",
        "content/installed",
        "content/workshop/downloads",
        "tools",
    ] {
        assert!(app_root.join(relative).is_dir(), "missing {relative}");
    }
}

#[test]
fn dev_env_uninstall_dry_run_keeps_basic_tools() {
    let tree = TestTree::new("dev-env-dry-run");
    let app_root = tree.app_root();
    fs::create_dir_all(app_root.join("tools/git/bin")).expect("create git bin");
    fs::create_dir_all(app_root.join("tools/steamcmd")).expect("create steamcmd dir");
    fs::write(app_root.join("tools/git/bin/git"), "").expect("write git marker");
    fs::write(app_root.join("tools/steamcmd/steamcmd"), "").expect("write steamcmd marker");

    let report = dev_env_uninstall(&InstallerOptions {
        app_root: Some(app_root.clone()),
        dry_run: true,
    })
    .expect("dry-run dev-env uninstall");

    assert!(report.dry_run());
    assert!(app_root.join("tools/git/bin/git").exists());
    assert!(app_root.join("tools/steamcmd/steamcmd").exists());
    assert!(
        report
            .actions()
            .iter()
            .all(|action| !action.contains("tools/git") && !action.contains("tools/steamcmd"))
    );
}

#[cfg(unix)]
#[test]
fn player_uninstall_removes_symlink_without_deleting_target() {
    use std::os::unix::fs::symlink;

    let tree = TestTree::new("player-symlink");
    let app_root = tree.app_root();
    let external = tree.root.join("external-registry");
    fs::create_dir_all(&external).expect("create external target");
    fs::write(external.join("marker.txt"), "keep").expect("write external marker");
    fs::create_dir_all(app_root.join(".vapor")).expect("create vapor dir");
    symlink(&external, app_root.join(".vapor/registry")).expect("create registry symlink");

    let report = player_uninstall(&InstallerOptions {
        app_root: Some(app_root.clone()),
        dry_run: false,
    })
    .expect("player uninstall");

    assert!(!report.dry_run());
    assert!(!app_root.join(".vapor/registry").exists());
    assert!(external.join("marker.txt").is_file());
}

#[test]
fn uninstall_dry_run_covers_all_installer_managed_state() {
    let tree = TestTree::new("uninstall-scope");
    let app_root = tree.app_root();

    let report = uninstall(&InstallerOptions {
        app_root: Some(app_root),
        dry_run: true,
    })
    .expect("dry-run uninstall");

    let actions = report.actions().join("\n");
    for expected in [
        "rustup",
        "rustup-home",
        "cargo-home",
        "tools/zig",
        "tools/llvm-mingw",
        "tools/cross",
        "tools/git",
        "tools/steamcmd",
        ".vapor/registry",
        ".vapor/downloads",
        ".vapor/extract",
        ".vapor/state",
        ".vapor/diagnostics",
        ".vapor/logs",
        "content/cache",
        "content/installed",
        "content/workshop/downloads",
        "output",
    ] {
        assert!(actions.contains(expected), "missing {expected}: {actions}");
    }
    for depot_owned in [
        "bin/x86_64",
        "docs",
        "examples",
        "bin/vapor-launch",
        "resources/vapor",
    ] {
        assert!(
            !actions.contains(depot_owned),
            "should not remove depot-owned {depot_owned}: {actions}"
        );
    }
}

#[cfg(unix)]
fn write_tool(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create tool parent");
    }
    fs::write(path, "#!/bin/sh\nexit 0\n").expect("write tool");
    let mut permissions = fs::metadata(path).expect("tool metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make tool executable");
}

#[cfg(unix)]
fn write_registry_checkout(app_root: &Path) {
    let registry = app_root.join(".vapor/registry");
    fs::create_dir_all(registry.join(".git")).expect("create registry git dir");
    fs::write(registry.join("Registry.vapor.toml"), "[registry]\n")
        .expect("write registry manifest");
}
