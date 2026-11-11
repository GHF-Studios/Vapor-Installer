# Vapor Installer

`vapor-installer` is the precompiled app-root maintenance and workflow shim.
`vapor_installer_ui` is the Installer product module hosted by Vapor-GUI; its
normal status, install, repair, and uninstall actions call Vapor's shared
operations directly.

The setup, teardown, app-root, toolchain, and RustRover workflow-launch
operations use the shared app-root tool implementation:

```text
resources/vapor/tools/
```

The source-visible Rust scripts remain useful development affordances, but the
shipped binary compiles the same implementation directly. First-run setup,
maintenance, and RustRover workflow launch therefore do not require Rust or
`rust-script` to already be installed.

## Command mapping

```text
vapor-installer status
  -> in-process app-root status

vapor-installer install [--app-root PATH]
  -> register user VAPOR_HOME/PATH, then prepare player setup

vapor-installer uninstall [--app-root PATH]
  -> remove machine integration while preserving Vapor Home and authored source

vapor-installer dev-env status
  -> in-process app-root status

vapor-installer dev-env install [--app-root PATH]
  -> in-process development provider setup

vapor-installer dev-env uninstall [--app-root PATH]
  -> in-process development provider teardown

vapor-installer erase inventory --app-root PATH
  -> list the exact Vapor-owned paths that a full erasure would remove

vapor-installer erase all --app-root PATH --confirm "ERASE VAPOR"
  -> remove command registration, then hand deletion to an external helper
     that waits for the installer process to exit before removing Vapor Home

vapor-installer ide patch-rustrover --super-workspace PATH [--app-root PATH]
  -> in-process RustRover project patch

vapor-installer ide run --workflow WORKFLOW --super-workspace PATH [--app-root PATH]
  -> in-process RustRover workflow launcher
```

`post-install` and `pre-uninstall` are also delegated to the matching lifecycle
anchor scripts.

Full erasure requires an explicit app root and the exact confirmation phrase.
The inventory includes only the canonical Vapor Home and existing known Vapor
state roots; Steam game files and user-managed Rust/Cargo installations outside
Vapor Home are preserved. On Windows and Linux, deletion is handed to a
short-lived helper outside Vapor Home so the running installer never deletes
its own executable.

## Runtime shape

For normal installed-app use, `vapor-installer` is the stable precompiled
adapter. For local development, the Rust scripts can still expose the same
implementation directly when Rust is already available.
