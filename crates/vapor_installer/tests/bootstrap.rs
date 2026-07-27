#[test]
fn installer_help_declares_script_ownership() {
    let help = vapor_installer::cli::help_text();

    assert!(help.contains("Vapor Installer Shim"));
    assert!(help.contains("tools/production/app_setup"));
    assert!(help.contains("setup_player.rs"));
    assert!(help.contains("teardown_player.rs"));
}
