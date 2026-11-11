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
    status: String,
    receiver: Option<mpsc::Receiver<Result<InstallerTaskResult, String>>>,
    progress_receiver: Option<mpsc::Receiver<String>>,
    confirm_uninstall: bool,
    confirm_full_erase: bool,
    full_erase_phrase: String,
    role: InstallationRoleSnapshot,
    selected_role: vapor_core::VaporRole,
    first_launch: bool,
}

struct InstallerTaskResult {
    message: String,
    role: Option<InstallationRoleSnapshot>,
    close_app: bool,
}

impl InstallerApp {
    pub fn new(app_root: Option<PathBuf>, first_launch: bool) -> Self {
        let (installation, status) = match app_root {
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
        let mut app = Self {
            status,
            installation,
            receiver: None,
            progress_receiver: None,
            confirm_uninstall: false,
            confirm_full_erase: false,
            full_erase_phrase: String::new(),
            role: InstallationRoleSnapshot::default(),
            selected_role: if first_launch {
                vapor_core::VaporRole::Player
            } else {
                vapor_core::VaporRole::Developer
            },
            first_launch,
        };
        app.refresh_role();
        app
    }

    fn run_installer(&mut self, operation: InstallerOperation) {
        let Some(installation) = self.installation.clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = format!("Running installer {}…", operation.label());
        thread::spawn(move || {
            let result = match operation {
                InstallerOperation::Status => vapor_core::diagnose_managed_state_for(&installation)
                    .map(|status| {
                        format!(
                            "Installation: {}\nHealthy: {}\nToolchain: {} ({})\nProjects: {}",
                            status.installation_root.display(),
                            status.is_healthy(),
                            status.toolchain_version,
                            if status.toolchain_installed {
                                "installed"
                            } else {
                                "missing"
                            },
                            status.current_projects,
                        )
                    })
                    .map_err(|error| error.to_string()),
                InstallerOperation::Repair => vapor_core::repair_managed_state_for(&installation)
                    .map(|report| {
                        format!(
                            "Repair complete. Installation: {}\nHealthy: {}",
                            report.status.installation_root.display(),
                            report.status.is_healthy(),
                        )
                    })
                    .map_err(|error| error.to_string()),
                InstallerOperation::Uninstall => vapor_core::uninstall_installation_for(
                    &installation,
                    vapor_core::UninstallOptions::default(),
                )
                .map(|report| {
                    format!(
                        "Vapor was removed from {}. User data and authored source were preserved.",
                        report.installation_root.display(),
                    )
                })
                .map_err(|error| error.to_string()),
            }
            .map(|message| InstallerTaskResult {
                message,
                role: None,
                close_app: false,
            });
            let _ = sender.send(result);
        });
    }

    fn run_full_erase(&mut self) {
        let Some(installation) = self.installation.clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = "Scheduling complete Vapor data erasure…".to_owned();
        thread::spawn(move || {
            let result = vapor_installer::full_erase::schedule(&installation.root).map(|log| {
                InstallerTaskResult {
                    message: format!("Vapor erasure is queued. Result log: {}", log.display()),
                    role: None,
                    close_app: true,
                }
            });
            let _ = sender.send(result);
        });
    }

    fn run_role_transition(&mut self, target: vapor_core::VaporRole, promote: bool) {
        let Some(installation) = self.installation.clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let (progress_sender, progress_receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.progress_receiver = Some(progress_receiver);
        self.status = format!(
            "{} installation role…",
            if promote { "Preparing" } else { "Changing" }
        );
        thread::spawn(move || {
            let result = (|| {
                if promote {
                    vapor_core::install_installation_for(&installation)
                        .map_err(|error| error.to_string())?;
                    if target == vapor_core::VaporRole::Developer {
                        vapor_installer::setup_development_for_app_root_with_progress(
                            &installation.root,
                            |stage| {
                                let _ = progress_sender.send(stage.to_owned());
                            },
                        )?;
                    } else if !player_setup_ready_at(&installation.root) {
                        vapor_installer::setup_player_for_app_root(&installation.root)?;
                    }
                } else if target == vapor_core::VaporRole::Player {
                    vapor_installer::teardown_development_for_app_root(&installation.root)?;
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
                let mut result = run_service_action(action, true, Some(&progress_sender))?;
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
        let current = self
            .role
            .role
            .parse::<vapor_core::VaporRole>()
            .unwrap_or(vapor_core::VaporRole::Player);
        let (sender, receiver) = mpsc::channel();
        let (progress_sender, progress_receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.progress_receiver = Some(progress_receiver);
        self.status = format!("Setting up this installation for {target}…");
        thread::spawn(move || {
            let result = vapor_core::install_installation_for(&installation)
                .map_err(|error| error.to_string())
                .and_then(|_| {
                    if target == vapor_core::VaporRole::Developer && target > current {
                        vapor_installer::setup_development_for_app_root_with_progress(
                            &installation.root,
                            |stage| {
                                let _ = progress_sender.send(stage.to_owned());
                            },
                        )
                    } else {
                        vapor_installer::setup_player_for_app_root(&installation.root)
                    }
                })
                .and_then(|()| {
                    if target > current {
                        run_service_action(
                            DeveloperAction::PromoteInstallationRole {
                                target: target.as_str().to_owned(),
                            },
                            true,
                            Some(&progress_sender),
                        )
                    } else {
                        run_service_action(
                            DeveloperAction::RefreshInstallationRole,
                            false,
                            None,
                        )
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

    fn continue_to_game(&mut self, context: &egui::Context) {
        let Some(installation) = &self.installation else {
            self.status = "Vapor installation is unavailable.".to_owned();
            return;
        };
        let app_root = installation.root.clone();
        let cli = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(PathBuf::from))
            .map(|bin| bin.join(format!("vapor{}", std::env::consts::EXE_SUFFIX)));
        let Some(cli) = cli.filter(|path| path.is_file()) else {
            self.status = "Vapor Shell is missing; repair Vapor before playing.".to_owned();
            return;
        };
        let mut command = Command::new(cli);
        command
            .args(["launch", "default"])
            .current_dir(&app_root)
            .env("VAPOR_HOME", &app_root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        match command.spawn() {
            Ok(_) => context.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(error) => self.status = format!("Could not launch Loo-Cast: {error}"),
        }
    }

    fn draw_first_launch(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.heading("Welcome to Vapor");
        ui.label("This copy was just installed. Required player setup has not run yet, so the game is locked until you finish this step.");
        ui.add_space(10.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("Choose how you’ll use Vapor");
            ui.label(
                "Developer installs the complete Vapor toolset. Player installs the runtime only.",
            );
            ui.add_enabled_ui(self.receiver.is_none(), |ui| {
                egui::ComboBox::from_id_salt("first-launch-role")
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
            ui.add_space(8.0);
            if self.player_setup_ready() {
                let installed = self
                    .role
                    .role
                    .parse::<vapor_core::VaporRole>()
                    .unwrap_or(vapor_core::VaporRole::Player);
                let role_ready = self.role.error.is_none() && installed >= self.selected_role;
                if ui
                    .add_enabled(
                        self.receiver.is_none() && role_ready,
                        egui::Button::new("Continue to Loo-Cast"),
                    )
                    .clicked()
                {
                    self.continue_to_game(ui.ctx());
                }
                if !role_ready && self.receiver.is_none() {
                    ui.label("Finish setup for the selected role before continuing.");
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
            ui.colored_label(egui::Color32::YELLOW, "Before uninstalling");
            ui.label("Steam does not reliably remove Vapor’s app-managed files and data on every platform. Before removing this Steam app, open Vapor Installer and choose Installation → Uninstall Vapor. You are responsible for running that cleanup operation.");
        });
    }

    fn refresh_role(&mut self) {
        if self.installation.is_none() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = "Checking installation capabilities…".to_owned();
        thread::spawn(move || {
            let result = run_service_action(DeveloperAction::RefreshInstallationRole, false, None);
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
            egui::CollapsingHeader::new("More")
                .default_open(false)
                .show(ui, |ui| {
                    if let Some(installation) = &self.installation {
                        ui.label("App folder");
                        ui.monospace(installation.root.display().to_string());
                    }
                    if ui
                        .add_enabled(
                            self.receiver.is_none() && self.installation.is_some(),
                            egui::Button::new("Remove command registration; keep Vapor data"),
                        )
                        .clicked()
                    {
                        self.confirm_uninstall = true;
                    }
                });
        });
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.colored_label(egui::Color32::LIGHT_RED, "Erase all Vapor data");
            ui.label("Permanently remove Vapor Home, legacy Vapor settings, managed Rust tools, and the external Vapor compiler tools. Steam game files and the USB recovery drive are excluded.");
            if ui
                .add_enabled(self.receiver.is_none() && self.installation.is_some(), egui::Button::new("Review full erasure…"))
                .clicked()
            {
                self.full_erase_phrase.clear();
                self.confirm_full_erase = true;
            }
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

            if current == vapor_core::VaporRole::Developer {
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
                        "Tracy: {}",
                        readiness(self.role.tracy_profiler_ready && self.role.tracy_analysis_ready)
                    ));
                });
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

fn parsed_installation_role(snapshot: &InstallationRoleSnapshot) -> Option<vapor_core::VaporRole> {
    snapshot.role.parse().ok()
}

fn role_description(role: vapor_core::VaporRole) -> &'static str {
    match role {
        vapor_core::VaporRole::Player => {
            "Play compositions and install content. Developer tools are not installed."
        }
        vapor_core::VaporRole::Developer => {
            "Build and manage Vapor content and ecosystem projects. The complete managed development toolset is included."
        }
    }
}

fn run_service_action(
    action: DeveloperAction,
    operation: bool,
    progress: Option<&mpsc::Sender<String>>,
) -> Result<InstallerTaskResult, String> {
    // Installer is launched from the target-specific app directory
    // (`bin/<target>`). Resolve the service beside the executable that is
    // actually running instead of assuming a flat `<root>/bin` layout.
    let gui_executable = std::env::current_exe()
        .map_err(|error| format!("could not locate the running Vapor Installer: {error}"))?;
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
                    if let Some(progress) = progress {
                        let message = event.message.clone().unwrap_or_else(|| {
                            event
                                .progress
                                .map(|value| format!("{:?} · {:.0}%", event.phase, value * 100.0))
                                .unwrap_or_else(|| format!("{:?}", event.phase))
                        });
                        let _ = progress.send(message);
                    }
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
        close_app: false,
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
    Repair,
    Uninstall,
}

impl InstallerOperation {
    fn label(self) -> &'static str {
        match self {
            Self::Status => "status check",
            Self::Repair => "repair",
            Self::Uninstall => "uninstall",
        }
    }
}

impl eframe::App for InstallerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(result) = self
            .receiver
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok())
        {
            self.receiver = None;
            self.progress_receiver = None;
            match result {
                Ok(result) => {
                    if let Some(role) = result.role {
                        sync_role_selection(&mut self.selected_role, &role);
                        self.role = role;
                    }
                    self.status = result.message;
                    if result.close_app {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                Err(error) => self.status = error,
            }
        }
        if let Some(message) = self
            .progress_receiver
            .as_ref()
            .and_then(|receiver| receiver.try_iter().last())
        {
            self.status = message;
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
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(10.0);
                    ui.heading("Vapor on this device");
                    ui.label("Install, repair, or completely erase this local Vapor setup.");
                    ui.add_space(12.0);
                    self.draw_role_panel(ui);
                    ui.add_space(10.0);
                    self.draw_installation_panel(ui);
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
            egui::Window::new("Remove command registration")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label("This removes Vapor from your user PATH and clears its user environment entry. Vapor files, tools, projects, and settings remain on this device.");
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.confirm_uninstall = false;
                        }
                        if ui.button("Remove registration").clicked() {
                            self.confirm_uninstall = false;
                            self.run_installer(InstallerOperation::Uninstall);
                        }
                    });
                });
        }
        if self.confirm_full_erase {
            egui::Window::new("Erase all Vapor data")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        "This permanently deletes Vapor and its local data.",
                    );
                    ui.label("Steam game files and your USB recovery drive are not included.");
                    match self
                        .installation
                        .as_ref()
                        .map(|installation| {
                            vapor_installer::full_erase::inventory(&installation.root)
                        })
                    {
                        Some(Ok(targets)) => {
                            egui::ScrollArea::vertical()
                                .max_height(190.0)
                                .show(ui, |ui| {
                                    for target in targets {
                                        ui.strong(target.label);
                                        ui.monospace(target.path.display().to_string());
                                    }
                                });
                        }
                        Some(Err(error)) => {
                            ui.colored_label(egui::Color32::LIGHT_RED, error);
                        }
                        None => {
                            ui.label("No active Vapor Home was found.");
                        }
                    }
                    if let Some(installation) = &self.installation {
                        let preserved =
                            vapor_installer::full_erase::preserved_user_rust_paths(
                                &installation.root,
                            );
                        if !preserved.is_empty() {
                            ui.separator();
                            ui.strong("Kept outside this erase");
                            ui.label(
                                "Your user-level Rust and Cargo installations are not part of Vapor's managed toolchain and will be preserved.",
                            );
                            for path in preserved {
                                ui.monospace(path.display().to_string());
                            }
                        }
                    }
                    ui.label("Type ERASE VAPOR to continue.");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.full_erase_phrase)
                            .desired_width(280.0),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.confirm_full_erase = false;
                            self.full_erase_phrase.clear();
                        }
                        let can_erase = self.receiver.is_none()
                            && self.full_erase_phrase == "ERASE VAPOR"
                            && self.installation.as_ref().is_some_and(|installation| {
                                vapor_installer::full_erase::inventory(&installation.root).is_ok()
                            });
                        if ui
                            .add_enabled(
                                can_erase,
                                egui::Button::new("Erase all listed Vapor data"),
                            )
                            .clicked()
                        {
                            self.confirm_full_erase = false;
                            self.full_erase_phrase.clear();
                            self.run_full_erase();
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
