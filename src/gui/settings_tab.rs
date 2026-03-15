//! Settings tab: default affinity, monitor intervals, appearance, autostart.

use egui::Ui;
use crate::config::Config;
use crate::gui::dialogs::AffinityDialog;
use crate::gui::theme::AppTheme;

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
        }
    }

    /// Returns Some(updated_config) when any Apply button is clicked.
    pub fn show(&mut self, ui: &mut Ui, ctx: &egui::Context, opacity: f32) -> Option<Config> {
        let mut changed = false;

        // ── Default CPU Affinity ──────────────────────────────────────────
        group_box(ui, "Default CPU Affinity", |ui| {
            ui.label(
                "Applied to every process that doesn't match a specific rule.\n\
                 Typical 7950X3D: Default → CCD1 (background), Rule: steam → CCD0 (game)"
            );
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
                for (label, val) in [
                    ("CCD0 (0-7,16-23)", "0-7,16-23"),
                    ("CCD1 (8-15,24-31)", "8-15,24-31"),
                    ("All", ""),
                ] {
                    if ui.add_enabled(self.default_affinity_enabled, egui::Button::new(label)).clicked() {
                        self.default_affinity_text = val.to_string();
                        self.default_affinity_enabled = true;
                    }
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
                    ui.add(egui::DragValue::new(&mut self.config.monitor.display_refresh_interval_ms)
                        .range(500..=10000).suffix("ms"));
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

        // ── Autostart ─────────────────────────────────────────────────────
        group_box(ui, "Autostart", |ui| {
            ui.checkbox(&mut self.autostart_enabled,
                "Start Process Lasso automatically with your desktop session");
        });

        ui.add_space(4.0);
        if ui.add_sized([ui.available_width(), 28.0], egui::Button::new("Apply Autostart Setting")).clicked() {
            if self.autostart_enabled {
                match write_systemd_unit() {
                    Ok(_) => self.status = "Autostart enabled (systemd user service installed).".into(),
                    Err(e) => self.status = format!("Autostart failed: {e}"),
                }
            } else {
                match disable_systemd_unit() {
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

fn check_autostart_enabled() -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "is-enabled", "process-lasso.service"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "enabled")
        .unwrap_or(false)
}

fn write_systemd_unit() -> std::io::Result<()> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = format!("{home}/.config/systemd/user");
    std::fs::create_dir_all(&dir)?;
    let exe = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("process-lasso"));
    let unit = format!(
        "[Unit]\nDescription=Process Lasso Linux\nAfter=graphical-session.target\n\n\
         [Service]\nExecStart={}\nRestart=on-failure\n\n\
         [Install]\nWantedBy=graphical-session.target\n",
        exe.display()
    );
    std::fs::write(format!("{dir}/process-lasso.service"), unit)?;
    std::process::Command::new("systemctl")
        .args(["--user", "enable", "process-lasso.service"])
        .output()?;
    Ok(())
}

fn disable_systemd_unit() -> std::io::Result<()> {
    std::process::Command::new("systemctl")
        .args(["--user", "disable", "process-lasso.service"])
        .output()?;
    Ok(())
}
