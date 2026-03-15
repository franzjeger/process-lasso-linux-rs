//! Rules tab: toolbar + sortable rule table.

use std::sync::{Arc, Mutex};

use egui::RichText;
use egui_extras::{Column, TableBuilder};

use crate::rules::{Rule, RuleEngine};
use crate::gui::dialogs::{RuleEditDialog, RulePresetsDialog};

pub struct RulesTab {
    pub selected_rule_id: Option<String>,
    pub edit_dialog:      Option<RuleEditDialog>,
    pub presets_dialog:   Option<RulePresetsDialog>,
    pub status:           String,
}

impl RulesTab {
    pub fn new() -> Self {
        Self {
            selected_rule_id: None,
            edit_dialog:      None,
            presets_dialog:   None,
            status:           String::new(),
        }
    }

    pub fn open_add_dialog(&mut self, template: Option<Rule>) {
        self.edit_dialog = Some(RuleEditDialog::new(template.unwrap_or_else(Rule::new_empty)));
    }

    pub fn show(
        &mut self,
        ui:              &mut egui::Ui,
        ctx:             &egui::Context,
        rule_engine:     &Arc<Mutex<RuleEngine>>,
        on_rules_changed: &mut bool,
        opacity:         f32,
    ) {
        let rules: Vec<Rule> = rule_engine.lock()
            .map(|re| re.get_rules().to_vec())
            .unwrap_or_default();

        let selected_id   = self.selected_rule_id.clone();
        let mut new_sel:  Option<String> = selected_id.clone();
        let mut open_edit: Option<Rule>  = None;
        let has_sel = self.selected_rule_id.is_some();

        // ── Toolbar ────────────────────────────────────────────────────────
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(0, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Add Rule").clicked() {
                        self.open_add_dialog(None);
                    }
                    if ui.button("Templates…").clicked() {
                        self.presets_dialog = Some(RulePresetsDialog::new());
                    }

                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);

                    if ui.add_enabled(has_sel, egui::Button::new("Edit")).clicked() {
                        if let (Some(id), Ok(re)) = (&self.selected_rule_id, rule_engine.lock()) {
                            if let Some(r) = re.get_rules().iter().find(|r| &r.rule_id == id) {
                                self.edit_dialog = Some(RuleEditDialog::new(r.clone()));
                            }
                        }
                    }

                    if ui.add_enabled(has_sel, egui::Button::new("Delete")).clicked() {
                        if let (Some(id), Ok(mut re)) = (self.selected_rule_id.clone(), rule_engine.lock()) {
                            re.remove_rule(&id);
                            *on_rules_changed = true;
                            self.selected_rule_id = None;
                            new_sel = None;
                        }
                    }

                    if ui.add_enabled(has_sel, egui::Button::new("Enable / Disable")).clicked() {
                        if let (Some(id), Ok(mut re)) = (&self.selected_rule_id, rule_engine.lock()) {
                            if let Some(r) = re.get_rules_mut().iter_mut().find(|r| &r.rule_id == id) {
                                r.enabled = !r.enabled;
                                *on_rules_changed = true;
                            }
                        }
                    }

                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);

                    if ui.button("Export…").clicked() {
                        self.export_rules(rule_engine);
                    }
                    if ui.button("Import…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("JSON", &["json"])
                            .pick_file()
                        {
                            self.import_rules(rule_engine, &path, on_rules_changed);
                        }
                    }
                });
            });

        if !self.status.is_empty() {
            ui.label(RichText::new(&self.status).color(ui.visuals().weak_text_color()));
        }

        ui.add_space(2.0);

        // ── Rule table ─────────────────────────────────────────────────────
        // Fixed column widths; Name stretches to fill.
        let fixed_w: f32 = 48.0 + 140.0 + 75.0 + 125.0 + 50.0 + 60.0 + 55.0; // = 553
        let name_w = (ui.available_width() - fixed_w - 20.0).max(100.0);

        let header_color  = ui.visuals().strong_text_color();
        let border_color  = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let text_color    = ui.visuals().text_color();
        let dim_color     = ui.visuals().weak_text_color();

        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, border_color))
            .inner_margin(egui::Margin::same(0))
            .show(ui, |ui| {
                TableBuilder::new(ui)
                    .striped(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::exact(48.0))      // Enabled
                    .column(Column::exact(name_w))    // Name
                    .column(Column::exact(140.0))     // Pattern
                    .column(Column::exact(75.0))      // Match
                    .column(Column::exact(125.0))     // Affinity
                    .column(Column::exact(50.0))      // Nice
                    .column(Column::exact(60.0))      // I/O Cls
                    .column(Column::exact(55.0))      // I/O Lvl
                    .min_scrolled_height(120.0)
                    .header(24.0, |mut hdr| {
                        for label in ["ON", "NAME", "PATTERN", "MATCH", "AFFINITY", "NICE", "I/O CLS", "I/O LVL"] {
                            hdr.col(|ui| {
                                ui.label(RichText::new(label).color(header_color).strong());
                            });
                        }
                    })
                    .body(|mut body| {
                        if rules.is_empty() {
                            body.row(32.0, |mut row| {
                                row.col(|_| {});
                                row.col(|ui| {
                                    ui.label(
                                        RichText::new("No rules yet — click \"Add Rule\" to create one.")
                                            .color(dim_color)
                                            .italics(),
                                    );
                                });
                                for _ in 2..8 { row.col(|_| {}); }
                            });
                        }

                        for rule in &rules {
                            let rule_id   = rule.rule_id.clone();
                            let is_sel    = selected_id.as_deref() == Some(&rule.rule_id);
                            let row_color = if rule.enabled { text_color } else { dim_color };

                            body.row(22.0, |mut row| {
                                row.set_selected(is_sel);

                                let en_text = if rule.enabled { "Yes" } else { "No" };
                                let (_, r0) = row.col(|ui| { ui.label(RichText::new(en_text).color(row_color)); });
                                let (_, r1) = row.col(|ui| { ui.label(RichText::new(&rule.name).color(row_color)); });
                                let (_, r2) = row.col(|ui| { ui.label(RichText::new(&rule.pattern).color(row_color)); });
                                let (_, r3) = row.col(|ui| { ui.label(RichText::new(&rule.match_type).color(row_color)); });
                                let (_, r4) = row.col(|ui| {
                                    ui.label(RichText::new(rule.affinity.as_deref().unwrap_or("")).color(row_color));
                                });
                                let (_, r5) = row.col(|ui| {
                                    ui.label(RichText::new(rule.nice.map(|n| n.to_string()).unwrap_or_default()).color(row_color));
                                });
                                let (_, r6) = row.col(|ui| {
                                    ui.label(RichText::new(rule.ionice_class.map(|c| c.to_string()).unwrap_or_default()).color(row_color));
                                });
                                let (_, r7) = row.col(|ui| {
                                    ui.label(RichText::new(rule.ionice_level.map(|l| l.to_string()).unwrap_or_default()).color(row_color));
                                });

                                let clicked = r0.clicked() || r1.clicked() || r2.clicked() || r3.clicked()
                                    || r4.clicked() || r5.clicked() || r6.clicked() || r7.clicked();
                                let doubled = r0.double_clicked() || r1.double_clicked() || r2.double_clicked()
                                    || r3.double_clicked() || r4.double_clicked() || r5.double_clicked()
                                    || r6.double_clicked() || r7.double_clicked();

                                if doubled {
                                    new_sel   = Some(rule_id.clone());
                                    open_edit = Some(rule.clone());
                                } else if clicked {
                                    new_sel = Some(rule_id.clone());
                                }
                            });
                        }
                    });
            });

        self.selected_rule_id = new_sel;
        if let Some(rule) = open_edit {
            self.edit_dialog = Some(RuleEditDialog::new(rule));
        }

        // ── Dialogs ────────────────────────────────────────────────────────
        if let Some(ref mut dlg) = self.edit_dialog {
            if let Some(result) = dlg.show(ctx, opacity) {
                self.edit_dialog = None;
                if let Some(rule) = result {
                    if let Ok(mut re) = rule_engine.lock() {
                        let exists = re.get_rules().iter().any(|r| r.rule_id == rule.rule_id);
                        if exists { re.update_rule(rule); } else { re.add_rule(rule); }
                        *on_rules_changed = true;
                    }
                }
            }
        }

        if let Some(ref mut dlg) = self.presets_dialog {
            if let Some(result) = dlg.show(ctx, opacity) {
                self.presets_dialog = None;
                if let Some(rule) = result {
                    self.edit_dialog = Some(RuleEditDialog::new(rule));
                }
            }
        }
    }

    fn export_rules(&mut self, rule_engine: &Arc<Mutex<RuleEngine>>) {
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name("process_lasso_rules.json")
            .add_filter("JSON", &["json"])
            .save_file()
        {
            let rules = rule_engine.lock()
                .map(|re| re.to_config_list())
                .unwrap_or_default();
            match serde_json::to_string_pretty(&rules) {
                Ok(text) => match std::fs::write(&path, &text) {
                    Ok(_)  => self.status = format!("Exported {} rules.", rules.len()),
                    Err(e) => self.status = format!("Export failed: {e}"),
                },
                Err(e) => self.status = format!("Serialise error: {e}"),
            }
        }
    }

    fn import_rules(
        &mut self,
        rule_engine: &Arc<Mutex<RuleEngine>>,
        path: &std::path::Path,
        on_rules_changed: &mut bool,
    ) {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Vec<crate::config::RuleConfig>>(&text) {
                Ok(configs) => {
                    let count = configs.len();
                    if let Ok(mut re) = rule_engine.lock() {
                        for cfg in configs { re.add_rule(Rule::from_config(&cfg)); }
                    }
                    *on_rules_changed = true;
                    self.status = format!("Imported {count} rules.");
                }
                Err(e) => self.status = format!("Parse error: {e}"),
            },
            Err(e) => self.status = format!("Read error: {e}"),
        }
    }
}
