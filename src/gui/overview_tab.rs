//! Overview / Dashboard tab: live CPU graph, RAM, load average, top-5 processes.

use egui::{Color32, RichText, Vec2};
use std::collections::VecDeque;

use crate::monitor::ProcInfo;

pub struct OverviewTab;

impl OverviewTab {
    pub fn new() -> Self {
        Self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        cpu_history: &VecDeque<f32>,
        cpu_avg: f32,
        snapshot: &[ProcInfo],
        disk_io_history: &VecDeque<(f32, f32)>,
        net_io_history: &VecDeque<(f32, f32)>,
        cpu_temp: Option<f32>,
        throttled_count: usize,
    ) {
        use crate::gui::theme::{self as th, tokens};
        let border_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let spacing = tokens::SPACE_M;
        let s = th::sem(ui);

        // ── KPI row: the four numbers people open this tab for ───────────────
        ui.horizontal(|ui| {
            let card_w = (ui.available_width() - spacing * 3.0) / 4.0 - 26.0;

            // CPU
            let cpu_detail = match cpu_temp {
                Some(t) => format!("{t:.0} °C · {} cores", crate::utils::get_cpu_count()),
                None => format!("{} cores", crate::utils::get_cpu_count()),
            };
            th::kpi_card(
                ui,
                card_w,
                "CPU",
                &format!("{cpu_avg:.0}%"),
                &cpu_detail,
                th::load_color(ui, cpu_avg),
            );
            ui.add_space(spacing);

            // Memory
            let (mem_value, mem_detail, mem_pct) = match read_ram_mb() {
                Some((used, total)) => {
                    let pct = used as f32 / total as f32 * 100.0;
                    (
                        format!("{pct:.0}%"),
                        format!(
                            "{:.1} of {:.1} GB",
                            used as f32 / 1024.0,
                            total as f32 / 1024.0
                        ),
                        pct,
                    )
                }
                None => ("—".to_string(), String::new(), 0.0),
            };
            th::kpi_card(
                ui,
                card_w,
                "Memory",
                &mem_value,
                &mem_detail,
                th::load_color(ui, mem_pct),
            );
            ui.add_space(spacing);

            // Load average — scale against core count for the stripe colour
            let (load_value, load_detail, load_pct) = match read_load_avg() {
                Some((l1, l5, l15)) => {
                    let cores = crate::utils::get_cpu_count().max(1) as f32;
                    (
                        format!("{l1:.2}"),
                        format!("{l5:.2} / {l15:.2} (5 / 15 min)"),
                        (l1 / cores * 100.0).min(100.0),
                    )
                }
                None => ("—".to_string(), String::new(), 0.0),
            };
            th::kpi_card(
                ui,
                card_w,
                "Load",
                &load_value,
                &load_detail,
                th::load_color(ui, load_pct),
            );
            ui.add_space(spacing);

            // ProBalance
            th::kpi_card(
                ui,
                card_w,
                "ProBalance",
                &throttled_count.to_string(),
                "throttled processes",
                if throttled_count > 0 { s.warning } else { s.ok },
            );
        });

        ui.add_space(spacing);

        // ── CPU history, full width ──────────────────────────────────────────
        {
            egui::Frame::new()
                .stroke(egui::Stroke::new(1.0_f32, border_color))
                .inner_margin(egui::Margin::same(8))
                .corner_radius(egui::CornerRadius::same(4))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(
                        RichText::new("CPU history · 120 s")
                            .strong()
                            .size(tokens::FONT_HEADING),
                    );
                    ui.add_space(4.0);

                    let graph_h = 90.0;
                    let (rect, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), graph_h),
                        egui::Sense::hover(),
                    );
                    let painter = ui.painter();
                    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);

                    // Gridlines at 25 / 50 / 75 % plus 0/100 axis labels
                    let grid_col = th::tint(ui.visuals().weak_text_color(), 60);
                    for frac in [0.25_f32, 0.5, 0.75] {
                        let y = rect.bottom() - frac * rect.height();
                        painter.line_segment(
                            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                            egui::Stroke::new(1.0_f32, grid_col),
                        );
                    }
                    painter.text(
                        rect.left_top() + Vec2::new(4.0, 2.0),
                        egui::Align2::LEFT_TOP,
                        "100%",
                        egui::FontId::proportional(tokens::FONT_SMALL),
                        ui.visuals().weak_text_color(),
                    );
                    painter.text(
                        rect.left_bottom() + Vec2::new(4.0, -2.0),
                        egui::Align2::LEFT_BOTTOM,
                        "0%",
                        egui::FontId::proportional(tokens::FONT_SMALL),
                        ui.visuals().weak_text_color(),
                    );

                    if cpu_history.len() >= 2 {
                        let n = cpu_history.len();
                        let pts: Vec<egui::Pos2> = cpu_history
                            .iter()
                            .enumerate()
                            .map(|(i, &v)| {
                                let x = rect.left() + i as f32 / (n - 1) as f32 * rect.width();
                                let y = rect.bottom() - (v / 100.0) * rect.height();
                                egui::pos2(x, y)
                            })
                            .collect();

                        // Fill under curve — one trapezoid per segment; the
                        // curve is not convex, so a single convex_polygon
                        // renders with self-overlap artifacts.
                        let fill_color = th::tint(th::load_color(ui, cpu_avg), 60);
                        for pair in pts.windows(2) {
                            painter.add(egui::Shape::convex_polygon(
                                vec![
                                    pair[0],
                                    pair[1],
                                    egui::pos2(pair[1].x, rect.bottom()),
                                    egui::pos2(pair[0].x, rect.bottom()),
                                ],
                                fill_color,
                                egui::Stroke::NONE,
                            ));
                        }

                        // Line
                        let line_color = th::load_color(ui, cpu_avg);
                        for pair in pts.windows(2) {
                            painter.line_segment(
                                [pair[0], pair[1]],
                                egui::Stroke::new(1.5_f32, line_color),
                            );
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
        }

        ui.add_space(spacing);

        // ── Disk + Network I/O graphs ────────────────────────────────────────
        ui.horizontal(|ui| {
            let half_w = (ui.available_width() - spacing) / 2.0;
            dual_io_graph(
                ui,
                "Disk I/O",
                disk_io_history,
                ("▼ Read", s.ok),
                ("▲ Write", s.warning),
                half_w,
                border_color,
            );
            ui.add_space(spacing);
            dual_io_graph(
                ui,
                "Network I/O",
                net_io_history,
                ("▼ Recv", s.accent),
                ("▲ Send", s.manual),
                half_w,
                border_color,
            );
        });

        ui.add_space(spacing);

        // ── Top Processes by CPU ─────────────────────────────────────────────
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0_f32, border_color))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(
                    RichText::new("Top processes (CPU%)")
                        .strong()
                        .size(tokens::FONT_HEADING),
                );
                ui.add_space(4.0);

                let mut top: Vec<&ProcInfo> = snapshot.iter().collect();
                top.sort_by(|a, b| {
                    b.cpu_percent
                        .partial_cmp(&a.cpu_percent)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                top.truncate(10);

                if top.is_empty() {
                    ui.label(RichText::new("No processes").weak());
                    return;
                }

                let avail_w = ui.available_width();
                let row_h = 24.0;
                let bar_col_w = avail_w * 0.35;
                let name_w = avail_w * 0.30;
                let pid_w = 60.0;
                let cpu_w = 60.0;
                let mem_w = 80.0;

                // Header — weak, not accent (§3); numeric columns right-aligned
                let hdr_bg = ui.visuals().widgets.noninteractive.bg_fill;
                let (hr, _) =
                    ui.allocate_exact_size(Vec2::new(avail_w, row_h), egui::Sense::hover());
                ui.painter().rect_filled(hr, 0.0, hdr_bg);
                let hdr_font = egui::FontId::proportional(tokens::FONT_LABEL);
                let hdr_col = ui.visuals().weak_text_color();
                let mut hx = hr.min.x + 4.0;
                for (label, w, numeric) in [
                    ("PID", pid_w, true),
                    ("NAME", name_w, false),
                    ("CPU%", cpu_w, true),
                    ("MEM (MB)", mem_w, true),
                    ("LOAD", bar_col_w, false),
                ] {
                    let (pos, align) = if numeric {
                        (
                            egui::pos2(hx + w - 10.0, hr.center().y),
                            egui::Align2::RIGHT_CENTER,
                        )
                    } else {
                        (egui::pos2(hx, hr.center().y), egui::Align2::LEFT_CENTER)
                    };
                    ui.painter()
                        .text(pos, align, label, hdr_font.clone(), hdr_col);
                    hx += w;
                }

                for (i, proc) in top.iter().enumerate() {
                    let row_bg = if i % 2 == 1 {
                        ui.visuals().faint_bg_color
                    } else {
                        ui.visuals().extreme_bg_color
                    };
                    let (rr, _) =
                        ui.allocate_exact_size(Vec2::new(avail_w, row_h), egui::Sense::hover());
                    ui.painter().rect_filled(rr, 0.0, row_bg);

                    let text_col = ui.visuals().text_color();
                    let font = egui::FontId::proportional(tokens::FONT_BODY);
                    // §2: numeric cells are monospace + right-aligned so they
                    // don't jitter as values change on every refresh.
                    let num_font = th::num_font(tokens::FONT_BODY);
                    let mut rx = rr.min.x + 4.0;
                    let cpu_pct = proc.cpu_percent;
                    let mem_mb = proc.mem_rss as f64 / 1_048_576.0;

                    // PID
                    ui.painter().text(
                        egui::pos2(rx + pid_w - 10.0, rr.center().y),
                        egui::Align2::RIGHT_CENTER,
                        proc.pid.to_string(),
                        num_font.clone(),
                        text_col,
                    );
                    rx += pid_w;
                    // Name
                    // Truncate on chars, not bytes — a byte slice can split a
                    // multibyte UTF-8 name and panic every frame.
                    let name_display = if proc.name.chars().count() > 22 {
                        let truncated: String = proc.name.chars().take(21).collect();
                        format!("{truncated}…")
                    } else {
                        proc.name.clone()
                    };
                    ui.painter().text(
                        egui::pos2(rx, rr.center().y),
                        egui::Align2::LEFT_CENTER,
                        name_display,
                        font.clone(),
                        text_col,
                    );
                    rx += name_w;
                    // CPU% — value carries the load colour
                    ui.painter().text(
                        egui::pos2(rx + cpu_w - 10.0, rr.center().y),
                        egui::Align2::RIGHT_CENTER,
                        format!("{cpu_pct:.1}"),
                        num_font.clone(),
                        th::load_color(ui, cpu_pct),
                    );
                    rx += cpu_w;
                    // Mem
                    ui.painter().text(
                        egui::pos2(rx + mem_w - 10.0, rr.center().y),
                        egui::Align2::RIGHT_CENTER,
                        format!("{mem_mb:.1}"),
                        num_font.clone(),
                        text_col,
                    );
                    rx += mem_w;
                    // CPU bar
                    let bar_margin = 4.0;
                    let bar_rect = egui::Rect::from_min_size(
                        egui::pos2(rx, rr.min.y + bar_margin),
                        Vec2::new(bar_col_w - bar_margin * 2.0, row_h - bar_margin * 2.0),
                    );
                    ui.painter()
                        .rect_filled(bar_rect, 2.0, ui.visuals().extreme_bg_color);
                    let fill_w = (bar_rect.width() * (cpu_pct / 100.0).clamp(0.0, 1.0)).max(0.0);
                    let fill = egui::Rect::from_min_size(
                        bar_rect.min,
                        Vec2::new(fill_w, bar_rect.height()),
                    );
                    ui.painter()
                        .rect_filled(fill, 2.0, th::load_color(ui, cpu_pct));
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
    if total == 0 {
        return None;
    }
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

/// Small two-line rate graph (e.g. disk read/write, net rx/tx) with
/// autoscaled Y axis and current-value labels.
#[allow(clippy::too_many_arguments)]
fn dual_io_graph(
    ui: &mut egui::Ui,
    title: &str,
    history: &VecDeque<(f32, f32)>,
    (label_a, color_a): (&str, Color32),
    (label_b, color_b): (&str, Color32),
    width: f32,
    border_color: Color32,
) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0_f32, border_color))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_min_width(width - 16.0);
            ui.set_max_width(width - 16.0);
            let (cur_a, cur_b) = history.back().copied().unwrap_or((0.0, 0.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(color_b, format!("{label_b} {cur_b:.1} MB/s"));
                    ui.colored_label(color_a, format!("{label_a} {cur_a:.1} MB/s"));
                });
            });
            ui.add_space(4.0);

            let graph_h = 60.0;
            let (rect, _) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), graph_h),
                egui::Sense::hover(),
            );
            let painter = ui.painter();
            painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);

            if history.len() >= 2 {
                // Shared autoscale so the two lines are comparable.
                let peak = history
                    .iter()
                    .map(|&(a, b)| a.max(b))
                    .fold(0.0f32, f32::max)
                    .max(0.1);
                let n = history.len();
                for (select, color) in [
                    (0usize, color_a), // .0 = first series
                    (1usize, color_b), // .1 = second series
                ] {
                    let pts: Vec<egui::Pos2> = history
                        .iter()
                        .enumerate()
                        .map(|(i, &(a, b))| {
                            let v = if select == 0 { a } else { b };
                            let x = rect.left() + i as f32 / (n - 1) as f32 * rect.width();
                            let y = rect.bottom() - (v / peak) * (rect.height() - 4.0) - 2.0;
                            egui::pos2(x, y)
                        })
                        .collect();
                    for pair in pts.windows(2) {
                        painter.line_segment([pair[0], pair[1]], egui::Stroke::new(1.5_f32, color));
                    }
                }
                // Peak label top-right
                painter.text(
                    rect.right_top() + Vec2::new(-4.0, 2.0),
                    egui::Align2::RIGHT_TOP,
                    format!("peak {peak:.1} MB/s"),
                    egui::FontId::proportional(10.0),
                    ui.visuals().weak_text_color(),
                );
            }
        });
}
