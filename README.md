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
  -> in-process player setup

vapor-installer uninstall [--app-root PATH]
  -> in-process player teardown

vapor-installer erase-all [--app-root PATH] --confirm
  -> preflighted deletion of Vapor User Data and every source repository
     registered in the canonical Superworkspace; unrecognized neighboring
     files and the Steam-owned Loo-Cast depot are preserved

vapor-installer dev-env status
  -> in-process app-root status

vapor-installer dev-env install [--app-root PATH]
  -> in-process development provider setup

vapor-installer dev-env uninstall [--app-root PATH]
  -> in-process development provider teardown

vapor-installer ide patch-rustrover --super-workspace PATH [--app-root PATH]
  -> in-process RustRover project patch

vapor-installer ide run --workflow WORKFLOW --super-workspace PATH [--app-root PATH]
  -> in-process RustRover workflow launcher
```

`post-install` and `pre-uninstall` are also delegated to the matching lifecycle
anchor scripts.

Erase-all is an explicit permanent-data-removal path. Without `--confirm`, the
CLI prints the affected roots and performs no deletion. The Installer UI has a
separate **Erase all Vapor data** action; ordinary uninstall preserves source
and User Data. This is not yet the standalone install/update/version manager.

## Runtime shape

For normal installed-app use, `vapor-installer` is the stable precompiled
adapter. For local development, the Rust scripts can still expose the same
implementation directly when Rust is already available.
