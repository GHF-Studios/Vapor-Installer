# Vapor Installer

`vapor-installer` is now a compatibility shim.

The actual setup, teardown, app-root, and toolchain operations live in the
super-workspace script layer:

```text
tools/production/app_setup/
```

This keeps DevOps behavior visible, scriptable, and easy to replace. The binary
exists so Steam launch wrappers and older command habits still have a stable
entrypoint. For shipped runtime use, the shim compiles the shared script
implementation directly so first-run setup does not require Rust or
`rust-script` to already be installed.

## Command mapping

```text
vapor-installer status
  -> rust-script --force tools/production/app_setup/status.rs

vapor-installer install [--app-root PATH]
  -> rust-script --force tools/production/app_setup/setup_player.rs [--app-root PATH]

vapor-installer uninstall [--app-root PATH]
  -> rust-script --force tools/production/app_setup/teardown_player.rs [--app-root PATH]

vapor-installer dev-env status
  -> rust-script --force tools/production/app_setup/status.rs

vapor-installer dev-env install [--app-root PATH]
  -> rust-script --force tools/production/app_setup/setup_development.rs [--app-root PATH]

vapor-installer dev-env uninstall [--app-root PATH]
  -> rust-script --force tools/production/app_setup/teardown_development.rs [--app-root PATH]
```

`post-install` and `pre-uninstall` are also delegated to the matching lifecycle
anchor scripts.

## Runtime shape

For local development, the direct script command is preferred. For launchers and
Steam entrypoints, `vapor-installer` remains a small stable adapter that calls
the same `tools/_shared/vapor_tools.rs` implementation in-process.
