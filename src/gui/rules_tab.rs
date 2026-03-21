//! Rules tab: toolbar + sortable rule table.

use std::sync::{Arc, Mutex};

use egui::RichText;
use egui_extras::{Column, TableBuilder};

use crate::rules::{Rule, RuleEngine};
use crate::gui::dialogs::{RuleEditDialog, RulePresetsDialog};

/// Result from a background file-dialog thread.
enum FileDialogResult {
    /// Export finished — carries a status string.
    ExportDone(String),
    /// Import finished — carries parsed rules or an error string.
    ImportDone(Result<Vec<crate::config::RuleConfig>, String>),
}

pub struct RulesTab {
    pub selected_rule_id: Option<String>,
    pub edit_dialog:      Option<RuleEditDialog>,
    pub presets_dialog:   Option<RulePresetsDialog>,
    pub status:           String,
    pub profile_name:     String,
    pub selected_profile: String,
    pub test_input:       String,
    /// Receives results from background file-dialog threads.
    file_rx: std::sync::mpsc::Receiver<FileDialogResult>,
    file_tx: std::sync::mpsc::Sender<FileDialogResult>,
    // Confirm dialog state
    confirm_delete_rule:    bool,
    confirm_load_profile:   bool,
    confirm_delete_profile: bool,
}

impl RulesTab {
    pub fn new() -> Self {
        let (file_tx, file_rx) = std::sync::mpsc::channel();
        Self {
            selected_rule_id: None,
            edit_dialog:      None,
            presets_dialog:   None,
            status:           String::new(),
            profile_name:     String::new(),
            selected_profile: String::new(),
            test_input:       String::new(),
            file_rx,
            file_tx,
            confirm_delete_rule:    false,
            confirm_load_profile:   false,
            confirm_delete_profile: false,
        }
    }

    pub fn open_add_dialog(&mut self, template: Option<Rule>) {
        self.edit_dialog = Some(RuleEditDialog::new(template.unwrap_or_else(Rule::new_empty)));
    }

    /// Returns `true` if rule_profiles in config changed (needs save).
    pub fn show(
        &mut self,
        ui:              &mut egui::Ui,
        ctx:             &egui::Context,
        rule_engine:     &Arc<Mutex<RuleEngine>>,
        on_rules_changed: &mut bool,
        opacity:         f32,
        rule_profiles:   &mut std::collections::HashMap<String, Vec<crate::config::RuleConfig>>,
        on_profiles_changed: &mut bool,
    ) {
        // ── Drain background file-dialog results ───────────────────────────
        while let Ok(result) = self.file_rx.try_recv() {
            match result {
                FileDialogResult::ExportDone(msg) => {
                    self.status = msg;
                }
                FileDialogResult::ImportDone(Ok(configs)) => {
                    let count = configs.len();
                    if let Ok(mut re) = rule_engine.lock() {
                        for cfg in configs { re.add_rule(Rule::from_config(&cfg)); }
                    }
                    *on_rules_changed = true;
                    self.status = format!("Imported {count} rules.");
                }
                FileDialogResult::ImportDone(Err(e)) => {
                    self.status = e;
                }
            }
        }

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
                        self.confirm_delete_rule = true;
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
                        let tx = self.file_tx.clone();
                        std::thread::spawn(move || {
                            let path = match crate::file_dialog::open("*.json") {
                                Some(p) => p,
                                None => return,
                            };
                            let result = match std::fs::read_to_string(&path) {
                                Err(e) => Err(format!("Read error: {e}")),
                                Ok(s) => serde_json::from_str::<Vec<crate::config::RuleConfig>>(&s)
                                    .map_err(|e| format!("Parse error: {e}")),
                            };
                            tx.send(FileDialogResult::ImportDone(result)).ok();
                        });
                    }

                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // ── Rule Profiles ────────────────────────────────────
                    ui.label(egui::RichText::new("Profile:").strong());
                    let profile_names: Vec<String> = {
                        let mut v: Vec<String> = rule_profiles.keys().cloned().collect();
                        v.sort();
                        v
                    };
                    egui::ComboBox::from_id_salt("profile_picker")
                        .selected_text(if self.selected_profile.is_empty() { "—" } else { &self.selected_profile })
                        .width(130.0)
                        .show_ui(ui, |ui| {
                            for name in &profile_names {
                                ui.selectable_value(&mut self.selected_profile, name.clone(), name.as_str());
                            }
                        });

                    if ui.add_enabled(!self.selected_profile.is_empty(), egui::Button::new("Load")).clicked() {
                        self.confirm_load_profile = true;
                    }

                    if ui.add_enabled(!self.selected_profile.is_empty(), egui::Button::new("Delete")).clicked() {
                        self.confirm_delete_profile = true;
                    }

                    ui.separator();
                    ui.add(egui::TextEdit::singleline(&mut self.profile_name)
                        .hint_text("New profile name…")
                        .desired_width(130.0));
                    if ui.add_enabled(!self.profile_name.trim().is_empty(), egui::Button::new("Save as Profile")).clicked() {
                        let name = self.profile_name.trim().to_string();
                        let rules = rule_engine.lock()
                            .map(|re| re.to_config_list())
                            .unwrap_or_default();
                        rule_profiles.insert(name.clone(), rules);
                        self.selected_profile = name.clone();
                        self.profile_name.clear();
                        *on_profiles_changed = true;
                        self.status = format!("Saved as profile '{name}'.");
                    }

                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Test pattern:").strong());
                    ui.text_edit_singleline(&mut self.test_input);
                    if !self.test_input.is_empty() {
                        let matches: Vec<String> = rules.iter()
                            .filter(|r| r.enabled && r.matches(&self.test_input))
                            .map(|r| r.name.clone())
                            .collect();
                        if matches.is_empty() {
                            ui.colored_label(ui.visuals().weak_text_color(), "No rules match");
                        } else {
                            ui.colored_label(egui::Color32::from_rgb(100, 200, 140),
                                format!("Matches: {}", matches.join(", ")));
                        }
                    }
                });
            });

        if !self.status.is_empty() {
            ui.label(RichText::new(&self.status).color(ui.visuals().weak_text_color()));
        }

        ui.add_space(2.0);

        // ── Rule table ─────────────────────────────────────────────────────
        // NAME uses Column::remainder() to fill all remaining space; all other
        // columns are resizable (drag the divider in the header).
        let header_color  = ui.visuals().strong_text_color();
        let border_color  = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let text_color    = ui.visuals().text_color();
        let dim_color     = ui.visuals().weak_text_color();

        // Compute column widths proportional to available width.
        // id_salt includes avail_w so egui_extras re-initialises columns on window resize.
        let avail_w = ui.available_width() - 2.0;
        let col_pattern = (avail_w * 0.13).clamp(80.0, 200.0);
        let col_match   = (avail_w * 0.07).clamp(55.0, 110.0);
        let col_aff     = (avail_w * 0.11).clamp(70.0, 160.0);
        let col_nice    = (avail_w * 0.04).clamp(38.0,  60.0);
        let col_iocls   = (avail_w * 0.05).clamp(50.0,  80.0);
        let col_iolv    = (avail_w * 0.05).clamp(45.0,  75.0);

        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, border_color))
            .inner_margin(egui::Margin::same(1))
            .show(ui, |ui| {
                TableBuilder::new(ui)
                    .id_salt(avail_w as i32)   // reset stored widths when window resizes
                    .striped(true)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::exact(48.0))
                    .column(Column::remainder())
                    .column(Column::initial(col_pattern).clip(true))
                    .column(Column::initial(col_match).clip(true))
                    .column(Column::initial(col_aff).clip(true))
                    .column(Column::initial(col_nice).clip(true))
                    .column(Column::initial(col_iocls).clip(true))
                    .column(Column::initial(col_iolv).clip(true))
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

        // ── Confirm dialogs ────────────────────────────────────────────────
        if self.confirm_delete_rule {
            let rule_name = self.selected_rule_id.as_ref()
                .and_then(|id| rules.iter().find(|r| &r.rule_id == id))
                .map(|r| r.name.as_str())
                .unwrap_or("this rule");
            let mut confirmed = false;
            let mut cancelled = false;
            egui::Window::new("Confirm Delete Rule")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!("Delete rule '{rule_name}'?"));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() { confirmed = true; }
                        if ui.button("Cancel").clicked() { cancelled = true; }
                    });
                });
            if confirmed {
                if let (Some(id), Ok(mut re)) = (self.selected_rule_id.clone(), rule_engine.lock()) {
                    re.remove_rule(&id);
                    *on_rules_changed = true;
                    self.selected_rule_id = None;
                }
                self.confirm_delete_rule = false;
            } else if cancelled {
                self.confirm_delete_rule = false;
            }
        }

        if self.confirm_load_profile {
            let profile = self.selected_profile.clone();
            let mut confirmed = false;
            let mut cancelled = false;
            egui::Window::new("Confirm Load Profile")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!("Load profile '{profile}'?\nThis replaces all current rules."));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Load").clicked() { confirmed = true; }
                        if ui.button("Cancel").clicked() { cancelled = true; }
                    });
                });
            if confirmed {
                if let Some(rules) = rule_profiles.get(&self.selected_profile) {
                    if let Ok(mut re) = rule_engine.lock() {
                        re.clear_rules();
                        for cfg in rules { re.add_rule(crate::rules::Rule::from_config(cfg)); }
                    }
                    *on_rules_changed = true;
                    self.status = format!("Loaded profile '{}'.", self.selected_profile);
                }
                self.confirm_load_profile = false;
            } else if cancelled {
                self.confirm_load_profile = false;
            }
        }

        if self.confirm_delete_profile {
            let profile = self.selected_profile.clone();
            let mut confirmed = false;
            let mut cancelled = false;
            egui::Window::new("Confirm Delete Profile")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!("Delete profile '{profile}'?"));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() { confirmed = true; }
                        if ui.button("Cancel").clicked() { cancelled = true; }
                    });
                });
            if confirmed {
                rule_profiles.remove(&self.selected_profile);
                self.selected_profile.clear();
                *on_profiles_changed = true;
                self.status = "Profile deleted.".into();
                self.confirm_delete_profile = false;
            } else if cancelled {
                self.confirm_delete_profile = false;
            }
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
        let rules = rule_engine.lock()
            .map(|re| re.to_config_list())
            .unwrap_or_default();
        let tx = self.file_tx.clone();
        std::thread::spawn(move || {
            let path = match crate::file_dialog::save("argus_lasso_rules.json", "*.json") {
                Some(p) => p,
                None => return,
            };
            let msg = match serde_json::to_string_pretty(&rules) {
                Err(e) => format!("Serialise error: {e}"),
                Ok(text) => match std::fs::write(&path, &text) {
                    Ok(_)  => format!("Exported {} rules.", rules.len()),
                    Err(e) => format!("Export failed: {e}"),
                },
            };
            tx.send(FileDialogResult::ExportDone(msg)).ok();
        });
    }
}

