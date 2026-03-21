//! Processes tab: CPU history + per-CPU bars + filter + sortable process table.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use egui::RichText;

use crate::gui::cpu_bars::{CpuBarsWidget, CpuHistoryWidget};
use crate::gui::theme::{self, Breeze};
use crate::monitor::{DaemonCmd, ProcInfo}; // ProcInfo used by RowItem
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
            SortCol::Ionice   => "I/O PRI",
            SortCol::Status   => "STATUS",
        }
    }
}

// ── Formatting helpers ────────────────────────────────────────────────────────

/// Convert raw "class/level" ionice string to human-readable form.
fn fmt_ionice(s: &str) -> String {
    if s.is_empty() { return "—".into(); }
    let mut parts = s.splitn(2, '/');
    let class: u32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let level: u32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    match class {
        0 => "—".into(),
        1 => format!("RT-{level}"),
        2 => format!("BE-{level}"),
        3 => "Idle".into(),
        _ => s.into(),
    }
}

/// Format bytes/s compactly: "1.2 MB/s", "456 KB/s", "—"
fn fmt_bps(bytes: u64) -> String {
    if bytes == 0 { return "—".into(); }
    if bytes >= 1_048_576 {
        format!("{:.1} MB/s", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.0} KB/s", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B/s")
    }
}

// ── Context menu action ───────────────────────────────────────────────────────

#[derive(Debug)]
pub enum TableAction {
    Kill { pid: u32, name: String, force: bool },
    Suspend { pid: u32, name: String },
    Resume { pid: u32, name: String },
    SetAffinity { pid: u32, name: String, current: String },
    SetNice { pid: u32, name: String, current: i32 },
    SetIonice { pid: u32, name: String },
    AddRule { name: String },
    None,
}

// ── Pending kill (undo support) ───────────────────────────────────────────────

pub struct PendingKill {
    pub pid: u32,
    pub name: String,
    pub force: bool,
    pub deadline: std::time::Instant,
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
    // Show processes as parent/child tree instead of flat list
    pub tree_view: bool,
    // Cached physical-core → HT-sibling map (read once from sysfs at startup)
    core_pairs: HashMap<u32, Vec<u32>>,
    // User-adjustable column widths: [PID, Name, CPU%, Mem, Nice, Aff, I/O, Status]
    // Name column auto-fills; user can drag handles to resize others.
    pub col_widths: Vec<f32>,
    pub cols_initialized: bool,
    // Last available width — used to detect window resize for auto-scaling
    last_avail_w: f32,
    // Pending kill awaiting undo
    #[allow(dead_code)]
    pub pending_kill: Option<PendingKill>,
    // Set to true when col_widths change so app.rs can persist them
    pub cols_dirty: bool,
}

impl ProcessTab {
    pub fn new(cfg_col_widths: &[f32]) -> Self {
        let col_widths = if cfg_col_widths.len() == 8 {
            cfg_col_widths.to_vec()
        } else {
            vec![60.0, 0.0, 90.0, 75.0, 45.0, 110.0, 58.0, 85.0]
        };
        Self {
            history: CpuHistoryWidget::new(),
            bars: CpuBarsWidget::new(),
            filter: String::new(),
            sort_col: SortCol::Cpu,
            sort_asc: false,
            selected_pid: None,
            hide_parked_in_proc_view: true,
            tree_view: false,
            core_pairs: build_core_pairs(),
            col_widths,
            cols_initialized: false,
            last_avail_w: 0.0,
            pending_kill: None,
            cols_dirty: false,
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
        suspended_pids: &std::collections::HashSet<u32>,
        _cmd_tx: &crossbeam_channel::Sender<DaemonCmd>,
        _rule_engine: &Arc<Mutex<RuleEngine>>,
        gaming_active: bool,
        proc_cpu_history: &std::collections::HashMap<u32, std::collections::VecDeque<f32>>,
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

        // Filter row + view toggles
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
            ui.separator();
            ui.checkbox(&mut self.tree_view, "Tree view");
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
        const PAD:      f32 = 4.0;

        // Auto-fill Name column (index 1) from available width minus fixed columns.
        let avail_w = ui.available_width() - 4.0;
        if !self.cols_initialized {
            let fixed: f32 = self.col_widths.iter().enumerate()
                .filter(|(i, _)| *i != 1)
                .map(|(_, &w)| w)
                .sum();
            self.col_widths[1] = (avail_w - fixed).max(150.0);
            self.cols_initialized = true;
            self.last_avail_w = avail_w;
        } else {
            // Auto-scale fixed columns proportionally when window width changes significantly
            if (avail_w - self.last_avail_w).abs() > 4.0 {
                let ratio = avail_w / self.last_avail_w.max(1.0);
                for (i, w) in self.col_widths.iter_mut().enumerate() {
                    if i != 1 {
                        *w = (*w * ratio).clamp(20.0, 300.0);
                    }
                }
            }
            self.last_avail_w = avail_w;
            // Recalculate name column each frame to fill remaining space.
            let fixed: f32 = self.col_widths.iter().enumerate()
                .filter(|(i, _)| *i != 1)
                .map(|(_, &w)| w)
                .sum();
            self.col_widths[1] = (avail_w - fixed).max(150.0);
        }
        let col_widths = self.col_widths.clone();
        let total_cols_w: f32 = col_widths.iter().sum();

        // Wrap table in a visible border frame
        let frame_border_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let mut col_width_deltas = [0.0f32; 8];
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, frame_border_color))
            .inner_margin(egui::Margin::same(1))
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
                        let is_active = *col == sort_col_cur && !self.tree_view;
                        let label_str = if is_active {
                            format!("{} {}", col.label(), if sort_asc_cur { "↑" } else { "↓" })
                        } else {
                            col.label().to_string()
                        };
                        let color = if is_active { ui.visuals().text_color() } else { Breeze::HIGHLIGHT };
                        let resp = ui.put(
                            cell_rect,
                            egui::Label::new(RichText::new(label_str).color(color).strong())
                                .sense(egui::Sense::click()),
                        );
                        if resp.clicked() && !self.tree_view {
                            if *col == sort_col_cur {
                                new_sort_asc = !sort_asc_cur;
                            } else {
                                new_sort_col = col.clone();
                                new_sort_asc = matches!(col, SortCol::Name | SortCol::Affinity);
                            }
                        }
                        x += cw;
                    }
                    // Drag-to-resize handles — one between each column pair
                    x = header_rect.min.x;
                    for i in 0..7usize {
                        x += col_widths[i];
                        let handle_rect = egui::Rect::from_min_size(
                            egui::pos2(x - 3.0, header_rect.min.y),
                            egui::vec2(6.0, HEADER_H),
                        );
                        let resp = ui.interact(
                            handle_rect,
                            egui::Id::new(("col_resize", i)),
                            egui::Sense::drag(),
                        );
                        let sep_color = if resp.hovered() || resp.dragged() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
                            Breeze::HIGHLIGHT
                        } else {
                            ui.visuals().widgets.noninteractive.bg_stroke.color
                        };
                        ui.painter().line_segment(
                            [egui::pos2(x, header_rect.min.y), egui::pos2(x, header_rect.max.y)],
                            egui::Stroke::new(1.0, sep_color),
                        );
                        if resp.dragged() {
                            col_width_deltas[i] += resp.drag_delta().x;
                        }
                    }
                }
                // Separator line between header and body
                ui.painter().line_segment(
                    [header_rect.left_bottom(), header_rect.right_bottom()],
                    egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                );

                // ── Scrollable body ───────────────────────────────────────────────
                // Build tree-ordered row list when tree_view is active.
                struct RowItem<'a> { proc: &'a ProcInfo, depth: usize }
                let row_items: Vec<RowItem> = if self.tree_view {
                    let pid_set: HashSet<u32> = sorted.iter().map(|p| p.pid).collect();
                    let mut children: HashMap<u32, Vec<usize>> = HashMap::new();
                    let mut roots: Vec<usize> = Vec::new();
                    for (i, p) in sorted.iter().enumerate() {
                        if p.ppid == 0 || !pid_set.contains(&p.ppid) {
                            roots.push(i);
                        } else {
                            children.entry(p.ppid).or_default().push(i);
                        }
                    }
                    // Sort children by name for stable display
                    for v in children.values_mut() {
                        v.sort_by_key(|&i| &sorted[i].name);
                    }
                    roots.sort_by_key(|&i| &sorted[i].name);
                    let mut result = Vec::new();
                    let mut stack: Vec<(usize, usize)> = roots.iter().map(|&i| (i, 0)).collect();
                    stack.reverse();
                    while let Some((idx, depth)) = stack.pop() {
                        result.push(RowItem { proc: &sorted[idx], depth });
                        if let Some(ch) = children.get(&sorted[idx].pid) {
                            let mut ch_sorted = ch.clone();
                            ch_sorted.sort_by_key(|&i| &sorted[i].name);
                            for ci in ch_sorted.into_iter().rev() {
                                stack.push((ci, depth + 1));
                            }
                        }
                    }
                    result
                } else {
                    sorted.iter().map(|p| RowItem { proc: p, depth: 0 }).collect()
                };

                egui::ScrollArea::vertical()
                    .id_salt("process_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (row_idx, item) in row_items.iter().enumerate() {
                            let proc     = item.proc;
                            let indent   = item.depth as f32 * 14.0;
                            let pid      = proc.pid;
                            let is_sel   = new_selected == Some(pid);
                            let throttled = throttled_pids.contains(&pid);
                            let cpu      = proc.cpu_percent;
                            let row_col  = theme::row_color(cpu, throttled, ui.visuals().text_color());
                            let aff_full = format_affinity_display(
                                &proc.affinity, &offline, core_pairs, hide_parked,
                            );
                            // Truncate affinity if very long, show full string in tooltip
                            const AFF_MAX: usize = 14;
                            let aff_display = if aff_full.len() > AFF_MAX {
                                format!("{}…", &aff_full[..AFF_MAX.saturating_sub(1)])
                            } else {
                                aff_full.clone()
                            };
                            let ionice_str = fmt_ionice(&proc.ionice);
                            let is_suspended = suspended_pids.contains(&pid);
                            let status_str = if is_suspended {
                                "⏸ Suspended"
                            } else if throttled {
                                "🔻 Throttled"
                            } else {
                                ""
                            };

                            // Clone fields needed inside closures
                            let name    = proc.name.clone();
                            let aff     = proc.affinity.clone();
                            let nice    = proc.nice;
                            let cmdline = proc.cmdline.clone();
                            let drb     = proc.disk_read_bps;
                            let dwb     = proc.disk_write_bps;

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

                            // Paint cell text directly
                            if ui.is_rect_visible(row_rect) {
                                let font = egui::FontId::proportional(13.5);
                                let painter = ui.painter();
                                let mut x = row_rect.min.x;
                                for (ci, &cw) in col_widths.iter().enumerate() {
                                    let x_off = if ci == 1 { indent } else { 0.0 };
                                    // For CPU% column (ci==2): draw sparkline on left, shift text right
                                    let text_x_off = if ci == 2 { cw * 0.45 } else { 0.0 };
                                    let text_pos = egui::pos2(x + PAD + x_off + text_x_off, row_rect.center().y);
                                    let text: std::borrow::Cow<str> = match ci {
                                        0 => pid.to_string().into(),
                                        1 => name.as_str().into(),
                                        2 => format!("{:.1}", cpu).into(),
                                        3 => format!("{:.1}", proc.mem_rss as f64 / 1_048_576.0).into(),
                                        4 => nice.to_string().into(),
                                        5 => aff_display.as_str().into(),
                                        6 => ionice_str.as_str().into(),
                                        7 => status_str.into(),
                                        _ => "".into(),
                                    };
                                    // Draw mini sparkline in left portion of CPU% cell
                                    if ci == 2 {
                                        if let Some(hist) = proc_cpu_history.get(&pid) {
                                            if hist.len() >= 2 {
                                                let spark_w = cw * 0.42;
                                                let spark_rect = egui::Rect::from_min_size(
                                                    egui::pos2(x + 1.0, row_rect.min.y + 2.0),
                                                    egui::vec2(spark_w, ROW_H - 4.0),
                                                );
                                                let lo = hist.iter().cloned().fold(f32::INFINITY, f32::min);
                                                let hi = hist.iter().cloned().fold(f32::NEG_INFINITY, f32::max).max(lo + 0.1);
                                                let pts: Vec<egui::Pos2> = hist.iter().enumerate()
                                                    .map(|(i, &v)| {
                                                        let px = spark_rect.left() + i as f32 / (hist.len() - 1).max(1) as f32 * spark_rect.width();
                                                        let py = spark_rect.bottom() - (v - lo) / (hi - lo) * spark_rect.height();
                                                        egui::pos2(px, py)
                                                    })
                                                    .collect();
                                                let spark_col = if cpu > 80.0 {
                                                    egui::Color32::from_rgb(240, 80, 60)
                                                } else if cpu > 50.0 {
                                                    egui::Color32::from_rgb(240, 180, 60)
                                                } else {
                                                    egui::Color32::from_rgb(80, 180, 100)
                                                };
                                                for pair in pts.windows(2) {
                                                    painter.line_segment([pair[0], pair[1]], egui::Stroke::new(1.0, spark_col));
                                                }
                                            }
                                        }
                                    }
                                    painter.text(
                                        text_pos,
                                        egui::Align2::LEFT_CENTER,
                                        text.as_ref(),
                                        font.clone(),
                                        row_col,
                                    );
                                    x += cw;
                                }

                                // Tooltip: hover name → cmdline + disk I/O + full affinity
                                if row_resp.hovered() {
                                    let ptr = ui.ctx().pointer_hover_pos();
                                    // Name cell
                                    let name_rect = egui::Rect::from_min_size(
                                        egui::pos2(row_rect.min.x + col_widths[0], row_rect.min.y),
                                        egui::vec2(col_widths[1], ROW_H),
                                    );
                                    // Affinity cell
                                    let aff_x = col_widths[..5].iter().sum::<f32>() + row_rect.min.x;
                                    let aff_rect = egui::Rect::from_min_size(
                                        egui::pos2(aff_x, row_rect.min.y),
                                        egui::vec2(col_widths[5], ROW_H),
                                    );
                                    if ptr.map_or(false, |p| name_rect.contains(p)) {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
                                        #[allow(deprecated)]
                                        egui::show_tooltip_at_pointer(
                                            ui.ctx(), ui.layer_id(),
                                            egui::Id::new(("proc_tip", pid)),
                                            |ui| {
                                                ui.label(egui::RichText::new(&name).strong());
                                                if !cmdline.is_empty() {
                                                    ui.label(egui::RichText::new(cmdline.as_str())
                                                        .size(11.5)
                                                        .color(ui.visuals().weak_text_color()));
                                                }
                                                ui.separator();
                                                ui.label(format!("PID: {}   PPID: {}", pid, proc.ppid));
                                                ui.label(format!("Disk R: {}   W: {}", fmt_bps(drb), fmt_bps(dwb)));
                                            },
                                        );
                                    } else if ptr.map_or(false, |p| aff_rect.contains(p))
                                        && aff_full.len() > AFF_MAX
                                    {
                                        #[allow(deprecated)]
                                        egui::show_tooltip_at_pointer(
                                            ui.ctx(), ui.layer_id(),
                                            egui::Id::new(("aff_tip", pid)),
                                            |ui| { ui.label(&aff_full); },
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
                                if is_suspended {
                                    if ui.button(format!("Resume {} ({})", name, pid)).clicked() {
                                        action = TableAction::Resume { pid, name: name.clone() };
                                        ui.close();
                                    }
                                } else if ui.button(format!("Suspend {} ({})", name, pid)).clicked() {
                                    action = TableAction::Suspend { pid, name: name.clone() };
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

        // Apply column resize deltas (index 1 = name auto-fills, skip it)
        self.cols_dirty = false;
        for (i, &delta) in col_width_deltas.iter().enumerate() {
            if delta != 0.0 && i != 1 {
                self.col_widths[i] = (self.col_widths[i] + delta).max(30.0);
                self.cols_dirty = true;
            }
        }

        self.sort_col    = new_sort_col;
        self.sort_asc    = new_sort_asc;
        self.selected_pid = new_selected;
        action
    }
}
