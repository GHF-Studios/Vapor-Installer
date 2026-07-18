# Vapor Installer

Vapor Installer owns installation and uninstallation mechanics for the Steam app
root. Vapor Shell remains the interactive product surface; installer operations
stay encapsulated here.

The app root is disposable. Installer-managed state is recreateable app-local
tooling, cache, logs, receipts, and bootstrap metadata. Authoritative user
progress or account data must live outside the Steam app root in OS-appropriate
user-data locations.

## User experience

Run `vapor-installer` with no arguments to open the visual installer. The
installer is the user-facing lifecycle surface for install, uninstall,
development-environment setup, and future upgrade/downgrade flows. Steam launch
wrappers call only the narrow headless bootstrap command so ordinary launches do
not ask testers to make setup decisions.

## Headless command shape

```text
vapor-installer status [--app-root PATH]
vapor-installer bootstrap status [--app-root PATH]
vapor-installer bootstrap install [--app-root PATH] [--dry-run]
vapor-installer bootstrap uninstall [--app-root PATH] [--dry-run] [--include-content-cache]
vapor-installer dev-env status [--app-root PATH]
vapor-installer dev-env install [--app-root PATH] [--dry-run]
vapor-installer dev-env uninstall [--app-root PATH] [--dry-run]
```

`bootstrap install` is the normal closed-alpha runtime preparation path. It is
safe to run repeatedly and prepares only basic functionality:

- app-local Git under `tools/git`;
- app-local SteamCMD under `tools/steamcmd`;
- app-local Vapor-Registry checkout under `.vapor/registry`;
- app-local disposable state, log, diagnostics, and content-cache directories.

`dev-env install` is the explicit development environment path. It first ensures
bootstrap readiness, then installs the Rust/Cargo and cross-build toolchain
needed for building Vapor projects. Normal testers should not need it.

`bootstrap uninstall` removes installer-owned generated bootstrap state,
including `tools/git`, `tools/steamcmd`, `.vapor/registry`, downloads, extracts,
and receipts. It does not remove user-authored source checkouts outside the app
root.

## Source layout

The crate is split by installer responsibility:

- `cli`: command-line parser and visual TUI adapter.
- `model`: public status/options/report value types.
- `app_root`: app-root discovery and `[root]` manifest checks.
- `bootstrap`: minimal runtime bootstrap install/uninstall/status.
- `dev_env`: explicit Rust/Cargo and cross-build development tooling.
- `git`: app-local Git acquisition and health checks.
- `acquire`: download, archive extraction, and checksum helpers.
- `fsutil`: app-root-contained filesystem mutation helpers.
- `paths`: shared app-local path conventions.

`src/main.rs` is intentionally only the process boundary; installer behavior
lives in the library modules.
