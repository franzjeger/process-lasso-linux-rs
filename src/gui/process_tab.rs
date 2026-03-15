//! Processes tab: CPU history + per-CPU bars + filter + sortable process table.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use egui::RichText;

use crate::gui::cpu_bars::{CpuBarsWidget, CpuHistoryWidget};
use crate::gui::theme::{self, Breeze};
use crate::monitor::{DaemonCmd, ProcInfo};
use crate::rules::RuleEngine;
use crate::utils::{build_core_pairs, cpulist_to_set, cpuset_to_cpulist, get_offline_cpus};

// ── Sort state ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum SortCol {
    Pid,
    Name,
    Cpu,
    Mem,
    Nice,
    Affinity,
    Ionice,
    Status,
}

impl SortCol {
    fn label(&self) -> &'static str {
        match self {
            SortCol::Pid      => "PID",
            SortCol::Name     => "NAME",
            SortCol::Cpu      => "CPU%",
            SortCol::Mem      => "MEM(MB)",
            SortCol::Nice     => "NICE",
            SortCol::Affinity => "AFFINITY",
            SortCol::Ionice   => "I/O",
            SortCol::Status   => "STATUS",
        }
    }
}

// ── Context menu action ───────────────────────────────────────────────────────

#[derive(Debug)]
pub enum TableAction {
    Kill { pid: u32, name: String, force: bool },
    SetAffinity { pid: u32, name: String, current: String },
    SetNice { pid: u32, name: String, current: i32 },
    SetIonice { pid: u32, name: String },
    AddRule { name: String },
    None,
}

// ── Format affinity string with grouped physical+HT pairs ─────────────────────

fn format_affinity_display(
    affinity_str: &str,
    offline: &HashSet<u32>,
    core_pairs: &HashMap<u32, Vec<u32>>,
    hide_parked: bool,
) -> String {
    if !hide_parked || offline.is_empty() {
        return affinity_str.to_string();
    }
    let cpus = match cpulist_to_set(affinity_str) {
        Ok(s) if !s.is_empty() => s,
        _ => return affinity_str.to_string(),
    };
    let visible: HashSet<u32> = cpus.difference(offline).copied().collect();
    if visible.is_empty() {
        return "—".to_string();
    }
    if core_pairs.is_empty() {
        return cpuset_to_cpulist(&visible);
    }
    let mut seen: HashSet<u32> = HashSet::new();
    let mut sorted_visible: Vec<u32> = visible.iter().copied().collect();
    sorted_visible.sort_unstable();
    let mut parts: Vec<String> = Vec::new();
    for cpu in &sorted_visible {
        if seen.contains(cpu) {
            continue;
        }
        seen.insert(*cpu);
        if let Some(siblings) = core_pairs.get(cpu) {
            let vis_sibs: Vec<u32> = siblings.iter()
                .filter(|&&s| visible.contains(&s) && !seen.contains(&s))
                .copied()
                .collect();
            if !vis_sibs.is_empty() {
                for &s in &vis_sibs {
                    seen.insert(s);
                }
                let sib_str = vis_sibs.iter().map(|c| c.to_string()).collect::<Vec<_>>().join("+");
                parts.push(format!("{cpu}+{sib_str}"));
            } else {
                parts.push(cpu.to_string());
            }
        } else {
            parts.push(cpu.to_string());
        }
    }
    parts.join(",")
}

// ── ProcessTab ────────────────────────────────────────────────────────────────

pub struct ProcessTab {
    pub history: CpuHistoryWidget,
    pub bars: CpuBarsWidget,
    pub filter: String,
    pub sort_col: SortCol,
    pub sort_asc: bool,
    // Single-row selection (by PID)
    pub selected_pid: Option<u32>,
    // Gaming mode: hide/group parked CPUs in affinity column
    pub hide_parked_in_proc_view: bool,
    // Cached physical-core → HT-sibling map (read once from sysfs at startup)
    core_pairs: HashMap<u32, Vec<u32>>,
}

impl ProcessTab {
    pub fn new() -> Self {
        Self {
            history: CpuHistoryWidget::new(),
            bars: CpuBarsWidget::new(),
            filter: String::new(),
            sort_col: SortCol::Cpu,
            sort_asc: false,
            selected_pid: None,
            hide_parked_in_proc_view: true,
            core_pairs: build_core_pairs(),
        }
    }

    pub fn update_cpu(&mut self, pcts: Vec<f32>) {
        let avg = if pcts.is_empty() { 0.0 } else { pcts.iter().sum::<f32>() / pcts.len() as f32 };
        self.history.push(avg);
        self.bars.update(pcts);
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &[ProcInfo],
        throttled_pids: &std::collections::HashSet<u32>,
        _cmd_tx: &crossbeam_channel::Sender<DaemonCmd>,
        _rule_engine: &Arc<Mutex<RuleEngine>>,
        gaming_active: bool,
    ) -> TableAction {
        // CPU history chart + per-CPU bars
        self.history.show(ui);
        ui.add_space(2.0);
        self.bars.show(ui);
        ui.add_space(4.0);

        // Keyboard shortcuts — all ctx calls MUST be outside ui.input():
        // ctx.input() holds the ContextImpl WRITE lock; calling ctx.read() or ctx.write()
        // inside it causes write→read or write→write re-entrant deadlock (parking_lot panics
        // after 10s with "Failed to acquire RwLock … Deadlock?").
        let filter_id = egui::Id::new("proc_filter");
        let (f5_pressed, slash_pressed) = ui.input(|i| (
            i.key_pressed(egui::Key::F5),
            i.key_pressed(egui::Key::Slash) && !i.modifiers.any(),
        ));
        if slash_pressed {
            ui.ctx().memory_mut(|m| m.request_focus(filter_id));
        }
        if f5_pressed {
            ui.ctx().request_repaint();
        }

        // Filter row + gaming mode toggle
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.add(egui::TextEdit::singleline(&mut self.filter)
                .id(filter_id)
                .hint_text("name / PID / cmdline  (/ to focus)")
                .desired_width(220.0));
            if !self.filter.is_empty() {
                if ui.small_button("✕").clicked() {
                    self.filter.clear();
                }
            }
            if gaming_active {
                ui.separator();
                ui.checkbox(&mut self.hide_parked_in_proc_view, "Group affinity / hide parked");
            }
        });
        ui.add_space(2.0);

        // Sort + filter (name, PID, or cmdline)
        let mut sorted = snapshot.to_vec();
        let filter_lower = self.filter.to_lowercase();
        if !filter_lower.is_empty() {
            sorted.retain(|p| {
                p.name.to_lowercase().contains(&filter_lower)
                    || p.pid.to_string().contains(&filter_lower)
                    || p.cmdline.to_lowercase().contains(&filter_lower)
            });
        }

        let asc = self.sort_asc;
        // All sorts use PID as a stable tiebreaker so equal rows never flicker.
        match self.sort_col {
            SortCol::Pid      => sorted.sort_by(|a, b| if asc { a.pid.cmp(&b.pid) } else { b.pid.cmp(&a.pid) }),
            SortCol::Name     => sorted.sort_by(|a, b| (if asc { a.name.cmp(&b.name) } else { b.name.cmp(&a.name) }).then(a.pid.cmp(&b.pid))),
            SortCol::Cpu      => sorted.sort_by(|a, b| {
                let ord = if asc { a.cpu_percent.partial_cmp(&b.cpu_percent) } else { b.cpu_percent.partial_cmp(&a.cpu_percent) };
                ord.unwrap_or(std::cmp::Ordering::Equal).then(a.pid.cmp(&b.pid))
            }),
            SortCol::Mem      => sorted.sort_by(|a, b| (if asc { a.mem_rss.cmp(&b.mem_rss) } else { b.mem_rss.cmp(&a.mem_rss) }).then(a.pid.cmp(&b.pid))),
            SortCol::Nice     => sorted.sort_by(|a, b| (if asc { a.nice.cmp(&b.nice) } else { b.nice.cmp(&a.nice) }).then(a.pid.cmp(&b.pid))),
            SortCol::Affinity => sorted.sort_by(|a, b| (if asc { a.affinity.cmp(&b.affinity) } else { b.affinity.cmp(&a.affinity) }).then(a.pid.cmp(&b.pid))),
            SortCol::Ionice   => sorted.sort_by(|a, b| (if asc { a.ionice.cmp(&b.ionice) } else { b.ionice.cmp(&a.ionice) }).then(a.pid.cmp(&b.pid))),
            SortCol::Status   => sorted.sort_by(|a, b| a.pid.cmp(&b.pid)),
        }

        // Offline CPUs for affinity display
        let offline = if gaming_active && self.hide_parked_in_proc_view {
            get_offline_cpus()
        } else {
            HashSet::new()
        };

        let sort_col_cur   = self.sort_col.clone();
        let sort_asc_cur   = self.sort_asc;
        let hide_parked    = self.hide_parked_in_proc_view;
        let core_pairs     = &self.core_pairs;

        let mut new_sort_col  = sort_col_cur.clone();
        let mut new_sort_asc  = sort_asc_cur;
        let mut new_selected  = self.selected_pid;
        let mut action        = TableAction::None;

        // Delete key — kill the currently selected process
        ui.input(|i| {
            if i.key_pressed(egui::Key::Delete) {
                if let Some(sel_pid) = self.selected_pid {
                    if let Some(proc) = sorted.iter().find(|p| p.pid == sel_pid) {
                        action = TableAction::Kill { pid: sel_pid, name: proc.name.clone(), force: false };
                    }
                }
            }
        });

        const COLS: [SortCol; 8] = [
            SortCol::Pid, SortCol::Name, SortCol::Cpu, SortCol::Mem,
            SortCol::Nice, SortCol::Affinity, SortCol::Ionice, SortCol::Status,
        ];
        const ROW_H:    f32 = 24.0;
        const HEADER_H: f32 = 24.0;
        const PAD:      f32 = 4.0;  // left padding inside each cell

        // Fixed columns: 70(PID)+65(CPU%)+80(Mem)+50(Nice)+130(Aff)+65(I/O)+100(Status) = 560
        let fixed_cols_w: f32 = 70.0 + 65.0 + 80.0 + 50.0 + 130.0 + 65.0 + 100.0;
        // Subtract frame border (2 × 1px) so table fills exactly the available width
        let name_col_w = (ui.available_width() - fixed_cols_w - 2.0).max(180.0);
        let col_widths: [f32; 8] = [70.0, name_col_w, 65.0, 80.0, 50.0, 130.0, 65.0, 100.0];
        let total_cols_w: f32 = col_widths.iter().sum();

        // Wrap table in a visible border frame
        let frame_border_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, frame_border_color))
            .inner_margin(egui::Margin::same(0))
            .show(ui, |ui| {
                // ── Sortable header (pinned, outside scroll area) ─────────────────
                let (header_rect, _) = ui.allocate_exact_size(
                    egui::Vec2::new(total_cols_w, HEADER_H),
                    egui::Sense::hover(),
                );
                // Header background
                ui.painter().rect_filled(
                    header_rect,
                    0.0,
                    ui.visuals().widgets.noninteractive.bg_fill,
                );
                {
                    let mut x = header_rect.min.x;
                    for (i, col) in COLS.iter().enumerate() {
                        let cw = col_widths[i];
                        let cell_rect = egui::Rect::from_min_size(
                            egui::Pos2::new(x + PAD, header_rect.min.y),
                            egui::Vec2::new(cw - PAD, HEADER_H),
                        );
                        let is_active = *col == sort_col_cur;
                        let label_str = if is_active {
                            format!("{} {}", col.label(), if sort_asc_cur { "▲" } else { "▼" })
                        } else {
                            col.label().to_string()
                        };
                        let color = if is_active { ui.visuals().text_color() } else { Breeze::HIGHLIGHT };
                        let resp = ui.put(
                            cell_rect,
                            egui::Label::new(RichText::new(label_str).color(color).strong())
                                .sense(egui::Sense::click()),
                        );
                        if resp.clicked() {
                            if *col == sort_col_cur {
                                new_sort_asc = !sort_asc_cur;
                            } else {
                                new_sort_col = col.clone();
                                new_sort_asc = matches!(col, SortCol::Name | SortCol::Affinity);
                            }
                        }
                        x += cw;
                    }
                }
                // Separator line between header and body
                ui.painter().line_segment(
                    [header_rect.left_bottom(), header_rect.right_bottom()],
                    egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                );

                // ── Scrollable body ───────────────────────────────────────────────
                egui::ScrollArea::vertical()
                    .id_salt("process_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (row_idx, proc) in sorted.iter().enumerate() {
                            let pid      = proc.pid;
                            let is_sel   = new_selected == Some(pid);
                            let throttled = throttled_pids.contains(&pid);
                            let cpu      = proc.cpu_percent;
                            let row_col  = theme::row_color(cpu, throttled, ui.visuals().text_color());
                            let aff_str  = format_affinity_display(
                                &proc.affinity, &offline, core_pairs, hide_parked,
                            );
                            let status_str = if throttled { "⏸ Throttled" } else { "" };

                            // Clone fields needed inside context_menu closure
                            let name    = proc.name.clone();
                            let aff     = proc.affinity.clone();
                            let nice    = proc.nice;
                            let cmdline = proc.cmdline.clone();

                            // Allocate the full row — advances the cursor
                            let (row_rect, row_resp) = ui.allocate_exact_size(
                                egui::Vec2::new(total_cols_w, ROW_H),
                                egui::Sense::click(),
                            );

                            // Row background
                            let bg = if is_sel {
                                ui.visuals().selection.bg_fill
                            } else if row_idx % 2 == 1 {
                                ui.visuals().faint_bg_color
                            } else {
                                ui.visuals().extreme_bg_color
                            };
                            ui.painter().rect_filled(row_rect, 0.0, bg);

                            // Paint cell text directly — no widget allocation so nothing
                            // can intercept the row's Sense::click().
                            if ui.is_rect_visible(row_rect) {
                                let font = egui::FontId::proportional(14.0);
                                let painter = ui.painter();
                                let mut x = row_rect.min.x;
                                for (ci, &cw) in col_widths.iter().enumerate() {
                                    let text_pos = egui::pos2(x + PAD, row_rect.center().y);
                                    let text: std::borrow::Cow<str> = match ci {
                                        0 => pid.to_string().into(),
                                        1 => name.as_str().into(),
                                        2 => format!("{:.1}", cpu).into(),
                                        3 => format!("{:.1}", proc.mem_rss as f64 / 1_048_576.0).into(),
                                        4 => nice.to_string().into(),
                                        5 => aff_str.as_str().into(),
                                        6 => proc.ionice.as_str().into(),
                                        7 => status_str.into(),
                                        _ => "".into(),
                                    };
                                    painter.text(
                                        text_pos,
                                        egui::Align2::LEFT_CENTER,
                                        text.as_ref(),
                                        font.clone(),
                                        row_col,
                                    );
                                    x += cw;
                                }

                                // Tooltip for name column: show cmdline when hovering that cell
                                if row_resp.hovered() {
                                    let name_cell_rect = egui::Rect::from_min_size(
                                        egui::pos2(row_rect.min.x + col_widths[0], row_rect.min.y),
                                        egui::vec2(col_widths[1], ROW_H),
                                    );
                                    if ui.ctx().pointer_hover_pos()
                                        .map_or(false, |p| name_cell_rect.contains(p))
                                    {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::Help);
                                        #[allow(deprecated)]
                                        egui::show_tooltip_at_pointer(
                                            ui.ctx(),
                                            ui.layer_id(),
                                            egui::Id::new(("proc_cmdline", pid)),
                                            |ui| { ui.label(cmdline.as_str()); },
                                        );
                                    }
                                }
                            }

                            // Click → select row
                            if row_resp.clicked() {
                                new_selected = Some(pid);
                            }

                            // Right-click context menu on the entire row
                            row_resp.context_menu(|ui| {
                                if ui.button(format!("Kill {} ({})", name, pid)).clicked() {
                                    action = TableAction::Kill { pid, name: name.clone(), force: false };
                                    ui.close();
                                }
                                if ui.button(format!("Force Kill {} ({})", name, pid)).clicked() {
                                    action = TableAction::Kill { pid, name: name.clone(), force: true };
                                    ui.close();
                                }
                                ui.separator();
                                if ui.button(format!("Set Affinity for {}", name)).clicked() {
                                    action = TableAction::SetAffinity {
                                        pid,
                                        name: name.clone(),
                                        current: aff.clone(),
                                    };
                                    ui.close();
                                }
                                if ui.button(format!("Set Priority (nice) for {}", name)).clicked() {
                                    action = TableAction::SetNice {
                                        pid,
                                        name: name.clone(),
                                        current: nice,
                                    };
                                    ui.close();
                                }
                                if ui.button(format!("Set I/O Priority for {}", name)).clicked() {
                                    action = TableAction::SetIonice { pid, name: name.clone() };
                                    ui.close();
                                }
                                ui.separator();
                                if ui.button(format!("Add Rule for '{}'", name)).clicked() {
                                    action = TableAction::AddRule { name: name.clone() };
                                    ui.close();
                                }
                            });
                        }
                    }); // end ScrollArea
            }); // end Frame border

        self.sort_col    = new_sort_col;
        self.sort_asc    = new_sort_asc;
        self.selected_pid = new_selected;
        action
    }
}
