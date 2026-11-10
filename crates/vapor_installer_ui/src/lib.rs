use eframe::egui;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use vapor_client::{Client, ClientHostConfig, service_sibling_path};
use vapor_client_protocol::{
    ClientAction, ClientEvent, ClientResponse, ClientResponseKind, DeveloperAction,
    InstallationRoleSnapshot, OperationStatus, ProtocolFrame,
};

pub struct InstallerApp {
    installation: Option<vapor_core::VaporInstallation>,
    bundle_root: Option<PathBuf>,
    page: InstallerPage,
    status: String,
    receiver: Option<mpsc::Receiver<Result<InstallerTaskResult, String>>>,
    confirm_uninstall: bool,
    role: InstallationRoleSnapshot,
    selected_role: vapor_core::VaporRole,
    first_launch: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum InstallerPage {
    #[default]
    Overview,
    Role,
    Installation,
}

struct InstallerTaskResult {
    message: String,
    role: Option<InstallationRoleSnapshot>,
}

impl InstallerApp {
    pub fn new(app_root: Option<PathBuf>, first_launch: bool) -> Self {
        let (installation, mut status) = match app_root {
            Some(root) => match vapor_core::VaporInstallation::from_explicit_root(root) {
                Ok(installation) => (
                    Some(installation),
                    if first_launch {
                        "First launch: choose your role and complete setup before playing."
                    } else {
                        "Ready to inspect or maintain this Vapor installation."
                    }
                    .to_owned(),
                ),
                Err(error) => (None, error.to_string()),
            },
            None => match vapor_core::VaporInstallation::discover() {
                Ok(installation) => (
                    Some(installation),
                    if first_launch {
                        "First launch: choose your role and complete setup before playing."
                    } else {
                        "Ready to inspect or maintain this Vapor installation."
                    }
                    .to_owned(),
                ),
                Err(error) => (None, error.to_string()),
            },
        };
        let bundle_root = standalone_bundle_root();
        let first_launch = first_launch || (installation.is_none() && bundle_root.is_some());
        if installation.is_none() && bundle_root.is_some() {
            status = "Ready to install Vapor into your user home.".to_owned();
        }
        let mut app = Self {
            status,
            installation,
            bundle_root,
            page: InstallerPage::Overview,
            receiver: None,
            confirm_uninstall: false,
            role: InstallationRoleSnapshot::default(),
            selected_role: vapor_core::VaporRole::Player,
            first_launch,
        };
        app.refresh_role();
        app
    }

    fn run_installer(&mut self, operation: InstallerOperation) {
        let installation = self.installation.clone();
        let bundle_root = self.bundle_root.clone();
        if installation.is_none()
            && !(matches!(operation, InstallerOperation::Install) && bundle_root.is_some())
        {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = format!("Running installer {}…", operation.label());
        thread::spawn(move || {
            let result = execute_installer_operation(
                operation,
                installation.as_ref(),
                bundle_root.as_deref(),
            )
            .map(|message| InstallerTaskResult {
                message,
                role: None,
            });
            let _ = sender.send(result);
        });
    }

    fn run_role_transition(&mut self, target: vapor_core::VaporRole, promote: bool) {
        let Some(installation) = self.installation.clone() else {
            return;
        };
        let home = installation.root.clone();
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = format!(
            "{} installation role…",
            if promote { "Preparing" } else { "Changing" }
        );
        thread::spawn(move || {
            let result = (|| {
                if promote {
                    vapor_core::install_installation_for(&installation)
                        .map_err(|error| error.to_string())?;
                    if !player_setup_ready_at(&installation.root) {
                        mark_player_setup_ready_at(&installation.root)?;
                    }
                }
                let action = if promote {
                    DeveloperAction::PromoteInstallationRole {
                        target: target.as_str().to_owned(),
                    }
                } else {
                    DeveloperAction::DemoteInstallationRole {
                        target: target.as_str().to_owned(),
                    }
                };
                let mut result = run_service_action(&home, action, true)?;
                if promote {
                    result.message.push_str(
                        "\nVapor commands are registered on your user PATH. Open a new terminal to use them.",
                    );
                }
                Ok(result)
            })();
            let _ = sender.send(result);
        });
    }

    fn run_first_launch_setup(&mut self) {
        let Some(installation) = self.installation.clone() else {
            return;
        };
        let target = self.selected_role;
        let home = installation.root.clone();
        let current = self
            .role
            .role
            .parse::<vapor_core::VaporRole>()
            .unwrap_or(vapor_core::VaporRole::Player);
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = format!("Setting up this installation for {target}…");
        thread::spawn(move || {
            let result = vapor_core::install_installation_for(&installation)
                .map_err(|error| error.to_string())
                .and_then(|_| mark_player_setup_ready_at(&installation.root))
                .and_then(|()| {
                    if target > current {
                        run_service_action(
                            &home,
                            DeveloperAction::PromoteInstallationRole {
                                target: target.as_str().to_owned(),
                            },
                            true,
                        )
                    } else if target < current {
                        run_service_action(
                            &home,
                            DeveloperAction::DemoteInstallationRole {
                                target: target.as_str().to_owned(),
                            },
                            true,
                        )
                    } else {
                        run_service_action(&home, DeveloperAction::RefreshInstallationRole, false)
                    }
                })
                .map(|mut result| {
                    result.message.push_str(
                        "\nVapor commands are registered on your user PATH. Open a new terminal to use them.",
                    );
                    result
                });
            let _ = sender.send(result);
        });
    }

    fn player_setup_ready(&self) -> bool {
        self.installation
            .as_ref()
            .is_some_and(|installation| player_setup_ready_at(&installation.root))
    }

    fn continue_to_vapor(&mut self, context: &egui::Context) {
        let Some(installation) = &self.installation else {
            self.status = "Vapor installation is unavailable.".to_owned();
            return;
        };
        let home = installation.root.clone();
        let gui = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(PathBuf::from))
            .map(|bin| bin.join(format!("vapor-gui{}", std::env::consts::EXE_SUFFIX)));
        let Some(gui) = gui.filter(|path| path.is_file()) else {
            self.status = "Vapor's app selector is missing; repair Vapor before opening it.".to_owned();
            return;
        };
        let mut command = Command::new(gui);
        command
            .args(["--mode", "chooser"])
            .current_dir(&home)
            .env("VAPOR_HOME", &home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        match command.spawn() {
            Ok(_) => context.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(error) => self.status = format!("Could not open Vapor: {error}"),
        }
    }

    fn draw_first_launch(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.heading("Welcome to Vapor");
        if self.installation.is_none() {
            ui.label("Install Vapor independently. The Loo-Cast Steam app remains a separate product installation.");
            ui.add_space(8.0);
            if ui
                .add_enabled(
                    self.receiver.is_none() && self.bundle_root.is_some(),
                    egui::Button::new("Install Vapor"),
                )
                .clicked()
            {
                self.run_installer(InstallerOperation::Install);
            }
            if self.bundle_root.is_none() {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    "This installer was not started from a valid standalone Vapor release bundle.",
                );
            }
            return;
        }
        ui.label("This copy was just installed. Finish the required Player or Developer setup before opening Vapor.");
        ui.add_space(10.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("Choose how you’ll use Vapor");
            ui.label("Player setup is included for every installation. Developer setup installs the complete required toolset; it cannot be skipped.");
            ui.add_enabled_ui(self.receiver.is_none(), |ui| {
                egui::ComboBox::from_id_salt("first-launch-role")
                    .selected_text(self.selected_role.display_name())
                    .show_ui(ui, |ui| {
                        for role in [vapor_core::VaporRole::Player, vapor_core::VaporRole::Developer] {
                            ui.selectable_value(&mut self.selected_role, role, role.display_name());
                        }
                    });
            });
            ui.label(role_description(self.selected_role));
            ui.add_space(8.0);
            if self.player_setup_ready() {
                let installed = self
                    .role
                    .role
                    .parse::<vapor_core::VaporRole>()
                    .unwrap_or(vapor_core::VaporRole::Player);
                let role_ready = installed == self.selected_role
                    && (self.selected_role == vapor_core::VaporRole::Player
                        || self.role.developer_tools_ready());
                if ui
                    .add_enabled(
                        self.receiver.is_none() && role_ready,
                        egui::Button::new("Open Vapor"),
                    )
                    .clicked()
                {
                    self.continue_to_vapor(ui.ctx());
                }
                if !role_ready && self.receiver.is_none() {
                    ui.label("Finish setup for the selected role before continuing.");
                    if installed != self.selected_role {
                        let promote = self.selected_role > installed;
                        if ui
                            .button(format!("Set up as {}", self.selected_role))
                            .clicked()
                        {
                            self.run_role_transition(self.selected_role, promote);
                        }
                    } else if self.selected_role.requires_developer_environment()
                        && ui.button("Repair Developer setup").clicked()
                    {
                        self.run_installer(InstallerOperation::Repair);
                    }
                }
            } else if ui
                .add_enabled(
                    self.receiver.is_none() && self.installation.is_some(),
                    egui::Button::new(format!("Set up as {}", self.selected_role)),
                )
                .clicked()
            {
                self.run_first_launch_setup();
            }
        });
        ui.add_space(10.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("Separate from Steam");
            ui.label("Vapor is installed independently under your user account. Removing the Loo-Cast Steam app does not uninstall Vapor or remove your Vapor data.");
        });
    }

    fn refresh_role(&mut self) {
        let Some(home) = self.installation.as_ref().map(|installation| installation.root.clone()) else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = "Checking installation capabilities…".to_owned();
        thread::spawn(move || {
            let result = run_service_action(&home, DeveloperAction::RefreshInstallationRole, false);
            let _ = sender.send(result);
        });
    }

    fn draw_installation_panel(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.heading("Vapor installation");
            ui.label("Manage the app on this device. Your downloaded content and projects stay separate.");
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.receiver.is_none() && self.installation.is_some(),
                        egui::Button::new("Repair installation"),
                    )
                    .clicked()
                {
                    self.run_installer(InstallerOperation::Repair);
                }
                if ui
                    .add_enabled(
                        self.receiver.is_none() && self.installation.is_some(),
                        egui::Button::new("Check health"),
                    )
                    .clicked()
                {
                    self.run_installer(InstallerOperation::Status);
                }
            });
            egui::CollapsingHeader::new("Advanced installation tools")
                .default_open(false)
                .show(ui, |ui| {
                    if let Some(installation) = &self.installation {
                        ui.label("Vapor home");
                        ui.monospace(installation.root.display().to_string());
                    }
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                self.receiver.is_none() && self.installation.is_some(),
                                egui::Button::new("Initialize or repair setup"),
                            )
                            .clicked()
                        {
                            self.run_installer(InstallerOperation::Install);
                        }
                        if ui
                            .add_enabled(
                                self.receiver.is_none() && self.installation.is_some(),
                                egui::Button::new("Uninstall Vapor"),
                            )
                            .clicked()
                        {
                            self.confirm_uninstall = true;
                        }
                    });
                });
        });
    }

    fn draw_role_panel(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.heading("Choose your role");
            ui.label(
                "Promote this installation when you want Vapor to prepare more tools for you.",
            );
            ui.add_space(8.0);
            let Some(current) = parsed_installation_role(&self.role) else {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    self.role
                        .error
                        .as_deref()
                        .unwrap_or("Vapor could not read this installation's role."),
                );
                if ui
                    .add_enabled(self.receiver.is_none(), egui::Button::new("Try again"))
                    .clicked()
                {
                    self.refresh_role();
                }
                return;
            };

            if let Some(error) = &self.role.error {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!("Role is {current}; developer-tool readiness is unavailable: {error}"),
                );
            }
            ui.strong(format!("Current role: {current}"));
            ui.add_enabled_ui(self.receiver.is_none(), |ui| {
                egui::ComboBox::from_id_salt("installer-role-target")
                    .selected_text(self.selected_role.display_name())
                    .show_ui(ui, |ui| {
                        for role in [
                            vapor_core::VaporRole::Player,
                            vapor_core::VaporRole::Developer,
                        ] {
                            ui.selectable_value(&mut self.selected_role, role, role.display_name());
                        }
                    });
            });
            ui.label(role_description(self.selected_role));
            let changing = self.selected_role != current;
            let action = if self.selected_role > current {
                "Promote"
            } else {
                "Demote"
            };
            if ui
                .add_enabled(
                    self.receiver.is_none() && changing,
                    egui::Button::new(if changing {
                        format!("{action} to {}", self.selected_role)
                    } else {
                        format!("Already set up as {current}")
                    }),
                )
                .clicked()
            {
                self.run_role_transition(self.selected_role, self.selected_role > current);
            }

            if current.requires_developer_environment() {
                ui.add_space(6.0);
                ui.strong("Developer tools");
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("Rust + Cargo: {}", readiness(self.role.rust_ready)));
                    ui.label(format!(
                        "Rust Analyzer: {}",
                        readiness(self.role.rust_analyzer_ready)
                    ));
                    ui.label(format!(
                        "Windows build tools: {}",
                        readiness(
                            self.role.windows_gnu_target_ready
                                && self.role.windows_gnu_linker_ready
                        )
                    ));
                    ui.label(format!(
                        "Linux build tools: {}",
                        readiness(
                            self.role.linux_gnu_target_ready && self.role.linux_gnu_linker_ready
                        )
                    ));
                    ui.label(format!(
                        "Tracy: {}",
                        readiness(self.role.tracy_profiler_ready && self.role.tracy_analysis_ready)
                    ));
                });
            }
            if current.requires_developer_environment()
                && !self.role.developer_tools_ready()
                && ui
                    .add_enabled(
                        self.receiver.is_none(),
                        egui::Button::new("Repair Developer setup"),
                    )
                    .clicked()
            {
                self.run_installer(InstallerOperation::Repair);
            }
        });
    }
}

fn player_setup_ready_at(app_root: &Path) -> bool {
    let receipt = app_root.join("data/state/installer/player.toml");
    std::fs::read_to_string(receipt).ok().is_some_and(|source| {
        source
            .lines()
            .any(|line| line.trim() == "state = \"ready\"")
    })
}

fn mark_player_setup_ready_at(home: &Path) -> Result<(), String> {
    let receipt = home.join("data/state/installer/player.toml");
    let parent = receipt
        .parent()
        .ok_or_else(|| "Vapor player setup receipt has no parent directory".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    std::fs::write(&receipt, "schema = 1\nstate = \"ready\"\n")
        .map_err(|error| format!("could not write {}: {error}", receipt.display()))
}

fn parsed_installation_role(snapshot: &InstallationRoleSnapshot) -> Option<vapor_core::VaporRole> {
    snapshot.role.parse().ok()
}

fn role_description(role: vapor_core::VaporRole) -> &'static str {
    match role {
        vapor_core::VaporRole::Player => {
            "Play compositions and install content. Developer tools are not required."
        }
        vapor_core::VaporRole::Developer => {
            "Use Vapor’s development workflows. Rust, both supported Rust targets, Linux and Windows cross-linkers, and Tracy are installed as part of this role."
        }
    }
}

fn run_service_action(
    home: &Path,
    action: DeveloperAction,
    operation: bool,
) -> Result<InstallerTaskResult, String> {
    let target = vapor_core::HostTarget::current()
        .map_err(|error| error.to_string())?
        .triple();
    let gui_executable = home
        .join("bin")
        .join(target)
        .join(format!("vapor-installer-gui{}", std::env::consts::EXE_SUFFIX));
    if !gui_executable.is_file() {
        return Err(format!(
            "Vapor Installer is missing from the selected Vapor home: {}",
            gui_executable.display()
        ));
    }
    let service_path = resolve_service_path(&gui_executable)?;
    let mut client = Client::connect_or_start(
        &service_path,
        ClientHostConfig {
            application_executable: gui_executable,
            workshop_app_id: None,
            workspace_root: None,
            active_composition_id: None,
            fixture_mode: false,
            legacy_storage: Default::default(),
            state_directory: None,
        },
    )
    .map_err(|error| error.to_string())?;
    let request = client
        .send(ClientAction::Developer(action), None)
        .map_err(|error| error.to_string())?;
    let response = await_response(&mut client, &request.0, Duration::from_secs(30))?;
    if let Some(error) = response.error {
        return Err(error.message);
    }
    if operation {
        if response.kind != vapor_client_protocol::ClientResponseKind::OperationAccepted {
            return Err("The service did not start the requested role change.".to_owned());
        }
        let operation_id = response.operation_id.ok_or_else(|| {
            "The service accepted the role change without an operation ID.".to_owned()
        })?;
        let deadline = Instant::now() + Duration::from_secs(20 * 60);
        loop {
            if Instant::now() >= deadline {
                return Err("Timed out while preparing installation tools.".to_owned());
            }
            match client.recv_timeout(Duration::from_secs(1)) {
                Ok(ProtocolFrame::Event(ClientEvent::OperationChanged(event)))
                    if event.id == operation_id =>
                {
                    match event.status {
                        OperationStatus::Succeeded => break,
                        OperationStatus::Failed { message } => return Err(message),
                        OperationStatus::Cancelled => {
                            return Err("Installation role change was cancelled.".to_owned());
                        }
                        OperationStatus::Running | OperationStatus::Cancelling => {}
                    }
                }
                Ok(_) | Err(vapor_client::ClientError::Timeout(_)) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        let refresh = client
            .send(
                ClientAction::Developer(DeveloperAction::RefreshInstallationRole),
                None,
            )
            .map_err(|error| error.to_string())?;
        let response = await_response(&mut client, &refresh.0, Duration::from_secs(30))?;
        if let Some(error) = response.error {
            return Err(error.message);
        }
        return Ok(task_result(&response));
    }
    Ok(task_result(&response))
}

fn resolve_service_path(application_executable: &std::path::Path) -> Result<PathBuf, String> {
    let service_path = service_sibling_path(application_executable);
    if !service_path.is_file() {
        return Err(format!(
            "Vapor Client service is missing beside the running app: {}. Repair or locally deploy Vapor Client.",
            service_path.display()
        ));
    }
    Ok(service_path)
}

fn task_result(response: &ClientResponse) -> InstallerTaskResult {
    let role = response.snapshot.installation_role.clone();
    let message = if let Some(error) = &role.error {
        format!("Role status unavailable: {error}")
    } else {
        format!("Installation role is {}.", role.role)
    };
    InstallerTaskResult {
        message,
        role: Some(role),
    }
}

fn sync_role_selection(selected: &mut vapor_core::VaporRole, snapshot: &InstallationRoleSnapshot) {
    if let Ok(role) = snapshot.role.parse() {
        *selected = role;
    }
}

fn await_response(
    client: &mut Client,
    request_id: &str,
    timeout: Duration,
) -> Result<ClientResponse, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            return Err("Timed out waiting for the Vapor service.".to_owned());
        }
        match client.recv_timeout(Duration::from_secs(1)) {
            Ok(ProtocolFrame::Response(response)) if response.request_id.0 == request_id => {
                if response.kind == ClientResponseKind::Rejected {
                    return Err(response.error.map_or_else(
                        || "The Vapor service rejected the request.".to_owned(),
                        |e| e.message,
                    ));
                }
                return Ok(*response);
            }
            Ok(_) | Err(vapor_client::ClientError::Timeout(_)) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn readiness(ready: bool) -> &'static str {
    if ready { "ready" } else { "not installed" }
}

#[derive(Clone, Copy)]
enum InstallerOperation {
    Status,
    Install,
    Repair,
    Uninstall,
}

impl InstallerOperation {
    fn label(self) -> &'static str {
        match self {
            Self::Status => "status check",
            Self::Install => "install",
            Self::Repair => "repair",
            Self::Uninstall => "uninstall",
        }
    }
}

fn execute_installer_operation(
    operation: InstallerOperation,
    installation: Option<&vapor_core::VaporInstallation>,
    bundle_root: Option<&Path>,
) -> Result<String, String> {
    match operation {
        InstallerOperation::Status => {
            let installation = installation
                .ok_or_else(|| "No Vapor installation is selected.".to_owned())?;
            let status = vapor_core::diagnose_managed_state_for(installation)
                .map_err(|error| error.to_string())?;
            Ok(format!(
                "Vapor home: {}\nHealthy: {}\nToolchain: {} ({})\nProjects: {}",
                status.installation_root.display(),
                status.is_healthy(),
                status.toolchain_version,
                if status.toolchain_installed {
                    "installed"
                } else {
                    "missing"
                },
                status.current_projects,
            ))
        }
        InstallerOperation::Install => {
            if let Some(installation) = installation {
                let report = vapor_core::install_installation_for(installation)
                    .map_err(|error| error.to_string())?;
                mark_player_setup_ready_at(&installation.root)?;
                return Ok(format!(
                    "Vapor setup is ready at {}\nSuperworkspace: {}",
                    report.installation_root.display(),
                    report.superworkspace_root.display(),
                ));
            }
            let bundle_root = bundle_root
                .ok_or_else(|| "A standalone Vapor release bundle is required.".to_owned())?;
            let home = vapor_core::VaporInstallation::default_home_root()
                .map_err(|error| error.to_string())?;
            let installed = vapor_core::VaporInstallation::install_from_bundle(bundle_root, &home)
                .map_err(|error| error.to_string())?;
            Ok(format!(
                "Vapor was installed at {}. Continue to choose Player or Developer setup.",
                installed.root.display(),
            ))
        }
        InstallerOperation::Repair => {
            let installation = installation
                .ok_or_else(|| "No Vapor installation is selected.".to_owned())?;
            let report = vapor_core::repair_managed_state_for(installation)
                .map_err(|error| error.to_string())?;
            Ok(format!(
                "Repair complete. Vapor home: {}\nHealthy: {}",
                report.status.installation_root.display(),
                report.status.is_healthy(),
            ))
        }
        InstallerOperation::Uninstall => {
            let installation = installation
                .ok_or_else(|| "No Vapor installation is selected.".to_owned())?;
            let report = vapor_core::uninstall_installation_for(
                installation,
                vapor_core::UninstallOptions::default(),
            )
            .map_err(|error| error.to_string())?;
            Ok(format!(
                "Vapor was removed from {}. User data and authored source were preserved.",
                report.installation_root.display(),
            ))
        }
    }
}

fn standalone_bundle_root() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let target_directory = executable.parent()?;
    let bin_directory = target_directory.parent()?;
    if bin_directory.file_name()? != "bin" {
        return None;
    }
    let root = bin_directory.parent()?;
    (root.join("Vapor-Standalone.toml").is_file()
        && root.join("metadata/toolchain.toml").is_file()
        && root.join("resources").is_dir())
    .then(|| root.to_path_buf())
}

impl eframe::App for InstallerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(result) = self
            .receiver
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok())
        {
            self.receiver = None;
            match result {
                Ok(result) => {
                    if let Some(role) = result.role {
                        sync_role_selection(&mut self.selected_role, &role);
                        self.role = role;
                    }
                    self.status = result.message;
                    if self.installation.is_none() {
                        self.installation = vapor_core::VaporInstallation::default_home_root()
                            .ok()
                            .and_then(|root| {
                                vapor_core::VaporInstallation::from_explicit_root(root).ok()
                            });
                    }
                }
                Err(error) => self.status = error,
            }
        }
        if self.first_launch {
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.draw_first_launch(ui));
            });
            egui::Panel::bottom("installer-first-launch-status").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(&self.status);
                    if self.receiver.is_some() {
                        ui.spinner();
                    }
                });
            });
        } else {
            egui::Panel::top("installer-navigation").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Vapor Installer");
                    ui.separator();
                    for (page, label) in [
                        (InstallerPage::Overview, "Overview"),
                        (InstallerPage::Role, "Your role"),
                        (InstallerPage::Installation, "Installation"),
                    ] {
                        if ui.selectable_label(self.page == page, label).clicked() {
                            self.page = page;
                        }
                    }
                });
            });
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| match self.page {
                    InstallerPage::Overview => {
                        ui.add_space(8.0);
                        ui.heading("Set up this device for Vapor");
                        ui.label("Choose how you use Vapor. You can change your role at any time.");
                        ui.add_space(18.0);
                        if ui.available_width() >= 820.0 {
                            ui.columns(2, |columns| {
                                self.draw_role_panel(&mut columns[0]);
                                self.draw_installation_panel(&mut columns[1]);
                            });
                        } else {
                            self.draw_role_panel(ui);
                            ui.add_space(12.0);
                            self.draw_installation_panel(ui);
                        }
                    }
                    InstallerPage::Role => {
                        ui.add_space(8.0);
                        ui.heading("Choose your role");
                        ui.label("Vapor prepares only the tools needed for the role you select.");
                        ui.add_space(12.0);
                        self.draw_role_panel(ui);
                    }
                    InstallerPage::Installation => {
                        ui.add_space(8.0);
                        ui.heading("Installation and recovery");
                        ui.label("Repair the app if it is incomplete, or inspect advanced local details.");
                        ui.add_space(12.0);
                        self.draw_installation_panel(ui);
                    }
                });
            });
            egui::Panel::bottom("installer-status").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(&self.status);
                    if self.receiver.is_some() {
                        ui.spinner();
                    }
                });
            });
        }
        if self.confirm_uninstall {
            egui::Window::new("Confirm uninstall")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label("This removes the independent Vapor installation and its machine integration. Steam-owned Loo-Cast files, locally installed games, and your user data are not removed by this action.");
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.confirm_uninstall = false;
                        }
                        if ui.button("Uninstall Vapor").clicked() {
                            self.confirm_uninstall = false;
                            self.run_installer(InstallerOperation::Uninstall);
                        }
                    });
                });
        }
        if self.receiver.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        InstallerApp, parsed_installation_role, player_setup_ready_at, resolve_service_path,
        sync_role_selection,
    };
    use vapor_client_protocol::InstallationRoleSnapshot;

    #[test]
    fn installer_keeps_an_explicit_root_as_its_operation_context() {
        let selected =
            std::env::temp_dir().join(format!("vapor-installer-ui-root-{}", std::process::id()));
        std::fs::create_dir_all(&selected).expect("create app root");
        std::fs::write(selected.join("App.vapor.toml"), "[root]\n").expect("write app manifest");

        let app = InstallerApp::new(Some(selected.clone()), false);

        assert_eq!(
            app.installation.expect("explicit installation").root,
            selected
        );
        std::fs::remove_dir_all(selected).expect("remove test app root");
    }

    #[test]
    fn first_launch_starts_on_the_player_role() {
        let selected =
            std::env::temp_dir().join(format!("vapor-installer-first-run-{}", std::process::id()));
        std::fs::create_dir_all(&selected).expect("create app root");
        std::fs::write(selected.join("App.vapor.toml"), "[root]\n").expect("write app manifest");

        let app = InstallerApp::new(Some(selected.clone()), true);

        assert!(app.first_launch);
        assert_eq!(app.selected_role, vapor_core::VaporRole::Player);
        std::fs::remove_dir_all(selected).expect("remove test app root");
    }

    #[test]
    fn installed_role_becomes_the_selected_role_instead_of_suggesting_a_demotion() {
        let mut selected = vapor_core::VaporRole::Player;
        let installed = InstallationRoleSnapshot {
            role: "developer".to_owned(),
            ..Default::default()
        };

        sync_role_selection(&mut selected, &installed);

        assert_eq!(selected, vapor_core::VaporRole::Developer);
    }

    #[test]
    fn readiness_failure_does_not_hide_role_promotion_choices() {
        let snapshot = InstallationRoleSnapshot {
            role: "player".to_owned(),
            error: Some("toolchain metadata is unavailable".to_owned()),
            ..Default::default()
        };

        assert_eq!(
            parsed_installation_role(&snapshot),
            Some(vapor_core::VaporRole::Player)
        );
    }

    #[test]
    fn unreadable_role_still_requires_refresh() {
        let snapshot = InstallationRoleSnapshot {
            error: Some("role state is unreadable".to_owned()),
            ..Default::default()
        };

        assert_eq!(parsed_installation_role(&snapshot), None);
    }

    #[test]
    fn service_is_resolved_beside_the_running_target_specific_app() {
        let root = std::env::temp_dir().join(format!(
            "vapor-installer-service-path-{}",
            std::process::id()
        ));
        let target_directory = root.join("bin").join("test-target");
        std::fs::create_dir_all(&target_directory).expect("create target-specific app directory");
        let application =
            target_directory.join(format!("vapor-gui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&application, b"app").expect("write app placeholder");
        let service_name = if cfg!(windows) {
            "vapor-client-service.exe"
        } else {
            "vapor-client-service"
        };
        let service = target_directory.join(service_name);
        std::fs::write(&service, b"service").expect("write service placeholder");

        assert_eq!(resolve_service_path(&application).unwrap(), service);

        std::fs::remove_dir_all(root).expect("remove temporary app directory");
    }

    #[test]
    fn player_setup_receipt_is_checked_at_the_selected_installation() {
        let root = std::env::temp_dir().join(format!(
            "vapor-installer-player-receipt-{}",
            std::process::id()
        ));
        let receipt = root.join("data/state/installer/player.toml");
        std::fs::create_dir_all(receipt.parent().unwrap()).expect("create receipt directory");
        std::fs::write(&receipt, "schema = 1\nstate = \"ready\"\n").expect("write receipt");

        assert!(player_setup_ready_at(&root));

        std::fs::write(&receipt, "schema = 1\nstate = \"incomplete\"\n").expect("replace receipt");
        assert!(!player_setup_ready_at(&root));
        std::fs::remove_dir_all(root).expect("remove temporary app root");
    }

    #[test]
    fn missing_service_error_reports_the_expected_target_directory() {
        let application = std::env::temp_dir()
            .join("vapor-installer-missing-service")
            .join("bin")
            .join("test-target")
            .join(format!("vapor-gui{}", std::env::consts::EXE_SUFFIX));
        let error = resolve_service_path(&application).unwrap_err();
        assert!(error.contains("Vapor Client service is missing beside the running app"));
        assert!(error.contains("bin"));
        assert!(error.contains("test-target"));
    }
}
