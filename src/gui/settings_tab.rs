//! Settings tab: default affinity, monitor intervals, appearance, autostart.

use egui::Ui;
use crate::config::Config;
use crate::cpu_park::{detect_topology, CpuTopology};
use crate::gui::dialogs::AffinityDialog;
use crate::gui::theme::AppTheme;
use crate::utils::cpuset_to_cpulist;

pub struct SettingsTab {
    pub config: Config,
    pub default_affinity_enabled: bool,
    pub default_affinity_text: String,
    pub cpu_dialog: Option<AffinityDialog>,
    pub opacity: f32,
    pub native_ppp: f32,
    pub autostart_enabled: bool,
    pub status: String,
    /// Active theme — changes are applied immediately in show().
    pub theme: AppTheme,
    // CPU Power state
    pub cpu_governor: String,
    pub available_governors: Vec<String>,
    pub cpu_epp: String,
    pub available_epps: Vec<String>,
    pub power_status: String,
    /// Detected CPU topology — drives dynamic quick-buttons
    pub topo: CpuTopology,
}

impl SettingsTab {
    pub fn new(config: Config) -> Self {
        let current_affinity = config.cpu.default_affinity.clone().unwrap_or_default();
        let default_affinity_enabled = !current_affinity.is_empty();
        let autostart_enabled = check_autostart_enabled();
        // Restore opacity and theme from persisted config.
        let opacity = config.ui.opacity.clamp(0.1, 1.0);
        let theme = AppTheme::from_str(&config.ui.theme);
        Self {
            default_affinity_text: current_affinity,
            default_affinity_enabled,
            cpu_dialog: None,
            config,
            opacity,
            native_ppp: 1.0,
            autostart_enabled,
            status: String::new(),
            theme,
            cpu_governor: read_governor(),
            available_governors: read_available_governors(),
            cpu_epp: read_epp(),
            available_epps: read_available_epps(),
            power_status: String::new(),
            topo: detect_topology(),
        }
    }

    /// Returns Some(updated_config) when any Apply button is clicked.
    pub fn show(&mut self, ui: &mut Ui, ctx: &egui::Context, opacity: f32) -> Option<Config> {
        let mut changed = false;

        // ── Default CPU Affinity ──────────────────────────────────────────
        group_box(ui, "Default CPU Affinity", |ui| {
            if self.topo.has_asymmetry() {
                ui.label(format!(
                    "Applied to every process that doesn't match a specific rule.\n\
                     Detected: {}. Typical: Default → {}, Game rule → {}",
                    self.topo.kind_label(),
                    self.topo.non_preferred_label,
                    self.topo.preferred_label,
                ));
            } else {
                ui.label("Applied to every process that doesn't match a specific rule.");
            }
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.checkbox(&mut self.default_affinity_enabled, "Enable default affinity:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.default_affinity_text)
                        .hint_text("e.g. 8-15,24-31")
                        .desired_width(130.0)
                        .interactive(self.default_affinity_enabled),
                );
                if ui.add_enabled(self.default_affinity_enabled, egui::Button::new("Pick CPUs…")).clicked() {
                    self.cpu_dialog = Some(AffinityDialog::new(&self.default_affinity_text, "Default"));
                }
            });

            ui.horizontal(|ui| {
                ui.label("Quick:");
                if self.topo.has_asymmetry() {
                    let pref_list = cpuset_to_cpulist(&self.topo.preferred);
                    let npref_list = cpuset_to_cpulist(&self.topo.non_preferred);
                    if ui.add_enabled(self.default_affinity_enabled,
                        egui::Button::new(self.topo.preferred_button_label())).clicked() {
                        self.default_affinity_text = pref_list;
                        self.default_affinity_enabled = true;
                    }
                    if ui.add_enabled(self.default_affinity_enabled,
                        egui::Button::new(self.topo.non_preferred_button_label())).clicked() {
                        self.default_affinity_text = npref_list;
                        self.default_affinity_enabled = true;
                    }
                }
                if ui.add_enabled(self.default_affinity_enabled, egui::Button::new("All")).clicked() {
                    self.default_affinity_text = String::new();
                    self.default_affinity_enabled = true;
                }
            });
        });

        // Handle Pick CPUs dialog
        if let Some(ref mut dlg) = self.cpu_dialog {
            if let Some(result) = dlg.show(ctx, opacity) {
                if !result.is_empty() {
                    self.default_affinity_text = result;
                }
                self.cpu_dialog = None;
            }
        }

        ui.add_space(4.0);
        if ui.button("Apply — enforce on all running processes now").clicked() {
            if self.default_affinity_enabled {
                let cpulist = self.default_affinity_text.trim().to_string();
                if cpulist.is_empty() {
                    self.status = "Select at least one CPU.".into();
                } else {
                    self.config.cpu.default_affinity = Some(cpulist.clone());
                    self.status = format!("Default affinity → {}. Enforcing now…", cpulist);
                    changed = true;
                }
            } else {
                self.config.cpu.default_affinity = None;
                self.status = "Default affinity disabled.".into();
                changed = true;
            }
        }

        ui.add_space(12.0);

        // ── Monitor Intervals ─────────────────────────────────────────────
        group_box(ui, "Monitor Intervals", |ui| {
            egui::Grid::new("mon_grid")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Rule enforce interval:");
                    ui.add(egui::DragValue::new(&mut self.config.monitor.rule_enforce_interval_ms)
                        .range(100..=10000).suffix("ms"));
                    ui.end_row();

                    ui.label("Display refresh interval:");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.config.monitor.display_refresh_interval_ms)
                            .range(500..=10000).suffix("ms"));
                        for (label, ms) in [("0.5s", 500u64), ("1s", 1000), ("2s", 2000), ("5s", 5000)] {
                            if ui.small_button(label).clicked() {
                                self.config.monitor.display_refresh_interval_ms = ms;
                            }
                        }
                    });
                    ui.end_row();
                });
        });

        ui.add_space(4.0);
        if ui.button("Apply Monitor Settings").clicked() {
            // Persist opacity and theme into the config that will be saved to disk.
            self.config.ui.opacity = self.opacity;
            self.config.ui.theme   = self.theme.to_str().into();
            crate::gui::theme::apply_theme(ctx, self.native_ppp, &self.theme);
            self.status = "Settings applied.".into();
            changed = true;
        }

        ui.add_space(12.0);

        // ── Appearance ────────────────────────────────────────────────────
        group_box(ui, "Appearance", |ui| {
            // Theme selector — applied immediately on change.
            ui.horizontal(|ui| {
                ui.label("Theme:");
                let prev_theme = self.theme.clone();
                egui::ComboBox::from_id_salt("theme_picker")
                    .selected_text(self.theme.label())
                    .show_ui(ui, |ui| {
                        for t in [AppTheme::BreezeDark, AppTheme::BreezeLight] {
                            ui.selectable_value(&mut self.theme, t.clone(), t.label());
                        }
                    });
                if self.theme != prev_theme {
                    crate::gui::theme::apply_theme(ctx, self.native_ppp, &self.theme);
                }
            });

            ui.add_space(4.0);

            // Opacity slider — live preview handled in app.rs.
            let border_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
            egui::Frame::new()
                .stroke(egui::Stroke::new(1.0, border_color))
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Window opacity:");
                        ui.add(egui::Slider::new(&mut self.opacity, 0.1f32..=1.0).show_value(true));
                    });
                });
        });

        ui.add_space(12.0);

        // ── CPU Power ─────────────────────────────────────────────────────
        group_box(ui, "CPU Power", |ui| {
            egui::Grid::new("cpu_power_grid")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Scaling governor:");
                    if self.available_governors.is_empty() {
                        ui.label(egui::RichText::new("(not available)").italics());
                    } else {
                        egui::ComboBox::from_id_salt("gov_picker")
                            .selected_text(&self.cpu_governor)
                            .show_ui(ui, |ui| {
                                for g in &self.available_governors.clone() {
                                    ui.selectable_value(&mut self.cpu_governor, g.clone(), g.as_str());
                                }
                            });
                    }
                    ui.end_row();

                    ui.label("Energy perf. preference:");
                    if self.available_epps.is_empty() {
                        ui.label(egui::RichText::new("(not available)").italics());
                    } else {
                        egui::ComboBox::from_id_salt("epp_picker")
                            .selected_text(&self.cpu_epp)
                            .show_ui(ui, |ui| {
                                for e in &self.available_epps.clone() {
                                    ui.selectable_value(&mut self.cpu_epp, e.clone(), e.as_str());
                                }
                            });
                    }
                    ui.end_row();
                });
        });

        ui.add_space(4.0);
        if ui.button("Apply CPU Power Settings").clicked() {
            let mut msgs: Vec<String> = Vec::new();
            if !self.available_governors.is_empty() {
                match set_governor(&self.cpu_governor) {
                    Ok(_) => msgs.push(format!("Governor → {}", self.cpu_governor)),
                    Err(e) => msgs.push(format!("Governor failed: {e}")),
                }
            }
            if !self.available_epps.is_empty() {
                match set_epp(&self.cpu_epp) {
                    Ok(_) => msgs.push(format!("EPP → {}", self.cpu_epp)),
                    Err(e) => msgs.push(format!("EPP failed: {e}")),
                }
            }
            self.power_status = msgs.join("  |  ");
        }

        if !self.power_status.is_empty() {
            ui.add_space(4.0);
            ui.colored_label(ui.visuals().weak_text_color(), &self.power_status);
        }

        ui.add_space(12.0);

        // ── Notifications ─────────────────────────────────────────────────
        group_box(ui, "Notifications", |ui| {
            ui.checkbox(&mut self.config.ui.notifications_enabled,
                "Enable desktop notifications (ProBalance throttle, HW alerts, kill events)");
        });

        ui.add_space(12.0);

        // ── Temperature Alerts ────────────────────────────────────────────
        group_box(ui, "Temperature Alerts", |ui| {
            ui.checkbox(&mut self.config.hw_alerts.enabled, "Enable temperature alerts (desktop notifications)");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Alert threshold:");
                ui.add(egui::Slider::new(&mut self.config.hw_alerts.temp_threshold_celsius, 50.0..=110.0)
                    .suffix(" °C")
                    .step_by(1.0));
            });
            ui.horizontal(|ui| {
                ui.label("Cooldown between alerts:");
                let mut cooldown = self.config.hw_alerts.cooldown_secs as f64;
                if ui.add(egui::Slider::new(&mut cooldown, 10.0..=300.0).suffix(" s").step_by(5.0)).changed() {
                    self.config.hw_alerts.cooldown_secs = cooldown as u64;
                }
            });
            if ui.button("Apply Alert Settings").clicked() {
                changed = true;
            }
        });
        ui.add_space(8.0);

        // ── Autostart ─────────────────────────────────────────────────────
        group_box(ui, "Autostart", |ui| {
            ui.checkbox(&mut self.autostart_enabled,
                "Start Argus-Lasso automatically with your desktop session");
        });

        ui.add_space(4.0);
        if ui.add_sized([ui.available_width(), 28.0], egui::Button::new("Apply Autostart Setting")).clicked() {
            if self.autostart_enabled {
                match write_autostart() {
                    Ok(_) => self.status = "Autostart enabled (XDG + systemd).".into(),
                    Err(e) => self.status = format!("Autostart failed: {e}"),
                }
            } else {
                match disable_autostart() {
                    Ok(_) => self.status = "Autostart disabled.".into(),
                    Err(e) => self.status = format!("Disable failed: {e}"),
                }
            }
        }

        if !self.status.is_empty() {
            ui.add_space(4.0);
            ui.colored_label(ui.visuals().weak_text_color(), &self.status);
        }

        if changed { Some(self.config.clone()) } else { None }
    }
}

/// Render a QGroupBox-style bordered section with a top-left title (matching Qt QGroupBox).
fn group_box(ui: &mut Ui, title: &str, add_contents: impl FnOnce(&mut Ui)) {
    let border_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let frame = egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, border_color))
        .inner_margin(egui::Margin::same(8))
        .corner_radius(egui::CornerRadius::same(4));

    frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.label(egui::RichText::new(title).strong().color(ui.visuals().strong_text_color()));
        ui.add_space(4.0);
        add_contents(ui);
    });
}

// ── CPU governor / EPP sysfs helpers ─────────────────────────────────────────

fn read_governor() -> String {
    std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn read_available_governors() -> Vec<String> {
    std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_available_governors")
        .map(|s| s.split_whitespace().map(|t| t.to_string()).collect())
        .unwrap_or_default()
}

fn set_governor(governor: &str) -> Result<(), String> {
    let cpu_count = crate::utils::get_cpu_count();
    let mut errors = 0usize;
    for i in 0..cpu_count {
        let path = format!("/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_governor");
        if std::fs::write(&path, governor).is_err() {
            errors += 1;
        }
    }
    if errors == cpu_count as usize {
        Err("Permission denied — try running as root or add a udev rule".into())
    } else {
        Ok(())
    }
}

fn read_epp() -> String {
    std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn read_available_epps() -> Vec<String> {
    std::fs::read_to_string(
        "/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_available_preferences",
    )
    .map(|s| s.split_whitespace().map(|t| t.to_string()).collect())
    .unwrap_or_default()
}

fn set_epp(epp: &str) -> Result<(), String> {
    let cpu_count = crate::utils::get_cpu_count();
    let mut errors = 0usize;
    for i in 0..cpu_count {
        let path = format!(
            "/sys/devices/system/cpu/cpu{i}/cpufreq/energy_performance_preference"
        );
        if std::fs::write(&path, epp).is_err() {
            errors += 1;
        }
    }
    if errors == cpu_count as usize {
        Err("Permission denied".into())
    } else {
        Ok(())
    }
}

fn check_autostart_enabled() -> bool {
    // Check XDG autostart first (works on GNOME and KDE).
    let home = std::env::var("HOME").unwrap_or_default();
    let xdg = format!("{home}/.config/autostart/argus-lasso.desktop");
    if std::path::Path::new(&xdg).exists() {
        return true;
    }
    // Fall back to systemd user service check.
    std::process::Command::new("systemctl")
        .args(["--user", "is-enabled", "argus-lasso.service"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "enabled")
        .unwrap_or(false)
}

fn write_autostart() -> std::io::Result<()> {
    let home = std::env::var("HOME").unwrap_or_default();
    let exe = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("argus-lasso"));

    // ── XDG autostart (works on GNOME, KDE, XFCE, and most other DEs) ────────
    let xdg_dir = format!("{home}/.config/autostart");
    std::fs::create_dir_all(&xdg_dir)?;
    let xdg_entry = format!(
        "[Desktop Entry]\nType=Application\nName=Argus-Lasso\n\
         Exec={} --minimized\nIcon=argus-lasso\nHidden=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exe.display()
    );
    std::fs::write(format!("{xdg_dir}/argus-lasso.desktop"), xdg_entry)?;

    // ── systemd user service (KDE / systemd-based desktops) ──────────────────
    let systemd_dir = format!("{home}/.config/systemd/user");
    if std::fs::create_dir_all(&systemd_dir).is_ok() {
        let unit = format!(
            "[Unit]\nDescription=Argus-Lasso Linux\nAfter=graphical-session.target\n\n\
             [Service]\nExecStart={} --minimized\nRestart=on-failure\n\n\
             [Install]\nWantedBy=graphical-session.target\n",
            exe.display()
        );
        let _ = std::fs::write(format!("{systemd_dir}/argus-lasso.service"), unit);
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "enable", "argus-lasso.service"])
            .output();
    }

    Ok(())
}

fn disable_autostart() -> std::io::Result<()> {
    let home = std::env::var("HOME").unwrap_or_default();

    // Remove XDG autostart entry.
    let xdg = format!("{home}/.config/autostart/argus-lasso.desktop");
    let _ = std::fs::remove_file(&xdg);

    // Disable systemd unit if present.
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", "argus-lasso.service"])
        .output();

    Ok(())
}
