# Vapor Installer

`vapor-installer` is the precompiled recovery and product-lifecycle command.
`vapor_installer_ui` is the Installer product module hosted by Vapor-GUI; its
legacy app-root status, player setup/repair, and teardown actions call Vapor's
shared operations directly. Standalone version acquisition and activation are
separate commands and controls; standalone repair and removal still need to be
routed through a stable bootstrapper outside the versioned app directory.

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

vapor-installer toolchain install
  -> install Vapor-managed Rust/Cargo and cross-build tools in Vapor User Data

vapor-installer dev-env uninstall [--app-root PATH]
  -> in-process development provider teardown

vapor-installer ide patch-rustrover --super-workspace PATH [--app-root PATH]
  -> in-process RustRover project patch

vapor-installer ide run --workflow WORKFLOW --super-workspace PATH [--app-root PATH]
  -> in-process RustRover workflow launcher
```

`post-install` and `pre-uninstall` are also delegated to the matching lifecycle
anchor scripts.

`toolchain install` is the standalone and automation path for a build host. It
does not write into a versioned Vapor App Instance and does not install
SteamCMD. Cargo, Rustup, Zig, LLVM-MinGW, and Vapor's cross-linker wrappers are
kept under the canonical Vapor User Data root. `source clone` remains the
Vapor-owned repository acquisition operation and accepts `--revision` with a
full commit SHA; submodules are initialized at the exact gitlinks recorded by
that revision. Without an explicit workspace path, source operations use the
canonical `~/Vapor` source home.

The signed standalone product lifecycle is being integrated into this same
Installer owner:

```text
vapor-installer app install [--channel stable]
vapor-installer app update [--channel stable]
vapor-installer app versions
vapor-installer app activate VERSION
```

It reads the signed Artifact manifest from the fixed Vapor HTTPS origin,
verifies exact manifest bytes with the public Ed25519 key pinned in this
binary, checks the downloaded archive's signed size and SHA-256, rejects
unsafe archive entries and game payload roots, and stages immutable versions
under canonical Vapor User Data before changing `active.version`. Installed
versions live under `releases/`; the active-version file selects what the
Steam creator handoff opens next. Version swaps do not modify the Steam app or
authored source checkouts.

The UI also lists locally installed versions and can select one for the next
launch. Artifact exposes a signed-release history index for older published
versions, but the UI does not yet use it for remote version selection.

The website bootstrap is a standalone `vapor-setup` binary. With no arguments
it opens the install/update GUI; `vapor-setup install [--channel stable] [--print-root]`
performs the same signed install headlessly for managed bootstrap automation.
It does not require Steam or another Vapor installation and is excluded from
Steam depots. The release bundle includes it as a separate immutable asset
whose size and SHA-256 are covered by the signed release manifest.

The standalone release archive has its own App Instance manifest. Its source
template is `Standalone-App.vapor.toml`; the release assembler places it in the
archive as `App.vapor.toml` with the release version. It must not reuse the
Steam-owned root `App.vapor.toml`, which contains Loo-Cast product metadata.

On Linux the product path is `$XDG_DATA_HOME/vapor/apps/vapor` (falling back
to `~/.local/share/vapor/apps/vapor`); on Windows it is
`%LOCALAPPDATA%/Vapor/apps/vapor`. Releases are stored under `releases/` and
the active version is an atomically replaced pointer. The setup binary is
compiled for Linux and Windows and included in locally signed `0.5.0` release
bundles. No production release is available to install yet.

Erase-all is an explicit permanent-data-removal path. The standalone form
(`vapor-installer app erase-all`) works even after the Vapor program has been
removed. Without `--confirm`, it prints the exact Vapor User Data, registered
source checkout, and workspace metadata paths and performs no deletion. The
Installer UI offers the same path with a reviewable confirmation. It preserves
unrecognized neighboring files and leaves Steam-managed Loo-Cast installed.
Ordinary standalone uninstall removes the program and its shell/environment
integration while preserving source and User Data. Publishing the signed
bundles, clean-install proof, and standalone runtime verification remain
unfinished.

## Runtime shape

The versioned app still ships a precompiled `vapor-installer` adapter. The
separate `vapor-setup` target is the website bootstrap download; it has been
compiled and included in locally signed bundles but is not published on
Artifact. For local development, Rust scripts expose the same app-root
implementation when Rust is available.
