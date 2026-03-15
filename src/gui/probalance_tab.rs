//! ProBalance settings tab — two-column form layout matching the Python version.

use egui::Ui;
use crate::config::ProBalanceConfig;

pub struct ProBalanceTab {
    pub cfg: ProBalanceConfig,
    pub new_exempt: String,
    pub selected_exempt: Option<usize>,
    pub status: String,
}

impl ProBalanceTab {
    pub fn new(cfg: ProBalanceConfig) -> Self {
        Self { cfg, new_exempt: String::new(), selected_exempt: None, status: String::new() }
    }

    /// Returns Some(updated_config) when Apply is clicked.
    pub fn show(&mut self, ui: &mut Ui) -> Option<ProBalanceConfig> {
        const LABEL_W: f32 = 280.0;

        ui.checkbox(&mut self.cfg.enabled, "ProBalance Enabled");
        ui.add_space(8.0);

        // ── Throttle Settings ─────────────────────────────────────────────
        group_box(ui, "Throttle Settings", |ui| {
            egui::Grid::new("probalance_throttle")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    right_label(ui, LABEL_W, "CPU threshold:");
                    ui.add(egui::DragValue::new(&mut self.cfg.cpu_threshold_percent)
                        .range(10.0f32..=100.0).suffix("%"));
                    ui.end_row();

                    right_label(ui, LABEL_W, "Consecutive seconds above threshold:");
                    ui.add(egui::DragValue::new(&mut self.cfg.consecutive_seconds)
                        .range(1.0f32..=60.0).suffix("s"));
                    ui.end_row();

                    right_label(ui, LABEL_W, "Nice adjustment (added on throttle):");
                    ui.add(egui::DragValue::new(&mut self.cfg.nice_adjustment).range(1..=19));
                    ui.end_row();

                    right_label(ui, LABEL_W, "Nice floor (max nice applied):");
                    ui.add(egui::DragValue::new(&mut self.cfg.nice_floor).range(1..=19));
                    ui.end_row();
                });
        });

        ui.add_space(10.0);

        // ── Restore Settings ──────────────────────────────────────────────
        group_box(ui, "Restore Settings", |ui| {
            egui::Grid::new("probalance_restore")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    right_label(ui, LABEL_W, "Restore when CPU below:");
                    ui.add(egui::DragValue::new(&mut self.cfg.restore_threshold_percent)
                        .range(1.0f32..=99.0).suffix("%"));
                    ui.end_row();

                    right_label(ui, LABEL_W, "Restore hysteresis (seconds below restore threshold):");
                    ui.add(egui::DragValue::new(&mut self.cfg.restore_hysteresis_seconds)
                        .range(1.0f32..=120.0).suffix("s"));
                    ui.end_row();
                });
        });

        ui.add_space(10.0);

        // ── Exempt Processes ──────────────────────────────────────────────
        group_box(ui, "Exempt Processes (pattern contains)", |ui| {
            egui::ScrollArea::vertical().max_height(140.0).id_salt("exempt_scroll").show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                for (i, pat) in self.cfg.exempt_patterns.iter().enumerate() {
                    let is_sel = self.selected_exempt == Some(i);
                    if ui.selectable_label(is_sel, egui::RichText::new(pat).color(ui.visuals().text_color())).clicked() {
                        self.selected_exempt = if is_sel { None } else { Some(i) };
                    }
                }
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let avail = ui.available_width();
                let btn_w = 90.0 + 110.0 + 8.0 * 2.0; // Add + Remove selected + spacing
                ui.add(egui::TextEdit::singleline(&mut self.new_exempt)
                    .hint_text("Pattern to exempt...")
                    .desired_width((avail - btn_w).max(80.0)));
                if ui.button("Add").clicked() && !self.new_exempt.trim().is_empty() {
                    self.cfg.exempt_patterns.push(self.new_exempt.trim().to_string());
                    self.new_exempt.clear();
                }
                let has_sel = self.selected_exempt.is_some();
                if ui.add_enabled(has_sel, egui::Button::new("Remove selected")).clicked() {
                    if let Some(i) = self.selected_exempt {
                        self.cfg.exempt_patterns.remove(i);
                        self.selected_exempt = None;
                    }
                }
            });
        });

        ui.add_space(12.0);

        if !self.status.is_empty() {
            ui.colored_label(ui.visuals().weak_text_color(), &self.status);
            ui.add_space(4.0);
        }

        // Full-width Apply button
        if ui.add_sized([ui.available_width(), 28.0], egui::Button::new("Apply Settings")).clicked() {
            self.status = "Settings applied.".into();
            return Some(self.cfg.clone());
        }

        None
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

/// Render a right-aligned label in a fixed-width grid cell.
fn right_label(ui: &mut Ui, width: f32, text: &str) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.set_min_width(width);
        ui.label(text);
    });
}
