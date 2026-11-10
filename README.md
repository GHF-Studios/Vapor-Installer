# Vapor Installer

`vapor-installer-gui` is the first-run installer and maintenance UI for the
standalone Vapor home. A release bundle contains `Vapor-Standalone.toml`,
target-specific executables under `bin/<target>/`, and shared read-only release
assets. The installer copies that allowlisted payload into the default Vapor
home, writes `Vapor-Home.toml`, and then guides the user through Player or
Developer setup.

Vapor home is separate from the Loo-Cast Steam app. Steam does not install,
update, or uninstall Vapor; removing Loo-Cast leaves Vapor and its data in place.
The Steam handoff only locates Vapor and passes its own product directory as
context.

Repair and uninstall operate on Vapor-owned files and machine integration.
Uninstall preserves the user's `data`, `sources`, `content`, and cache unless
the user separately chooses to remove them. It never deletes Steam-owned
product files or a locally installed game.

The command-line `vapor-installer` remains the maintenance adapter. Shared
setup and workflow implementations are compiled from
`resources/vapor/tools/` so first-run work does not require Rust or
`rust-script` to already be installed.
