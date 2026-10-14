use eframe::egui;
use std::{path::PathBuf, sync::mpsc, thread};

pub struct InstallerApp {
    app_root: PathBuf,
    status: String,
    receiver: Option<mpsc::Receiver<Result<String, String>>>,
    confirm_uninstall: bool,
}

impl InstallerApp {
    pub fn new(app_root: Option<PathBuf>) -> Self {
        Self {
            app_root: app_root
                .or_else(|| std::env::var_os("VAPOR_HOME").map(PathBuf::from))
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_default(),
            status: "Ready to inspect or maintain this Vapor installation.".to_owned(),
            receiver: None,
            confirm_uninstall: false,
        }
    }

    fn run_installer(&mut self, operation: InstallerOperation) {
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.status = format!("Running installer {}…", operation.label());
        thread::spawn(move || {
            let result = match operation {
                InstallerOperation::Status => vapor_core::diagnose_managed_state()
                    .map(|status| {
                        format!(
                            "Installation: {}\nHealthy: {}\nToolchain: {} ({})\nProjects: {}",
                            status.installation_root.display(),
                            status.is_healthy(),
                            status.toolchain_version,
                            if status.toolchain_installed { "installed" } else { "missing" },
                            status.current_projects,
                        )
                    })
                    .map_err(|error| error.to_string()),
                InstallerOperation::Install => vapor_core::install_installation()
                    .map(|report| {
                        format!(
                            "Installed Vapor at {}\nSuperworkspace: {}",
                            report.installation_root.display(),
                            report.superworkspace_root.display(),
                        )
                    })
                    .map_err(|error| error.to_string()),
                InstallerOperation::Repair => vapor_core::repair_managed_state()
                    .map(|report| {
                        format!(
                            "Repair complete. Installation: {}\nHealthy: {}",
                            report.status.installation_root.display(),
                            report.status.is_healthy(),
                        )
                    })
                    .map_err(|error| error.to_string()),
                InstallerOperation::Uninstall => vapor_core::uninstall_installation(
                    vapor_core::UninstallOptions::default(),
                )
                .map(|report| {
                    format!(
                        "Vapor was removed from {}. User data and authored source were preserved.",
                        report.installation_root.display(),
                    )
                })
                .map_err(|error| error.to_string()),
            };
            let _ = sender.send(result);
        });
    }
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
        if let Some(result) = self.receiver.as_ref().and_then(|receiver| receiver.try_recv().ok()) {
            self.receiver = None;
            self.status = result.unwrap_or_else(|error| error);
        }
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.heading("Vapor Installer");
            ui.label("Manage the installed Vapor app root.");
            ui.horizontal(|ui| {
                if ui.add_enabled(self.receiver.is_none(), egui::Button::new("Check status")).clicked() {
                    self.run_installer(InstallerOperation::Status);
                }
                if ui.add_enabled(self.receiver.is_none(), egui::Button::new("Install")).clicked() {
                    self.run_installer(InstallerOperation::Install);
                }
                if ui.add_enabled(self.receiver.is_none(), egui::Button::new("Uninstall")).clicked() {
                    self.confirm_uninstall = true;
                }
                if ui.add_enabled(self.receiver.is_none(), egui::Button::new("Repair")).clicked() {
                    self.run_installer(InstallerOperation::Repair);
                }
            });
            ui.separator();
            ui.label(format!("App root: {}", self.app_root.display()));
            ui.label(&self.status);
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
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}
