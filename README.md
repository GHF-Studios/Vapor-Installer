# Vapor Installer

Vapor Installer owns installation and uninstallation mechanics for the Steam app
root. Vapor Shell remains the interactive product surface; installer operations
stay encapsulated here.

The app root is disposable. Installer-managed state is recreateable app-local
tooling, cache, logs, receipts, and install metadata. Authoritative user
progress or account data must live outside the Steam app root in OS-appropriate
user-data locations.

## User experience

Run `vapor-installer` with no arguments to open the visual installer. The
installer is the user-facing lifecycle surface for install, uninstall, and
developer-mode upgrade/downgrade flows. Steam launch wrappers call only the
narrow headless install command so ordinary launches do not ask testers to make
setup decisions.

## Headless command shape

```text
vapor-installer status [--app-root PATH]
vapor-installer install [--app-root PATH] [--dry-run]
vapor-installer uninstall [--app-root PATH] [--dry-run]
vapor-installer dev-env status [--app-root PATH]
vapor-installer dev-env install [--app-root PATH] [--dry-run]
vapor-installer dev-env uninstall [--app-root PATH] [--dry-run]
```

`install` is the normal closed-alpha player-mode preparation path. It is safe to
run repeatedly and prepares only basic functionality:

- app-local Git under `tools/git`;
- app-local SteamCMD under `tools/steamcmd`;
- app-local Vapor-Registry checkout under `.vapor/registry`;
- app-local disposable state, log, diagnostics, and content-cache directories.

`dev-env install` is the explicit developer-mode upgrade path. It first ensures
player-mode readiness, then installs the Rust/Cargo and cross-build toolchain
needed for building Vapor projects. Normal testers should not need it.

`dev-env uninstall` is the developer-mode downgrade path. It removes Rust/Cargo
and cross-build tooling while keeping player mode installed.

## Uninstall model

Uninstall is intentionally split between installer-owned state and Steam-owned
files:

1. Optional: downgrade developer mode back to player mode without uninstalling
   the app:

   ```text
   vapor-installer dev-env uninstall --app-root /path/to/steam/app
   ```

2. Remove every installer-managed mutable app-root path:

   ```text
   vapor-installer uninstall --app-root /path/to/steam/app
   ```

   This removes Rust/Cargo and cross-build tooling if present, app-local Git,
   SteamCMD, `.vapor/registry`, downloads/extracts, `.vapor/state`,
   `.vapor/diagnostics`, `.vapor/logs`, `bin/.vapor`, generated `content/`
   state, and `output/`. It does not remove depot-owned shell binaries, docs,
   examples, launch wrappers, scripts, or `Vapor.toml`.

3. Use Steam's uninstall feature to remove the depot-owned application files,
   including Vapor Shell, docs, launch wrappers, and `vapor-installer` itself.

No uninstall command recurses outside the resolved `[root]` app root.

## Source layout

The crate is split by installer responsibility:

- `cli`: command-line parser and visual TUI adapter.
- `model`: public status/options/report value types.
- `app_root`: app-root discovery and `[root]` manifest checks.
- `bootstrap`: internal player-mode install/uninstall/status implementation.
- `dev_env`: explicit Rust/Cargo and cross-build development tooling.
- `git`: app-local Git acquisition and health checks.
- `acquire`: download, archive extraction, and checksum helpers.
- `fsutil`: app-root-contained filesystem mutation helpers.
- `paths`: shared app-local path conventions.

`src/main.rs` is intentionally only the process boundary; installer behavior
lives in the library modules.
