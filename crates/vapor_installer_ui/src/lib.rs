use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use vapor_client::{Client, ClientHostConfig, service_sibling_path};
use vapor_client_protocol::{
    ClientAction, ClientEvent, ClientOrigin, ClientResponse, ClientResponseKind, DeveloperAction,
    InstallationRoleSnapshot, OperationStatus, ProtocolFrame,
};

pub struct InstallerApp {
    installation: Option<vapor_core::VaporInstallation>,
    page: InstallerPage,
    status: String,
    receiver: Option<mpsc::Receiver<Result<InstallerTaskResult, String>>>,
    progress_receiver: Option<mpsc::Receiver<String>>,
    confirm_uninstall: bool,
    confirm_standalone_uninstall: bool,
    confirm_vapor_data_erasure: bool,
    vapor_data_erasure_plan: Option<vapor_core::VaporDataErasurePlan>,
    role: InstallationRoleSnapshot,
    selected_role: vapor_core::VaporRole,
    selected_release: String,
    standalone_bootstrap: bool,
}

struct InstallerTaskResult {
    message: String,
    role: Option<InstallationRoleSnapshot>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InstallerPage {
    Overview,
    Role,
    Installation,
}

impl InstallerApp {
    pub fn new(app_root: Option<PathBuf>) -> Self {
        let (installation, status) = match app_root {
            Some(root) => match vapor_core::VaporInstallation::from_explicit_root(root) {
                Ok(installation) => (
                    Some(installation),
                    "Ready to inspect or maintain this Vapor installation.".to_owned(),
                ),
                Err(error) => (None, error.to_string()),
            },
            None => match vapor_core::VaporInstallation::discover() {
                Ok(installation) => (
                    Some(installation),
                    "Ready to inspect or maintain this Vapor installation.".to_owned(),
                ),
                Err(error) => (None, error.to_string()),
            },
        };
        let mut app = Self {
            status,
            installation,
            page: InstallerPage::Overview,
            receiver: None,
            progress_receiver: None,
            confirm_uninstall: false,
            confirm_standalone_uninstall: false,
            confirm_vapor_data_erasure: false,
            vapor_data_erasure_plan: None,
            role: InstallationRoleSnapshot::default(),
            selected_role: vapor_core::VaporRole::Player,
            selected_release: String::new(),
            standalone_bootstrap: false,
        };
        app.refresh_role();
        app
    }

    /// Create the website-distributed setup surface without requiring a
    /// Steam app root, installed Vapor app, or running Client service.
    pub fn new_standalone_bootstrap() -> Self {
        Self {
            installation: None,
            page: InstallerPage::Overview,
            status: "Install Vapor independently of Steam, or open the existing installation.".to_owned(),
            receiver: None,
            progress_receiver: None,
            confirm_uninstall: false,
            confirm_standalone_uninstall: false,
            confirm_vapor_data_erasure: false,
            vapor_data_erasure_plan: None,
            role: InstallationRoleSnapshot::default(),
            selected_role: vapor_core::VaporRole::Player,
            selected_release: vapor_installer::standalone::active_version()
                .ok()
                .flatten()
                .unwrap_or_default(),
            standalone_bootstrap: true,
        }
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
                InstallerOperation::Install => {
                    vapor_installer::prepare_player_installation(&installation).map(|report| {
                        format!(
                            "Installed Vapor at {}\nSuperworkspace: {}",
                            report.installation_root.display(),
                            report.superworkspace_root.display(),
                        )
                    })
                }
                InstallerOperation::Repair => {
                    vapor_installer::prepare_player_installation(&installation)
                        .and_then(|_| {
                            vapor_core::repair_managed_state_for(&installation)
                                .map_err(|error| error.to_string())
                        })
                        .map(|report| {
                            format!(
                                "Repair complete. Installation: {}\nHealthy: {}",
                                report.status.installation_root.display(),
                                report.status.is_healthy(),
                            )
                        })
                }
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
            .map(|message| InstallerTaskResult { message, role: None });
            let _ = sender.send(result);
        });
    }

    fn install_standalone_release(&mut self) {
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = "Checking the signed stable Vapor release…".to_owned();
        thread::spawn(move || {
            let result = vapor_installer::standalone::install_latest("stable").map(|report| {
                InstallerTaskResult {
                    message: format!(
                        "Vapor {} is ready at {}. Steam Play remains available without Vapor.",
                        report.version,
                        report.release_root.display()
                    ),
                    role: None,
                }
            });
            let _ = sender.send(result);
        });
    }

    fn activate_standalone_release(&mut self, version: String) {
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = format!("Selecting Vapor {version} for the next launch…");
        thread::spawn(move || {
            let result = vapor_installer::standalone::activate_installed_version(&version)
                .map(|_| InstallerTaskResult {
                    message: format!(
                        "Vapor {version} is selected and will open on the next launch."
                    ),
                    role: None,
                });
            let _ = sender.send(result);
        });
    }

    fn repair_standalone_release(&mut self) {
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = "Downloading and verifying the signed stable release for repair…".to_owned();
        thread::spawn(move || {
            let result = vapor_installer::standalone::repair_latest().map(|report| InstallerTaskResult {
                message: format!("Vapor {} was repaired and activated.", report.version),
                role: None,
            });
            let _ = sender.send(result);
        });
    }

    fn uninstall_standalone_release(&mut self) {
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = "Removing installed Vapor program versions…".to_owned();
        thread::spawn(move || {
            let result = vapor_installer::standalone::uninstall_app().map(|preserved| {
                let mut message = "Vapor program versions and their shell integration were removed. User data and source were preserved.".to_owned();
                for path in preserved {
                    message.push_str(&format!("\nPreserved unrecognized entry: {}", path.display()));
                }
                InstallerTaskResult { message, role: None }
            });
            let _ = sender.send(result);
        });
    }

    fn preview_vapor_data_erasure(&mut self) {
        match vapor_core::plan_vapor_data_erasure() {
            Ok(plan) => {
                self.vapor_data_erasure_plan = Some(plan);
                self.confirm_vapor_data_erasure = true;
            }
            Err(error) => self.status = format!("Could not safely prepare the full wipe: {error}"),
        }
    }

    fn erase_all_vapor_data(&mut self, reviewed_plan: vapor_core::VaporDataErasurePlan) {
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = "Permanently erasing Vapor data and registered source checkouts…".to_owned();
        thread::spawn(move || {
            let result = vapor_core::erase_all_vapor_data_from_plan(&reviewed_plan).map(|report| {
                InstallerTaskResult {
                    message: format!(
                        "Vapor data was erased from {}. Steam-managed Loo-Cast was left installed.{}",
                        report.user_data_root.display(),
                        if report.preserved_superworkspace_entries.is_empty() {
                            String::new()
                        } else {
                            format!(
                                " Preserved {} unrelated Superworkspace entr{}.",
                                report.preserved_superworkspace_entries.len(),
                                if report.preserved_superworkspace_entries.len() == 1 { "y" } else { "ies" }
                            )
                        }
                    ),
                    role: None,
                }
            }).map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
    }

    fn launch_standalone_release(&mut self) {
        let result = (|| {
            let version = vapor_installer::standalone::active_version()?
                .ok_or_else(|| "Vapor is not installed yet.".to_owned())?;
            let executable = vapor_installer::standalone::active_entrypoint()?;
            vapor_core::managed_process::background_command(executable)
                .spawn()
                .map_err(|error| format!("Could not open Vapor {version}: {error}"))?;
            Ok(format!("Opened Vapor {version}."))
        })();
        match result {
            Ok(message) => self.status = message,
            Err(error) => self.status = error,
        }
    }

    fn draw_standalone_panel(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.heading(if self.standalone_bootstrap {
                "Install Vapor"
            } else {
                "Standalone Vapor app"
            });
            ui.label(
                "Install or update Vapor in its own user-data location. This does not touch the Steam game or your source checkouts.",
            );
            match vapor_installer::standalone::active_version() {
                Ok(Some(version)) => ui.label(format!("Active release: {version}")),
                Ok(None) => ui.label("Vapor is not installed on this device yet."),
                Err(error) => ui.colored_label(
                    egui::Color32::YELLOW,
                    format!("Could not read the active release: {error}"),
                ),
            };
            match vapor_installer::standalone::installed_versions() {
                Ok(versions) if !versions.is_empty() => {
                    let active = vapor_installer::standalone::active_version()
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    if !versions.iter().any(|version| version == &self.selected_release) {
                        self.selected_release = active.clone();
                    }
                    ui.horizontal(|ui| {
                        ui.label("Installed versions:");
                        egui::ComboBox::from_id_salt("standalone-release-version")
                            .selected_text(if self.selected_release.is_empty() {
                                "Choose a version"
                            } else {
                                &self.selected_release
                            })
                            .show_ui(ui, |ui| {
                                for version in &versions {
                                    ui.selectable_value(
                                        &mut self.selected_release,
                                        version.clone(),
                                        version.as_str(),
                                    );
                                }
                            });
                        if ui
                            .add_enabled(
                                self.receiver.is_none()
                                    && !self.selected_release.is_empty()
                                    && self.selected_release != active,
                                egui::Button::new("Use on next launch"),
                            )
                            .clicked()
                        {
                            self.activate_standalone_release(self.selected_release.clone());
                        }
                    });
                }
                Ok(_) => {}
                Err(error) => {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        format!("Could not list installed releases: {error}"),
                    );
                }
            }
            ui.add_space(6.0);
            if ui
                .add_enabled(
                    self.receiver.is_none(),
                    egui::Button::new(if vapor_installer::standalone::active_version()
                        .ok()
                        .flatten()
                        .is_some()
                    {
                        "Install updates"
                    } else {
                        "Install Vapor"
                    }),
                )
                .clicked()
            {
                self.install_standalone_release();
            }
            if ui
                .add_enabled(
                    self.receiver.is_none()
                        && vapor_installer::standalone::active_version()
                            .ok()
                            .flatten()
                            .is_some(),
                    egui::Button::new("Repair Vapor"),
                )
                .clicked()
            {
                self.repair_standalone_release();
            }
            if ui
                .add_enabled(
                    self.receiver.is_none()
                        && vapor_installer::standalone::active_version()
                            .ok()
                            .flatten()
                            .is_some(),
                    egui::Button::new("Open Vapor"),
                )
                .clicked()
            {
                self.launch_standalone_release();
            }
            if vapor_installer::standalone::active_version()
                .ok()
                .flatten()
                .is_some()
                && ui.button("Remove Vapor…").clicked()
            {
                self.confirm_standalone_uninstall = true;
            }
            if self.standalone_bootstrap || self.installation.is_none() {
                if ui
                    .add_enabled(
                        self.receiver.is_none(),
                        egui::Button::new("Erase all Vapor data…"),
                    )
                    .clicked()
                {
                    self.preview_vapor_data_erasure();
                }
            }
            if self.standalone_bootstrap {
                ui.small("Vapor installs under its own user-data location. This setup does not require Steam.");
            }
        });
    }

    fn run_role_transition(&mut self, target: vapor_core::VaporRole, promote: bool) {
        let Some(installation) = self.installation.clone() else {
            return;
        };
        let home = installation.root.clone();
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
                    vapor_installer::prepare_player_installation(&installation)?;
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
                let mut result = run_service_action(&home, action, true, Some(&progress_sender))?;
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

    fn refresh_role(&mut self) {
        let Some(home) = self
            .installation
            .as_ref()
            .map(|installation| installation.root.clone())
        else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = "Checking installation capabilities…".to_owned();
        thread::spawn(move || {
            let result =
                run_service_action(&home, DeveloperAction::RefreshInstallationRole, false, None);
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
                        egui::Button::new("Set up Vapor"),
                    )
                    .clicked()
                {
                    self.run_installer(InstallerOperation::Install);
                }
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
                        ui.label("Vapor home");
                        ui.monospace(installation.root.display().to_string());
                    }
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                self.receiver.is_none() && self.installation.is_some(),
                                egui::Button::new("Uninstall Vapor"),
                            )
                            .clicked()
                        {
                            self.confirm_uninstall = true;
                        }
                        if ui
                            .add_enabled(
                                self.receiver.is_none() && self.installation.is_some(),
                                egui::Button::new("Erase all Vapor data…"),
                            )
                            .clicked()
                        {
                            self.preview_vapor_data_erasure();
                        }
                    });
                });
        });
    }

    fn draw_role_panel(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.heading("Choose Player or Developer");
            ui.label("Developer setup installs the complete supported toolchain for creating content and working on Vapor.");
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
            let action = if self.selected_role == vapor_core::VaporRole::Developer {
                "Set up Developer tools"
            } else {
                "Switch to Player"
            };
            if ui
                .add_enabled(
                    self.receiver.is_none() && changing,
                    egui::Button::new(if changing {
                        action.to_owned()
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

fn parsed_installation_role(snapshot: &InstallationRoleSnapshot) -> Option<vapor_core::VaporRole> {
    snapshot.role.parse().ok()
}

fn role_description(role: vapor_core::VaporRole) -> &'static str {
    match role {
        vapor_core::VaporRole::Player => {
            "Play compositions and install content. Developer tools are not installed."
        }
        vapor_core::VaporRole::Developer => {
            "Create content or work on Vapor. Vapor prepares the complete supported development toolchain."
        }
    }
}

fn run_service_action(
    home: &Path,
    action: DeveloperAction,
    operation: bool,
    progress: Option<&mpsc::Sender<String>>,
) -> Result<InstallerTaskResult, String> {
    let target = vapor_core::HostTarget::current()
        .map_err(|error| error.to_string())?
        .triple();
    let gui_executable = home.join("bin").join(target).join(format!(
        "vapor-installer-gui{}",
        std::env::consts::EXE_SUFFIX
    ));
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
    client.set_origin(ClientOrigin::Installer);
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
    Install,
    Status,
    Repair,
    Uninstall,
}

impl InstallerOperation {
    fn label(self) -> &'static str {
        match self {
            Self::Install => "player setup",
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
        egui::Panel::top("installer-navigation").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(if self.standalone_bootstrap {
                    "Vapor Setup"
                } else {
                    "Vapor Installer"
                });
                if !self.standalone_bootstrap {
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
                }
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.page {
                InstallerPage::Overview => {
                    ui.add_space(8.0);
                    if self.standalone_bootstrap {
                        ui.heading("Vapor installs independently of Steam");
                        ui.label("Install the current signed release, update an existing copy, or open Vapor.");
                        ui.add_space(18.0);
                        self.draw_standalone_panel(ui);
                    } else {
                        ui.heading("Set up this device for Vapor");
                        ui.label("Choose how you use Vapor. You can change your role at any time.");
                        ui.add_space(12.0);
                        self.draw_standalone_panel(ui);
                        ui.add_space(12.0);
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
                }
                InstallerPage::Role => {
                    ui.add_space(8.0);
                    ui.heading("Choose Player or Developer");
                    ui.label("Player setup omits development tools. Developer setup installs the complete supported toolchain.");
                    ui.add_space(12.0);
                    self.draw_role_panel(ui);
                }
                InstallerPage::Installation => {
                    ui.add_space(8.0);
                    ui.heading("Vapor app and device setup");
                    ui.label("The standalone app, optional developer setup, and their data have separate lifecycle controls.");
                    ui.add_space(12.0);
                    self.draw_standalone_panel(ui);
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
        if self.confirm_uninstall {
            egui::Window::new("Remove command registration")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label("This removes the independent Vapor installation and its machine integration. Steam-owned Loo-Cast files, locally installed games, and your user data are not removed by this action.");
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
        if self.confirm_standalone_uninstall {
            egui::Window::new("Remove Vapor?")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label("This removes the installed Vapor program versions and their shell integration. Your Vapor settings, caches, logs, saves, and authored source checkouts remain.");
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.confirm_standalone_uninstall = false;
                        }
                        if ui.button("Remove Vapor").clicked() {
                            self.confirm_standalone_uninstall = false;
                            self.uninstall_standalone_release();
                        }
                    });
                });
        }
        if self.confirm_vapor_data_erasure {
            egui::Window::new("Erase all Vapor data?")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label("This permanently removes Vapor's installed releases, settings, caches, logs, game-specific saves, and registered source checkouts. Steam-managed Loo-Cast stays installed.");
                    if let Some(plan) = &self.vapor_data_erasure_plan {
                        ui.label("Vapor User Data to remove:");
                        ui.monospace(plan.user_data_root.display().to_string());
                        if let Some(root) = &plan.superworkspace_root {
                            ui.label("Registered source workspace:");
                            ui.monospace(root.display().to_string());
                        }
                        if !plan.source_repositories.is_empty() {
                            ui.label("Registered source checkouts to remove:");
                            for path in &plan.source_repositories {
                                ui.monospace(path.display().to_string());
                            }
                        }
                        if !plan.superworkspace_data.is_empty() {
                            ui.label("Vapor-managed workspace metadata and IDE state to remove:");
                            for path in &plan.superworkspace_data {
                                ui.monospace(path.display().to_string());
                            }
                        }
                        if !plan.preserved_superworkspace_entries.is_empty() {
                            ui.label("Unrecognized neighboring entries preserved:");
                            for path in &plan.preserved_superworkspace_entries {
                                ui.monospace(path.display().to_string());
                            }
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.confirm_vapor_data_erasure = false;
                            self.vapor_data_erasure_plan = None;
                        }
                        if ui.button("Permanently erase all Vapor data").clicked() {
                            let reviewed_plan = self.vapor_data_erasure_plan.clone();
                            self.confirm_vapor_data_erasure = false;
                            self.vapor_data_erasure_plan = None;
                            if let Some(reviewed_plan) = reviewed_plan {
                                self.erase_all_vapor_data(reviewed_plan);
                            }
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
        InstallerApp, parsed_installation_role, resolve_service_path, sync_role_selection,
    };
    use vapor_client_protocol::InstallationRoleSnapshot;

    #[test]
    fn installer_keeps_an_explicit_root_as_its_operation_context() {
        let selected =
            std::env::temp_dir().join(format!("vapor-installer-ui-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&selected);
        std::fs::create_dir_all(&selected).expect("create app root");
        std::fs::write(
            selected.join("Vapor-Home.toml"),
            "schema = 1\n\n[vapor]\nid = \"ghf-studios/vapor\"\n",
        )
        .expect("write Vapor home marker");

        let app = InstallerApp::new(Some(selected.clone()));

        assert_eq!(
            app.installation.expect("explicit installation").root,
            std::fs::canonicalize(&selected).expect("canonicalize explicit installation")
        );
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
