use eframe::egui;
use std::{
    path::PathBuf,
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
    confirm_uninstall: bool,
    role: InstallationRoleSnapshot,
    selected_role: vapor_core::VaporRole,
}

struct InstallerTaskResult {
    message: String,
    role: Option<InstallationRoleSnapshot>,
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
            receiver: None,
            confirm_uninstall: false,
            role: InstallationRoleSnapshot::default(),
            selected_role: vapor_core::VaporRole::Composer,
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
                InstallerOperation::Install => vapor_core::install_installation_for(&installation)
                    .map(|report| {
                        format!(
                            "Installed Vapor at {}\nSuperworkspace: {}",
                            report.installation_root.display(),
                            report.superworkspace_root.display(),
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
            });
            let _ = sender.send(result);
        });
    }

    fn run_role_transition(&mut self, target: vapor_core::VaporRole, promote: bool) {
        let Some(installation) = self.installation.clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = format!(
            "{} installation role…",
            if promote { "Preparing" } else { "Changing" }
        );
        thread::spawn(move || {
            let action = if promote {
                DeveloperAction::PromoteInstallationRole {
                    target: target.as_str().to_owned(),
                }
            } else {
                DeveloperAction::DemoteInstallationRole {
                    target: target.as_str().to_owned(),
                }
            };
            let result = run_service_action(&installation, action, true);
            let _ = sender.send(result);
        });
    }

    fn refresh_role(&mut self) {
        let Some(installation) = self.installation.clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = "Checking installation capabilities…".to_owned();
        thread::spawn(move || {
            let result = run_service_action(
                &installation,
                DeveloperAction::RefreshInstallationRole,
                false,
            );
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
                        ui.label("App folder");
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
            if let Some(error) = &self.role.error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
                if ui
                    .add_enabled(self.receiver.is_none(), egui::Button::new("Try again"))
                    .clicked()
                {
                    self.refresh_role();
                }
                return;
            }

            let current = self
                .role
                .role
                .parse::<vapor_core::VaporRole>()
                .unwrap_or(vapor_core::VaporRole::Player);
            ui.strong(format!("Current role: {current}"));
            ui.add_enabled_ui(self.receiver.is_none(), |ui| {
                egui::ComboBox::from_id_salt("installer-role-target")
                    .selected_text(self.selected_role.display_name())
                    .show_ui(ui, |ui| {
                        for role in [
                            vapor_core::VaporRole::Player,
                            vapor_core::VaporRole::Composer,
                            vapor_core::VaporRole::ContentDeveloper,
                            vapor_core::VaporRole::EcosystemDeveloper,
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

            if current >= vapor_core::VaporRole::Composer {
                ui.add_space(6.0);
                ui.strong("Developer tools");
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("Rust + Cargo: {}", readiness(self.role.rust_ready)));
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

fn role_description(role: vapor_core::VaporRole) -> &'static str {
    match role {
        vapor_core::VaporRole::Player => {
            "Play compositions and install content. Developer tools are not required."
        }
        vapor_core::VaporRole::Composer => {
            "Create and manage playable compositions. Vapor prepares Rust, Cargo, Windows build support, and Tracy."
        }
        vapor_core::VaporRole::ContentDeveloper => {
            "Create and publish content that can be added to compositions."
        }
        vapor_core::VaporRole::EcosystemDeveloper => {
            "Work on Vapor itself and its first-party ecosystem."
        }
    }
}

fn run_service_action(
    installation: &vapor_core::VaporInstallation,
    action: DeveloperAction,
    operation: bool,
) -> Result<InstallerTaskResult, String> {
    let gui_executable = installation
        .root
        .join("bin")
        .join(format!("vapor-gui{}", std::env::consts::EXE_SUFFIX));
    let service_path = service_sibling_path(&gui_executable);
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
        let operation_id = response
            .snapshot
            .active_operation
            .map(|active| active.id)
            .ok_or_else(|| {
                "The service accepted the role change without an operation.".to_owned()
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
                }
                Err(error) => self.status = error,
            }
        }
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Vapor Installer");
                ui.label("Set up this device for playing, composing, or developing with Vapor.");
                ui.add_space(12.0);

                if ui.available_width() >= 820.0 {
                    ui.columns(2, |columns| {
                        self.draw_installation_panel(&mut columns[0]);
                        self.draw_role_panel(&mut columns[1]);
                    });
                } else {
                    self.draw_role_panel(ui);
                    ui.add_space(10.0);
                    self.draw_installation_panel(ui);
                }

                ui.add_space(10.0);
                ui.label(&self.status);
            });
        });
        if self.confirm_uninstall {
            egui::Window::new("Confirm uninstall")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label("Remove Vapor's machine integration? User data and authored source will be preserved.");
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
    use super::{InstallerApp, sync_role_selection};
    use vapor_client_protocol::InstallationRoleSnapshot;

    #[test]
    fn installer_keeps_an_explicit_root_as_its_operation_context() {
        let selected =
            std::env::temp_dir().join(format!("vapor-installer-ui-root-{}", std::process::id()));
        std::fs::create_dir_all(&selected).expect("create app root");
        std::fs::write(selected.join("App.vapor.toml"), "[root]\n").expect("write app manifest");

        let app = InstallerApp::new(Some(selected.clone()));

        assert_eq!(
            app.installation.expect("explicit installation").root,
            selected
        );
        std::fs::remove_dir_all(selected).expect("remove test app root");
    }

    #[test]
    fn installed_role_becomes_the_selected_role_instead_of_suggesting_a_demotion() {
        let mut selected = vapor_core::VaporRole::Composer;
        let installed = InstallationRoleSnapshot {
            role: "ecosystem-developer".to_owned(),
            ..Default::default()
        };

        sync_role_selection(&mut selected, &installed);

        assert_eq!(selected, vapor_core::VaporRole::EcosystemDeveloper);
    }
}
