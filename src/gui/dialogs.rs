//! Modal dialogs: affinity picker, nice dialog, ionice dialog, rule edit,
//! process picker, Steam game picker, Lutris game picker, rule presets.

use std::collections::HashSet;
use egui::{Context, Ui, ViewportBuilder, ViewportId};

use crate::cpu_park::{detect_topology, get_smt_siblings_of};
use crate::gui::theme::Breeze;
use crate::rules::Rule;
use crate::utils::{cpulist_to_set, cpuset_to_cpulist, get_cpu_count, get_offline_cpus};

// ── AffinityPicker (reusable inline widget) ───────────────────────────────────

/// Inline CPU-affinity picker: CCD buttons, physical+HT checkbox pairs, convenience buttons.
/// Renders inside any existing UI panel — no window wrapping.
pub struct AffinityPicker {
    pub checkboxes: Vec<bool>,
    pub cpu_count: u32,
    pub offline: HashSet<u32>,
    pub preferred: HashSet<u32>,
    pub non_preferred: HashSet<u32>,
    pub smt_siblings: HashSet<u32>,
}

impl AffinityPicker {
    /// Initialise from an existing cpulist string (empty = all CPUs selected).
    pub fn new(current_affinity: &str) -> Self {
        let cpu_count = get_cpu_count();
        let offline = get_offline_cpus();
        let topo = detect_topology();
        let all_cpus: HashSet<u32> = (0..cpu_count).collect();
        let smt_siblings = get_smt_siblings_of(&all_cpus);

        let selected = if current_affinity.trim().is_empty() {
            (0..cpu_count).collect::<HashSet<_>>()
        } else {
            cpulist_to_set(current_affinity).unwrap_or_default()
        };

        let checkboxes = (0..cpu_count as usize)
            .map(|i| selected.contains(&(i as u32)))
            .collect();

        let has_asym = topo.has_asymmetry();
        Self {
            checkboxes,
            cpu_count,
            offline,
            preferred: if has_asym { topo.preferred } else { HashSet::new() },
            non_preferred: if has_asym { topo.non_preferred } else { HashSet::new() },
            smt_siblings,
        }
    }

    /// Current cpulist string computed from checkbox state.
    pub fn cpulist(&self) -> String {
        let selected: HashSet<u32> = self.checkboxes.iter()
            .enumerate()
            .filter(|(_, &b)| b)
            .map(|(i, _)| i as u32)
            .collect();
        cpuset_to_cpulist(&selected)
    }

    /// Render the picker inline. Call inside any ui.group() / ui.vertical() etc.
    pub fn show(&mut self, ui: &mut Ui) {
        // Parked CPUs warning
        if !self.offline.is_empty() {
            let offline_str = cpuset_to_cpulist(&self.offline);
            ui.colored_label(
                Breeze::WARNING,
                format!("CPUs {offline_str} are parked"),
            );
            ui.add_space(2.0);
        }

        // CCD / topology quick buttons
        if !self.preferred.is_empty() {
            ui.horizontal(|ui| {
                ui.label("Quick:");
                if ui.button("CCD0 (preferred)").clicked() {
                    for (i, cb) in self.checkboxes.iter_mut().enumerate() {
                        *cb = self.preferred.contains(&(i as u32));
                    }
                }
                if ui.button("CCD1 (non-preferred)").clicked() {
                    for (i, cb) in self.checkboxes.iter_mut().enumerate() {
                        *cb = self.non_preferred.contains(&(i as u32));
                    }
                }
                if ui.button("All cores").clicked() {
                    for (i, cb) in self.checkboxes.iter_mut().enumerate() {
                        if !self.offline.contains(&(i as u32)) { *cb = true; }
                    }
                }
                let has_smt = !self.smt_siblings.is_empty();
                ui.add_enabled_ui(has_smt, |ui| {
                    if ui.button("No SMT").clicked() {
                        for (i, cb) in self.checkboxes.iter_mut().enumerate() {
                            *cb = !self.smt_siblings.contains(&(i as u32))
                                && !self.offline.contains(&(i as u32));
                        }
                    }
                });
            });
        } else {
            ui.horizontal(|ui| {
                ui.label("Quick:");
                if ui.button("All cores").clicked() {
                    for (i, cb) in self.checkboxes.iter_mut().enumerate() {
                        if !self.offline.contains(&(i as u32)) { *cb = true; }
                    }
                }
                let has_smt = !self.smt_siblings.is_empty();
                ui.add_enabled_ui(has_smt, |ui| {
                    if ui.button("No SMT").clicked() {
                        for (i, cb) in self.checkboxes.iter_mut().enumerate() {
                            *cb = !self.smt_siblings.contains(&(i as u32))
                                && !self.offline.contains(&(i as u32));
                        }
                    }
                });
                if ui.button("None").clicked() {
                    for cb in &mut self.checkboxes { *cb = false; }
                }
            });
        }

        ui.add_space(4.0);

        // Core checkbox grid — physical+HT pairs side by side
        egui::ScrollArea::vertical().max_height(180.0).id_salt("aff_picker_scroll").show(ui, |ui| {
            if !self.preferred.is_empty() {
                // Topology-aware sections
                let phys_pref: Vec<u32> = {
                    let mut v: Vec<u32> = self.preferred.iter()
                        .filter(|c| !self.smt_siblings.contains(c))
                        .copied().collect();
                    v.sort_unstable();
                    v
                };
                let phys_npref: Vec<u32> = {
                    let mut v: Vec<u32> = self.non_preferred.iter()
                        .filter(|c| !self.smt_siblings.contains(c))
                        .copied().collect();
                    v.sort_unstable();
                    v
                };

                if !phys_pref.is_empty() {
                    ui.colored_label(Breeze::HIGHLIGHT, "Preferred CCD");
                    self.show_core_pairs(ui, &phys_pref);
                }
                if !phys_npref.is_empty() {
                    ui.colored_label(ui.visuals().weak_text_color(), "Non-preferred CCD");
                    self.show_core_pairs(ui, &phys_npref);
                }
            } else {
                // Flat grid of physical+HT pairs
                let all_phys: Vec<u32> = {
                    let mut v: Vec<u32> = (0..self.cpu_count)
                        .filter(|c| !self.smt_siblings.contains(c))
                        .collect();
                    v.sort_unstable();
                    v
                };
                if all_phys.is_empty() {
                    // No topology info — flat grid
                    egui::Grid::new("aff_flat_grid").num_columns(8).show(ui, |ui| {
                        for i in 0..self.cpu_count as usize {
                            let offline = self.offline.contains(&(i as u32));
                            ui.add_enabled(!offline, egui::Checkbox::new(&mut self.checkboxes[i], i.to_string()));
                            if (i + 1) % 8 == 0 { ui.end_row(); }
                        }
                    });
                } else {
                    self.show_core_pairs(ui, &all_phys);
                }
            }
        });

        // Read-only cpulist result
        let cpulist = self.cpulist();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Result:").color(ui.visuals().weak_text_color()));
            ui.label(egui::RichText::new(if cpulist.is_empty() { "(none)" } else { &cpulist }).color(Breeze::HIGHLIGHT));
        });
    }

    /// Render physical+HT checkbox pairs using horizontal_wrapped for responsive layout.
    fn show_core_pairs(&mut self, ui: &mut Ui, phys_cpus: &[u32]) {
        ui.horizontal_wrapped(|ui| {
            for &phys in phys_cpus {
                let p = phys as usize;
                let p_offline = self.offline.contains(&phys);
                let sibling = self.find_sibling(phys);

                // Render the physical+HT pair inside a small horizontal group
                ui.add_enabled(!p_offline, egui::Checkbox::new(&mut self.checkboxes[p], format!("{phys}")));

                if let Some(s) = sibling {
                    let s_usize = s as usize;
                    let s_offline = self.offline.contains(&s);
                    ui.add_enabled(!s_offline, egui::Checkbox::new(&mut self.checkboxes[s_usize], format!("HT{s}")));
                }

                // Visual separator between pairs
                ui.separator();
            }
        });
    }

    /// Find the HT sibling for a physical CPU by looking for a CPU in smt_siblings
    /// that has the same core topology. We use the half-offset heuristic as a fast path
    /// (correct for most AMD and Intel layouts).
    fn find_sibling(&self, phys: u32) -> Option<u32> {
        // Fast path: sibling = phys + N/2 where N = total logical CPUs
        let half = self.cpu_count / 2;
        if half > 0 {
            let candidate = phys + half;
            if candidate < self.cpu_count && self.smt_siblings.contains(&candidate) {
                return Some(candidate);
            }
        }
        // Slow path: scan smt_siblings for the one closest to phys
        self.smt_siblings.iter()
            .filter(|&&s| s > phys && s < phys + self.cpu_count)
            .min_by_key(|&&s| s.wrapping_sub(phys))
            .copied()
    }
}

// ── AffinityDialog ────────────────────────────────────────────────────────────

pub struct AffinityDialog {
    pub open: bool,
    pub title: String,
    pub checkboxes: Vec<bool>,  // indexed by cpu number
    pub cpu_count: u32,
    pub offline: HashSet<u32>,
    pub preferred: HashSet<u32>,
    pub non_preferred: HashSet<u32>,
    pub smt_siblings: HashSet<u32>,
    pub result: Option<String>,
}

impl AffinityDialog {
    pub fn new(current_affinity: &str, title: &str) -> Self {
        let cpu_count = get_cpu_count();
        let offline = get_offline_cpus();
        let topo = detect_topology();
        let all_cpus: HashSet<u32> = (0..cpu_count).collect();
        let smt_siblings = get_smt_siblings_of(&all_cpus);

        let selected = if current_affinity.is_empty() {
            (0..cpu_count).collect::<HashSet<_>>()
        } else {
            cpulist_to_set(current_affinity).unwrap_or_default()
        };

        let checkboxes = (0..cpu_count as usize)
            .map(|i| selected.contains(&(i as u32)))
            .collect();

        Self {
            open: true,
            title: title.to_string(),
            checkboxes,
            cpu_count,
            offline,
            preferred: if topo.has_asymmetry() { topo.preferred.clone() } else { HashSet::new() },
            non_preferred: if topo.has_asymmetry() { topo.non_preferred.clone() } else { HashSet::new() },
            smt_siblings,
            result: None,
        }
    }

    /// Returns Some(cpulist) when accepted, None while open, Some("") when cancelled.
    pub fn show(&mut self, ctx: &Context, opacity: f32) -> Option<String> {
        if !self.open {
            return self.result.clone();
        }

        let mut close_as: Option<bool> = None; // Some(true)=accept, Some(false)=cancel

        {
            let title_str = format!("Set CPU Affinity — {}", self.title);
            let checkboxes  = &mut self.checkboxes;
            let offline     = &self.offline;
            let preferred   = &self.preferred;
            let non_preferred = &self.non_preferred;
            let smt_siblings = &self.smt_siblings;
            let cpu_count   = self.cpu_count;

            ctx.show_viewport_immediate(
                ViewportId::from_hash_of("affinity_dialog"),
                ViewportBuilder::default()
                    .with_title(title_str)
                    .with_app_id("argus-lasso")
                    .with_icon(egui::IconData { rgba: crate::icon::RGBA.to_vec(), width: crate::icon::W, height: crate::icon::H })
                    .with_min_inner_size([500.0, 440.0])
                    .with_transparent(true)
                    .with_resizable(true),
                |ctx, _class| {
                    let _opacity_saved = crate::gui::theme::push_viewport_opacity(ctx, opacity);
                    if ctx.input(|i| i.viewport().close_requested()) {
                        close_as = Some(false);
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        if !offline.is_empty() {
                            let offline_str = cpuset_to_cpulist(offline);
                            ui.colored_label(
                                Breeze::WARNING,
                                format!("CPUs {offline_str} are parked — disable Gaming Mode to use them."),
                            );
                            ui.add_space(4.0);
                        }

                        ui.group(|ui| {
                            ui.label("Select CPUs:");
                            ui.add_space(4.0);

                            if !preferred.is_empty() && !non_preferred.is_empty() {
                                let ccd0_name = "Preferred CCD";
                                let ccd1_name = "Non-preferred CCD (parked in Gaming Mode)";
                                let pref_phys: Vec<u32> = preferred.iter()
                                    .copied().filter(|c| !smt_siblings.contains(c))
                                    .collect::<std::collections::BTreeSet<_>>().into_iter().collect();
                                let pref_ht: Vec<u32> = preferred.iter()
                                    .copied().filter(|c| smt_siblings.contains(c))
                                    .collect::<std::collections::BTreeSet<_>>().into_iter().collect();
                                let npref_phys: Vec<u32> = non_preferred.iter()
                                    .copied().filter(|c| !smt_siblings.contains(c))
                                    .collect::<std::collections::BTreeSet<_>>().into_iter().collect();
                                let npref_ht: Vec<u32> = non_preferred.iter()
                                    .copied().filter(|c| smt_siblings.contains(c))
                                    .collect::<std::collections::BTreeSet<_>>().into_iter().collect();
                                for (section_name, cpus) in [
                                    (format!("{ccd0_name} — physical cores"), pref_phys),
                                    (format!("{ccd0_name} — HT siblings"), pref_ht),
                                    (format!("{ccd1_name} — physical cores"), npref_phys),
                                    (format!("{ccd1_name} — HT siblings"), npref_ht),
                                ] {
                                    if cpus.is_empty() { continue; }
                                    ui.colored_label(Breeze::HIGHLIGHT, &section_name);
                                    ui.horizontal_wrapped(|ui| {
                                        for cpu in &cpus {
                                            let c = *cpu as usize;
                                            if c < checkboxes.len() {
                                                ui.add_enabled(
                                                    !offline.contains(cpu),
                                                    egui::Checkbox::new(&mut checkboxes[c], cpu.to_string()),
                                                );
                                            }
                                        }
                                    });
                                }
                            } else {
                                egui::Grid::new("cpu_grid").num_columns(8).show(ui, |ui| {
                                    for i in 0..cpu_count as usize {
                                        ui.add_enabled(
                                            !offline.contains(&(i as u32)),
                                            egui::Checkbox::new(&mut checkboxes[i], i.to_string()),
                                        );
                                        if (i + 1) % 8 == 0 { ui.end_row(); }
                                    }
                                });
                            }
                        });

                        ui.horizontal(|ui| {
                            if ui.button("All").clicked() {
                                for (i, cb) in checkboxes.iter_mut().enumerate() {
                                    if !offline.contains(&(i as u32)) { *cb = true; }
                                }
                            }
                            if ui.button("None").clicked() {
                                for (i, cb) in checkboxes.iter_mut().enumerate() {
                                    if !offline.contains(&(i as u32)) { *cb = false; }
                                }
                            }
                            if !preferred.is_empty() {
                                if ui.button("Preferred CCD").clicked() {
                                    for (i, cb) in checkboxes.iter_mut().enumerate() {
                                        *cb = preferred.contains(&(i as u32));
                                    }
                                }
                                if ui.button("Non-preferred CCD").clicked() {
                                    for (i, cb) in checkboxes.iter_mut().enumerate() {
                                        *cb = non_preferred.contains(&(i as u32));
                                    }
                                }
                            }
                        });

                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("OK").clicked() {
                                close_as = Some(true);
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            if ui.button("Cancel").clicked() {
                                close_as = Some(false);
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        });
                    });
                    crate::gui::theme::pop_viewport_opacity(ctx, _opacity_saved);
                },
            );
        }

        if let Some(accept) = close_as {
            self.open = false;
            if accept {
                let selected: HashSet<u32> = self.checkboxes.iter()
                    .enumerate()
                    .filter(|(_, &b)| b)
                    .map(|(i, _)| i as u32)
                    .collect();
                if !selected.is_empty() {
                    let cpulist = cpuset_to_cpulist(&selected);
                    self.result = Some(cpulist.clone());
                    return Some(cpulist);
                }
                // empty selection — keep open
                self.open = true;
            } else {
                self.result = Some(String::new());
                return Some(String::new());
            }
        }
        None
    }
}

// ── NiceDialog ────────────────────────────────────────────────────────────────

pub struct NiceDialog {
    pub open: bool,
    pub title: String,
    pub value: i32,
    pub result: Option<Option<i32>>,  // None = cancelled, Some(v) = accepted
}

impl NiceDialog {
    pub fn new(current: i32, title: &str) -> Self {
        Self { open: true, title: title.to_string(), value: current, result: None }
    }

    pub fn show(&mut self, ctx: &Context, opacity: f32) -> Option<Option<i32>> {
        if !self.open {
            return self.result.clone();
        }

        let mut close_as: Option<bool> = None;

        {
            let title_str = format!("Set Priority (nice) — {}", self.title);
            let value = &mut self.value;

            ctx.show_viewport_immediate(
                ViewportId::from_hash_of("nice_dialog"),
                ViewportBuilder::default()
                    .with_title(title_str)
                    .with_app_id("argus-lasso")
                    .with_icon(egui::IconData { rgba: crate::icon::RGBA.to_vec(), width: crate::icon::W, height: crate::icon::H })
                    .with_min_inner_size([380.0, 160.0])
                    .with_transparent(true)
                    .with_resizable(false),
                |ctx, _class| {
                    let _opacity_saved = crate::gui::theme::push_viewport_opacity(ctx, opacity);
                    if ctx.input(|i| i.viewport().close_requested()) {
                        close_as = Some(false);
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.label("Nice priority: lower = higher priority. Negative values require root.");
                        ui.add(egui::Slider::new(value, -20..=19).text("nice"));
                        ui.horizontal(|ui| {
                            for (label, val) in [
                                ("High (-10)", -10i32), ("Normal (0)", 0),
                                ("Low (5)", 5), ("Very Low (15)", 15), ("Idle (19)", 19),
                            ] {
                                if ui.small_button(label).clicked() { *value = val; }
                            }
                        });
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("OK").clicked() {
                                close_as = Some(true);
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            if ui.button("Cancel").clicked() {
                                close_as = Some(false);
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        });
                    });
                    crate::gui::theme::pop_viewport_opacity(ctx, _opacity_saved);
                },
            );
        }

        if let Some(accept) = close_as {
            self.open = false;
            if accept {
                self.result = Some(Some(self.value));
                return Some(Some(self.value));
            } else {
                self.result = Some(None);
                return Some(None);
            }
        }
        None
    }
}

// ── IoNiceDialog ──────────────────────────────────────────────────────────────

pub struct IoNiceDialog {
    pub open: bool,
    pub title: String,
    pub class: i32,
    pub level: i32,
    pub result: Option<Option<(i32, i32)>>,
}

impl IoNiceDialog {
    pub fn new(title: &str) -> Self {
        Self { open: true, title: title.to_string(), class: 2, level: 4, result: None }
    }

    pub fn show(&mut self, ctx: &Context, opacity: f32) -> Option<Option<(i32, i32)>> {
        if !self.open { return self.result.clone(); }

        let mut close_as: Option<bool> = None;

        {
            let title_str = format!("Set I/O Priority — {}", self.title);
            let class = &mut self.class;
            let level = &mut self.level;

            ctx.show_viewport_immediate(
                ViewportId::from_hash_of("ionice_dialog"),
                ViewportBuilder::default()
                    .with_title(title_str)
                    .with_app_id("argus-lasso")
                    .with_icon(egui::IconData { rgba: crate::icon::RGBA.to_vec(), width: crate::icon::W, height: crate::icon::H })
                    .with_min_inner_size([340.0, 160.0])
                    .with_transparent(true)
                    .with_resizable(false),
                |ctx, _class| {
                    let _opacity_saved = crate::gui::theme::push_viewport_opacity(ctx, opacity);
                    if ctx.input(|i| i.viewport().close_requested()) {
                        close_as = Some(false);
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.label("I/O class: Realtime requires root. Level 0=highest, 7=lowest.");
                        egui::ComboBox::from_label("I/O Class")
                            .selected_text(match *class {
                                0 => "None (default)",
                                1 => "Realtime (root)",
                                2 => "Best-effort",
                                3 => "Idle",
                                _ => "?",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(class, 0, "None (default)");
                                ui.selectable_value(class, 1, "Realtime (root)");
                                ui.selectable_value(class, 2, "Best-effort");
                                ui.selectable_value(class, 3, "Idle");
                            });
                        if *class == 1 || *class == 2 {
                            ui.add(egui::Slider::new(level, 0..=7).text("level"));
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("OK").clicked() {
                                close_as = Some(true);
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            if ui.button("Cancel").clicked() {
                                close_as = Some(false);
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        });
                    });
                    crate::gui::theme::pop_viewport_opacity(ctx, _opacity_saved);
                },
            );
        }

        if let Some(accept) = close_as {
            self.open = false;
            if accept {
                self.result = Some(Some((self.class, self.level)));
                return Some(Some((self.class, self.level)));
            } else {
                self.result = Some(None);
                return Some(None);
            }
        }
        None
    }
}

// ── RuleEditDialog ────────────────────────────────────────────────────────────

pub struct RuleEditDialog {
    pub open: bool,
    pub rule: Rule,
    pub affinity_enabled: bool,
    pub nice_enabled: bool,
    pub ionice_enabled: bool,
    pub affinity_picker: AffinityPicker,
    pub result: Option<Option<Rule>>,
}

impl RuleEditDialog {
    pub fn new(template: Rule) -> Self {
        let affinity_enabled = template.affinity.is_some();
        let nice_enabled = template.nice.is_some();
        let ionice_enabled = template.ionice_class.is_some();
        let picker_init = template.affinity.as_deref().unwrap_or("");
        Self {
            open: true,
            affinity_picker: AffinityPicker::new(picker_init),
            rule: template,
            affinity_enabled,
            nice_enabled,
            ionice_enabled,
            result: None,
        }
    }

    pub fn show(&mut self, ctx: &Context, opacity: f32) -> Option<Option<Rule>> {
        if !self.open { return self.result.clone(); }

        let mut close_as: Option<bool> = None;

        {
            let title_str = if self.rule.name.is_empty() {
                "Add Rule".to_string()
            } else {
                format!("Edit Rule — {}", self.rule.name)
            };
            let rule             = &mut self.rule;
            let affinity_enabled = &mut self.affinity_enabled;
            let nice_enabled     = &mut self.nice_enabled;
            let ionice_enabled   = &mut self.ionice_enabled;
            let picker           = &mut self.affinity_picker;

            ctx.show_viewport_immediate(
                ViewportId::from_hash_of("rule_edit_dialog"),
                ViewportBuilder::default()
                    .with_title(title_str)
                    .with_app_id("argus-lasso")
                    .with_icon(egui::IconData { rgba: crate::icon::RGBA.to_vec(), width: crate::icon::W, height: crate::icon::H })
                    .with_min_inner_size([540.0, 520.0])
                    .with_transparent(true)
                    .with_resizable(true),
                |ctx, _class| {
                    let _opacity_saved = crate::gui::theme::push_viewport_opacity(ctx, opacity);
                    if ctx.input(|i| i.viewport().close_requested()) {
                        close_as = Some(false);
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        egui::Grid::new("rule_form").num_columns(2).show(ui, |ui| {
                            ui.label("Name:");
                            ui.add(egui::TextEdit::singleline(&mut rule.name).desired_width(260.0));
                            ui.end_row();

                            ui.label("Pattern:");
                            ui.add(egui::TextEdit::singleline(&mut rule.pattern).desired_width(260.0));
                            ui.end_row();

                            ui.label("Match type:");
                            egui::ComboBox::from_id_salt("match_type")
                                .selected_text(&rule.match_type)
                                .show_ui(ui, |ui| {
                                    for mt in ["contains", "exact", "regex"] {
                                        ui.selectable_value(&mut rule.match_type, mt.to_string(), mt);
                                    }
                                });
                            ui.end_row();

                            ui.label("Nice:");
                            ui.horizontal(|ui| {
                                ui.checkbox(nice_enabled, "Enable");
                                let nice = rule.nice.get_or_insert(0);
                                ui.add_enabled(*nice_enabled, egui::DragValue::new(nice).range(-20..=19));
                            });
                            ui.end_row();

                            ui.label("I/O Priority:");
                            ui.horizontal(|ui| {
                                ui.checkbox(ionice_enabled, "Enable");
                                let class = rule.ionice_class.get_or_insert(2);
                                ui.add_enabled(*ionice_enabled, egui::DragValue::new(class).range(0..=3));
                                ui.label("class, level:");
                                let level = rule.ionice_level.get_or_insert(4);
                                ui.add_enabled(*ionice_enabled, egui::DragValue::new(level).range(0..=7));
                            });
                            ui.end_row();

                            ui.label("Enabled:");
                            ui.checkbox(&mut rule.enabled, "Rule active");
                            ui.end_row();
                        });

                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.checkbox(affinity_enabled, "CPU Affinity");
                            if *affinity_enabled {
                                ui.label(egui::RichText::new("(select cores below)").color(ui.visuals().weak_text_color()));
                            }
                        });
                        if *affinity_enabled {
                            ui.group(|ui| {
                                ui.set_min_width(ui.available_width());
                                picker.show(ui);
                            });
                        }

                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("OK").clicked() && !rule.pattern.is_empty() {
                                close_as = Some(true);
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            if ui.button("Cancel").clicked() {
                                close_as = Some(false);
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        });
                    });
                    crate::gui::theme::pop_viewport_opacity(ctx, _opacity_saved);
                },
            );
        }

        if let Some(accept) = close_as {
            self.open = false;
            if accept {
                if self.affinity_enabled {
                    let cpulist = self.affinity_picker.cpulist();
                    self.rule.affinity = if cpulist.is_empty() { None } else { Some(cpulist) };
                } else {
                    self.rule.affinity = None;
                }
                if !self.nice_enabled { self.rule.nice = None; }
                if !self.ionice_enabled {
                    self.rule.ionice_class = None;
                    self.rule.ionice_level = None;
                }
                self.rule.refresh_regex();
                self.result = Some(Some(self.rule.clone()));
                return Some(Some(self.rule.clone()));
            } else {
                self.result = Some(None);
                return Some(None);
            }
        }
        None
    }
}

// ── RulePresetsDialog ─────────────────────────────────────────────────────────

pub const RULE_PRESETS: &[(&str, &str, &str, Option<&str>, Option<i32>, Option<i32>, Option<i32>)] = &[
    ("Steam (CCD0)",        "steam",           "exact",    Some("0-7,16-23"),  None,    None, None),
    ("steamwebhelper",      "steamwebhelper",  "exact",    Some("0-7,16-23"),  Some(5), None, None),
    ("Wine / Proton",       "wine",            "contains", Some("0-7,16-23"),  None,    None, None),
    ("Proton",              "proton",          "contains", Some("0-7,16-23"),  None,    None, None),
    ("OBS Studio",          "obs",             "exact",    Some("0-7,16-23"),  Some(-1),None, None),
    ("Discord",             "discord",         "contains", Some("8-15,24-31"),Some(5), None, None),
    ("Firefox",             "firefox",         "contains", Some("8-15,24-31"),None,    None, None),
    ("Chromium / Chrome",   "chrom",           "contains", Some("8-15,24-31"),None,    None, None),
    ("KWin",                "kwin",            "contains", None,               None,    None, None),
    ("Plasma Shell",        "plasmashell",     "exact",    Some("8-15,24-31"),Some(5), None, None),
    ("Compiler (gcc/clang)","gcc",             "contains", Some("0-15,16-31"),None,    Some(2), Some(4)),
    ("Archive / compress",  "7z",              "contains", Some("8-15,24-31"),Some(10),Some(3), None),
];

pub struct RulePresetsDialog {
    pub open: bool,
    pub selected: Option<usize>,
    pub result: Option<Option<Rule>>,
}

impl RulePresetsDialog {
    pub fn new() -> Self {
        Self { open: true, selected: None, result: None }
    }

    pub fn show(&mut self, ctx: &Context, opacity: f32) -> Option<Option<Rule>> {
        if !self.open { return self.result.clone(); }

        let mut close_as: Option<bool> = None;

        {
            let selected = &mut self.selected;

            ctx.show_viewport_immediate(
                ViewportId::from_hash_of("rule_presets_dialog"),
                ViewportBuilder::default()
                    .with_title("Rule Templates")
                    .with_app_id("argus-lasso")
                    .with_icon(egui::IconData { rgba: crate::icon::RGBA.to_vec(), width: crate::icon::W, height: crate::icon::H })
                    .with_min_inner_size([640.0, 360.0])
                    .with_transparent(true)
                    .with_resizable(true),
                |ctx, _class| {
                    let _opacity_saved = crate::gui::theme::push_viewport_opacity(ctx, opacity);
                    if ctx.input(|i| i.viewport().close_requested()) {
                        close_as = Some(false);
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.label("Select a preset to create a pre-filled rule:");
                        ui.add_space(4.0);
                        // Header row
                        let hdr_bg = ui.visuals().widgets.noninteractive.bg_fill;
                        let hdr_col = ui.visuals().strong_text_color();
                        egui::Frame::new().fill(hdr_bg).show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            egui::Grid::new("preset_hdr").num_columns(4).min_col_width(60.0)
                                .spacing([16.0, 2.0]).show(ui, |ui| {
                                for label in ["NAME", "PATTERN", "MATCH", "AFFINITY"] {
                                    ui.label(egui::RichText::new(label).strong().color(hdr_col));
                                }
                                ui.end_row();
                            });
                        });
                        egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                            for (i, (name, pat, match_type, aff, _nice, _ioc, _iol)) in RULE_PRESETS.iter().enumerate() {
                                let is_sel = *selected == Some(i);
                                let bg = if is_sel {
                                    ui.visuals().selection.bg_fill
                                } else if i % 2 == 1 {
                                    ui.visuals().faint_bg_color
                                } else {
                                    ui.visuals().extreme_bg_color
                                };
                                egui::Frame::new().fill(bg).show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    let resp = egui::Grid::new(("preset_row", i))
                                        .num_columns(4)
                                        .min_col_width(60.0)
                                        .spacing([16.0, 2.0])
                                        .show(ui, |ui| {
                                            ui.label(*name);
                                            ui.label(*pat);
                                            ui.label(*match_type);
                                            ui.label(aff.unwrap_or(""));
                                            ui.end_row();
                                        }).response;
                                    if resp.clicked() { *selected = Some(i); }
                                    if resp.double_clicked() {
                                        *selected = Some(i);
                                        close_as = Some(true);
                                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                    }
                                });
                            }
                        });
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.add_enabled(selected.is_some(), egui::Button::new("Use Preset")).clicked() {
                                close_as = Some(true);
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            if ui.button("Cancel").clicked() {
                                close_as = Some(false);
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        });
                    });
                    crate::gui::theme::pop_viewport_opacity(ctx, _opacity_saved);
                },
            );
        }

        if let Some(accept) = close_as {
            self.open = false;
            if accept {
                if let Some(idx) = self.selected {
                    let (name, pat, match_type, aff, nice, ioc, iol) = RULE_PRESETS[idx];
                    let mut rule = Rule::new_empty();
                    rule.name = name.to_string();
                    rule.pattern = pat.to_string();
                    rule.match_type = match_type.to_string();
                    rule.affinity = aff.map(|s| s.to_string());
                    rule.nice = nice;
                    rule.ionice_class = ioc;
                    rule.ionice_level = iol;
                    self.result = Some(Some(rule.clone()));
                    return Some(Some(rule));
                }
            }
            self.result = Some(None);
            return Some(None);
        }
        None
    }
}

// ── SteamGamePickerDialog ─────────────────────────────────────────────────────

pub struct SteamGamePickerDialog {
    pub open: bool,
    pub games: Vec<(String, String)>,  // (appid, name)
    pub filter: String,
    pub selected: Option<usize>,
    pub result: Option<Option<(String, String)>>,
    pub loaded: bool,
}

impl SteamGamePickerDialog {
    pub fn new() -> Self {
        let mut dlg = Self {
            open: true,
            games: Vec::new(),
            filter: String::new(),
            selected: None,
            result: None,
            loaded: false,
        };
        dlg.games = scan_steam_library();
        dlg.loaded = true;
        dlg
    }

    pub fn show(&mut self, ctx: &Context, opacity: f32) -> Option<Option<(String, String)>> {
        if !self.open { return self.result.clone(); }
        let mut accepted = false;
        let mut cancelled = false;
        {
            let filter  = &mut self.filter;
            let games   = &self.games;
            let selected = &mut self.selected;
            ctx.show_viewport_immediate(
                ViewportId::from_hash_of("steam_game_picker"),
                ViewportBuilder::default()
                    .with_title("Pick Steam Game")
                    .with_app_id("argus-lasso")
                    .with_icon(egui::IconData { rgba: crate::icon::RGBA.to_vec(), width: crate::icon::W, height: crate::icon::H })
                    .with_min_inner_size([560.0, 520.0])
                    .with_transparent(true)
                    .with_resizable(true),
                |ctx, _class| {
                    let _opacity_saved = crate::gui::theme::push_viewport_opacity(ctx, opacity);
                    if ctx.input(|i| i.viewport().close_requested()) {
                        cancelled = true;
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Filter:");
                            ui.text_edit_singleline(filter);
                        });

                        let filter_lower = filter.to_lowercase();
                        let filtered: Vec<_> = games.iter().enumerate()
                            .filter(|(_, (id, name))| filter_lower.is_empty()
                                || name.to_lowercase().contains(&filter_lower)
                                || id.contains(&filter_lower))
                            .collect();

                        egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                            for (orig_i, (appid, name)) in &filtered {
                                let sel = *selected == Some(*orig_i);
                                let row = format!("{appid:<10} {name}");
                                let resp = ui.selectable_label(sel, &row);
                                if resp.double_clicked() {
                                    *selected = Some(*orig_i);
                                    accepted = true;
                                } else if resp.clicked() {
                                    *selected = Some(*orig_i);
                                }
                            }
                        });

                        ui.label(format!("{} games found", games.len()));
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("Select").clicked() && selected.is_some() {
                                accepted = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            if ui.button("Cancel").clicked() {
                                cancelled = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        });
                    });
                    crate::gui::theme::pop_viewport_opacity(ctx, _opacity_saved);
                },
            );
        }
        if accepted {
            if let Some(idx) = self.selected {
                if let Some((appid, name)) = self.games.get(idx) {
                    let r = (appid.clone(), name.clone());
                    self.open = false;
                    self.result = Some(Some(r.clone()));
                    return Some(Some(r));
                }
            }
        }
        if cancelled {
            self.open = false;
            self.result = Some(None);
            return Some(None);
        }
        None
    }
}

fn scan_steam_library() -> Vec<(String, String)> {
    use std::path::PathBuf;
    let home = std::env::var("HOME").unwrap_or_default();
    let roots = vec![
        PathBuf::from(&home).join(".steam/steam"),
        PathBuf::from(&home).join(".local/share/Steam"),
    ];
    let mut seen = std::collections::HashSet::new();
    let mut lib_dirs: Vec<PathBuf> = Vec::new();
    for root in roots {
        if let Ok(resolved) = root.canonicalize() {
            if seen.insert(resolved.clone()) {
                lib_dirs.push(resolved.join("steamapps"));
            }
        }
    }
    // Parse libraryfolders.vdf for additional paths
    let extra: Vec<_> = lib_dirs.clone();
    for lib in extra {
        let vdf = lib.join("libraryfolders.vdf");
        if let Ok(text) = std::fs::read_to_string(&vdf) {
            for cap in text.lines() {
                if cap.contains("\"path\"") {
                    if let Some(p) = cap.split('"').nth(3) {
                        let apps = PathBuf::from(p).join("steamapps");
                        if let Ok(r) = apps.canonicalize() {
                            if seen.insert(r.clone()) {
                                lib_dirs.push(r);
                            }
                        }
                    }
                }
            }
        }
    }
    let mut games: std::collections::HashMap<String, String> = Default::default();
    for apps_dir in &lib_dirs {
        if let Ok(entries) = std::fs::read_dir(apps_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let fname = name.to_string_lossy();
                if fname.starts_with("appmanifest_") && fname.ends_with(".acf") {
                    if let Ok(text) = std::fs::read_to_string(entry.path()) {
                        let appid = extract_vdf_field(&text, "appid");
                        let gname = extract_vdf_field(&text, "name");
                        if let (Some(id), Some(n)) = (appid, gname) {
                            games.insert(id, n);
                        }
                    }
                }
            }
        }
    }
    let mut sorted: Vec<_> = games.into_iter().collect();
    sorted.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    sorted
}

fn extract_vdf_field(text: &str, field: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&format!("\"{field}\"")) {
            let parts: Vec<_> = trimmed.splitn(4, '"').collect();
            if parts.len() >= 4 {
                return Some(parts[3].to_string());
            }
        }
    }
    None
}

// ── LutrisGamePickerDialog ────────────────────────────────────────────────────

pub struct LutrisGamePickerDialog {
    pub open: bool,
    pub games: Vec<(String, String)>,  // (slug, display_label)
    pub filter: String,
    pub selected: Option<usize>,
    pub result: Option<Option<(String, String)>>,  // (slug, name)
    pub status: String,
}

impl LutrisGamePickerDialog {
    pub fn new() -> Self {
        let (games, status) = scan_lutris_library();
        Self {
            open: true,
            games,
            filter: String::new(),
            selected: None,
            result: None,
            status,
        }
    }

    pub fn show(&mut self, ctx: &Context, opacity: f32) -> Option<Option<(String, String)>> {
        if !self.open { return self.result.clone(); }
        let mut accepted = false;
        let mut cancelled = false;
        {
            let filter   = &mut self.filter;
            let games    = &self.games;
            let selected = &mut self.selected;
            let status   = &self.status;
            ctx.show_viewport_immediate(
                ViewportId::from_hash_of("lutris_game_picker"),
                ViewportBuilder::default()
                    .with_title("Pick Lutris Game")
                    .with_app_id("argus-lasso")
                    .with_icon(egui::IconData { rgba: crate::icon::RGBA.to_vec(), width: crate::icon::W, height: crate::icon::H })
                    .with_min_inner_size([560.0, 520.0])
                    .with_transparent(true)
                    .with_resizable(true),
                |ctx, _class| {
                    let _opacity_saved = crate::gui::theme::push_viewport_opacity(ctx, opacity);
                    if ctx.input(|i| i.viewport().close_requested()) {
                        cancelled = true;
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Filter:");
                            ui.text_edit_singleline(filter);
                        });

                        let fl = filter.to_lowercase();
                        let filtered: Vec<_> = games.iter().enumerate()
                            .filter(|(_, (_, label))| fl.is_empty() || label.to_lowercase().contains(&fl))
                            .collect();

                        egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                            for (orig_i, (_, label)) in &filtered {
                                let sel = *selected == Some(*orig_i);
                                let resp = ui.selectable_label(sel, label.as_str());
                                if resp.double_clicked() {
                                    *selected = Some(*orig_i);
                                    accepted = true;
                                } else if resp.clicked() {
                                    *selected = Some(*orig_i);
                                }
                            }
                        });

                        ui.label(status.as_str());
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("Select").clicked() && selected.is_some() {
                                accepted = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            if ui.button("Cancel").clicked() {
                                cancelled = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        });
                    });
                    crate::gui::theme::pop_viewport_opacity(ctx, _opacity_saved);
                },
            );
        }
        if accepted {
            if let Some(idx) = self.selected {
                if let Some((slug, label)) = self.games.get(idx) {
                    let name = label.split("  [").next().unwrap_or(label).to_string();
                    let r = (slug.clone(), name);
                    self.open = false;
                    self.result = Some(Some(r.clone()));
                    return Some(Some(r));
                }
            }
        }
        if cancelled {
            self.open = false;
            self.result = Some(None);
            return Some(None);
        }
        None
    }
}

fn scan_lutris_library() -> (Vec<(String, String)>, String) {
    let home = std::env::var("HOME").unwrap_or_default();
    let db = format!("{home}/.local/share/lutris/pga.db");
    if !std::path::Path::new(&db).exists() {
        return (vec![], "Lutris database not found.".into());
    }
    // We don't want to pull in rusqlite; parse the sqlite file via the sqlite3 CLI.
    let output = std::process::Command::new("sqlite3")
        .args([&db, "SELECT name,slug,runner FROM games WHERE installed=1 ORDER BY name COLLATE NOCASE"])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            let games: Vec<(String, String)> = text.lines()
                .filter_map(|line| {
                    let parts: Vec<_> = line.splitn(3, '|').collect();
                    if parts.len() == 3 {
                        let name = parts[0].trim().to_string();
                        let slug = parts[1].trim().to_string();
                        let runner = parts[2].trim().to_string();
                        Some((slug, format!("{name}  [{runner}]")))
                    } else {
                        None
                    }
                })
                .collect();
            let count = games.len();
            (games, format!("{count} installed games found"))
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            (vec![], format!("sqlite3 error: {err}"))
        }
        Err(e) => (vec![], format!("sqlite3 not found: {e}")),
    }
}
