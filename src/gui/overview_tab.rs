//! Overview / Dashboard tab: live CPU graph, RAM, load average, top-5 processes.

use egui::{Color32, RichText, Vec2};
use std::collections::VecDeque;

use crate::monitor::ProcInfo;

pub struct OverviewTab;

impl OverviewTab {
    pub fn new() -> Self { Self }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        cpu_history: &VecDeque<f32>,
        cpu_avg: f32,
        snapshot: &[ProcInfo],
    ) {
        let border_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let spacing = 8.0;

        // ── Row 1: CPU graph + RAM + Load ────────────────────────────────────
        ui.horizontal(|ui| {
            // CPU History Graph
            let graph_w = (ui.available_width() * 0.55).max(200.0);
            egui::Frame::new()
                .stroke(egui::Stroke::new(1.0, border_color))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_min_width(graph_w - 16.0);
                    ui.label(RichText::new("CPU Usage History").strong());
                    ui.add_space(4.0);

                    let graph_h = 100.0;
                    let (rect, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), graph_h),
                        egui::Sense::hover(),
                    );
                    let painter = ui.painter();
                    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);

                    if cpu_history.len() >= 2 {
                        let n = cpu_history.len();
                        let pts: Vec<egui::Pos2> = cpu_history.iter().enumerate()
                            .map(|(i, &v)| {
                                let x = rect.left() + i as f32 / (n - 1) as f32 * rect.width();
                                let y = rect.bottom() - (v / 100.0) * rect.height();
                                egui::pos2(x, y)
                            })
                            .collect();

                        // Fill under curve
                        let mut fill_pts = pts.clone();
                        fill_pts.push(egui::pos2(rect.right(), rect.bottom()));
                        fill_pts.push(egui::pos2(rect.left(), rect.bottom()));
                        let fill_color = if cpu_avg > 80.0 {
                            Color32::from_rgba_unmultiplied(220, 60, 60, 60)
                        } else if cpu_avg > 50.0 {
                            Color32::from_rgba_unmultiplied(220, 160, 40, 60)
                        } else {
                            Color32::from_rgba_unmultiplied(60, 160, 80, 60)
                        };
                        painter.add(egui::epaint::PathShape::convex_polygon(fill_pts, fill_color, egui::Stroke::NONE));

                        // Line
                        let line_color = if cpu_avg > 80.0 {
                            Color32::from_rgb(220, 80, 60)
                        } else if cpu_avg > 50.0 {
                            Color32::from_rgb(220, 180, 60)
                        } else {
                            Color32::from_rgb(80, 180, 100)
                        };
                        for pair in pts.windows(2) {
                            painter.line_segment([pair[0], pair[1]], egui::Stroke::new(1.5, line_color));
                        }
                    }

                    // Current value label
                    painter.text(
                        rect.right_top() + Vec2::new(-4.0, 4.0),
                        egui::Align2::RIGHT_TOP,
                        format!("{cpu_avg:.0}%"),
                        egui::FontId::proportional(12.0),
                        ui.visuals().strong_text_color(),
                    );
                });

            ui.add_space(spacing);

            // RAM + Load in a vertical stack
            ui.vertical(|ui| {
                // RAM
                egui::Frame::new()
                    .stroke(egui::Stroke::new(1.0, border_color))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_min_width(180.0);
                        ui.label(RichText::new("Memory").strong());
                        ui.add_space(4.0);
                        if let Some((used, total)) = read_ram_mb() {
                            let pct = used as f32 / total as f32;
                            ui.label(format!("{used} MB / {total} MB  ({:.0}%)", pct * 100.0));
                            let bar_h = 16.0;
                            let (bar_rect, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), bar_h),
                                egui::Sense::hover(),
                            );
                            ui.painter().rect_filled(bar_rect, 3.0, ui.visuals().extreme_bg_color);
                            let fill_w = (bar_rect.width() * pct).max(0.0);
                            let fill = egui::Rect::from_min_size(bar_rect.min, Vec2::new(fill_w, bar_h));
                            let ram_col = if pct > 0.85 {
                                Color32::from_rgb(220, 80, 60)
                            } else if pct > 0.65 {
                                Color32::from_rgb(220, 180, 60)
                            } else {
                                Color32::from_rgb(80, 180, 100)
                            };
                            ui.painter().rect_filled(fill, 3.0, ram_col);
                        } else {
                            ui.label("—");
                        }
                    });

                ui.add_space(spacing);

                // Load Average
                egui::Frame::new()
                    .stroke(egui::Stroke::new(1.0, border_color))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_min_width(180.0);
                        ui.label(RichText::new("Load Average").strong());
                        ui.add_space(4.0);
                        if let Some((l1, l5, l15)) = read_load_avg() {
                            egui::Grid::new("load_grid").num_columns(2).spacing([8.0, 2.0]).show(ui, |ui| {
                                ui.label("1 min:"); ui.label(format!("{l1:.2}")); ui.end_row();
                                ui.label("5 min:"); ui.label(format!("{l5:.2}")); ui.end_row();
                                ui.label("15 min:"); ui.label(format!("{l15:.2}")); ui.end_row();
                            });
                        } else {
                            ui.label("—");
                        }
                    });
            });
        });

        ui.add_space(spacing);

        // ── Top Processes by CPU ─────────────────────────────────────────────
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, border_color))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(RichText::new("Top Processes (CPU%)").strong());
                ui.add_space(4.0);

                let mut top: Vec<&ProcInfo> = snapshot.iter().collect();
                top.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap_or(std::cmp::Ordering::Equal));
                top.truncate(10);

                if top.is_empty() {
                    ui.label(RichText::new("No processes").weak());
                    return;
                }

                let avail_w = ui.available_width();
                let row_h = 20.0;
                let bar_col_w = avail_w * 0.35;
                let name_w = avail_w * 0.30;
                let pid_w = 60.0;
                let cpu_w = 60.0;
                let mem_w = 80.0;

                // Header
                let hdr_bg = ui.visuals().widgets.noninteractive.bg_fill;
                let (hr, _) = ui.allocate_exact_size(Vec2::new(avail_w, row_h), egui::Sense::hover());
                ui.painter().rect_filled(hr, 0.0, hdr_bg);
                let hdr_font = egui::FontId::proportional(12.5);
                let hdr_col = ui.visuals().strong_text_color();
                let mut hx = hr.min.x + 4.0;
                for (label, w) in [("PID", pid_w), ("NAME", name_w), ("CPU%", cpu_w), ("MEM(MB)", mem_w), ("CPU BAR", bar_col_w)] {
                    ui.painter().text(egui::pos2(hx, hr.center().y), egui::Align2::LEFT_CENTER, label, hdr_font.clone(), hdr_col);
                    hx += w;
                }

                for (i, proc) in top.iter().enumerate() {
                    let row_bg = if i % 2 == 1 { ui.visuals().faint_bg_color } else { ui.visuals().extreme_bg_color };
                    let (rr, _) = ui.allocate_exact_size(Vec2::new(avail_w, row_h), egui::Sense::hover());
                    ui.painter().rect_filled(rr, 0.0, row_bg);

                    let text_col = ui.visuals().text_color();
                    let font = egui::FontId::proportional(13.0);
                    let mut rx = rr.min.x + 4.0;
                    let cpu_pct = proc.cpu_percent;
                    let mem_mb = proc.mem_rss as f64 / 1_048_576.0;

                    // PID
                    ui.painter().text(egui::pos2(rx, rr.center().y), egui::Align2::LEFT_CENTER,
                        proc.pid.to_string(), font.clone(), text_col);
                    rx += pid_w;
                    // Name
                    let name_display = if proc.name.len() > 22 { format!("{}…", &proc.name[..21]) } else { proc.name.clone() };
                    ui.painter().text(egui::pos2(rx, rr.center().y), egui::Align2::LEFT_CENTER,
                        name_display, font.clone(), text_col);
                    rx += name_w;
                    // CPU%
                    let cpu_col = if cpu_pct > 80.0 { Color32::from_rgb(240, 80, 60) }
                        else if cpu_pct > 40.0 { Color32::from_rgb(240, 180, 60) }
                        else { text_col };
                    ui.painter().text(egui::pos2(rx, rr.center().y), egui::Align2::LEFT_CENTER,
                        format!("{cpu_pct:.1}"), font.clone(), cpu_col);
                    rx += cpu_w;
                    // Mem
                    ui.painter().text(egui::pos2(rx, rr.center().y), egui::Align2::LEFT_CENTER,
                        format!("{mem_mb:.1}"), font.clone(), text_col);
                    rx += mem_w;
                    // CPU bar
                    let bar_margin = 4.0;
                    let bar_rect = egui::Rect::from_min_size(
                        egui::pos2(rx, rr.min.y + bar_margin),
                        Vec2::new(bar_col_w - bar_margin * 2.0, row_h - bar_margin * 2.0),
                    );
                    ui.painter().rect_filled(bar_rect, 2.0, ui.visuals().extreme_bg_color);
                    let fill_w = (bar_rect.width() * (cpu_pct / 100.0).clamp(0.0, 1.0)).max(0.0);
                    let fill = egui::Rect::from_min_size(bar_rect.min, Vec2::new(fill_w, bar_rect.height()));
                    let bar_col = if cpu_pct > 80.0 { Color32::from_rgb(220, 80, 60) }
                        else if cpu_pct > 40.0 { Color32::from_rgb(220, 180, 60) }
                        else { Color32::from_rgb(80, 180, 100) };
                    ui.painter().rect_filled(fill, 2.0, bar_col);
                }
            });
    }
}

fn read_ram_mb() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut total = 0u64;
    let mut available = 0u64;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("MemTotal:") {
            total = v.split_whitespace().next()?.parse().ok()?;
        } else if let Some(v) = line.strip_prefix("MemAvailable:") {
            available = v.split_whitespace().next()?.parse().ok()?;
        }
    }
    if total == 0 { return None; }
    let used_mb = (total - available) / 1024;
    let total_mb = total / 1024;
    Some((used_mb, total_mb))
}

fn read_load_avg() -> Option<(f32, f32, f32)> {
    let text = std::fs::read_to_string("/proc/loadavg").ok()?;
    let mut parts = text.split_whitespace();
    let l1: f32 = parts.next()?.parse().ok()?;
    let l5: f32 = parts.next()?.parse().ok()?;
    let l15: f32 = parts.next()?.parse().ok()?;
    Some((l1, l5, l15))
}
