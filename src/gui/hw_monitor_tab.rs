//! Hardware Monitor tab — HWiNFO-style sensor view.
//!
//! Renders sensor groups grouped by category. Each group is wrapped in a
//! visible border frame. A single global column header above the scroll area
//! has always-visible separator lines and drag-to-resize handles.

use egui::{Color32, Stroke, Ui, Vec2};

use crate::hw_monitor::{HwMonitorData, Sensor, HISTORY_LEN};

// Category display order
const CATEGORY_ORDER: &[&str] = &["CPU", "GPU", "Memory", "Storage", "Network", "System"];

// Highlight colour for the active resize handle (same blue as Breeze accent)
const RESIZE_HIGHLIGHT: Color32 = Color32::from_rgb(100, 150, 255);

// ── State ─────────────────────────────────────────────────────────────────────

pub struct HwMonitorTab {
    pub show_sparklines: bool,
    pub filter: String,
    /// Fixed column widths: [value, min, max, avg].  Name column fills the rest.
    pub col_widths: [f32; 4],
    last_avail_w: f32,
    /// Set to true when user drags a column handle; app.rs persists col_widths to config.
    pub cols_dirty: bool,
}

impl HwMonitorTab {
    pub fn new() -> Self {
        Self {
            show_sparklines: true,
            filter: String::new(),
            col_widths: [100.0, 72.0, 72.0, 72.0],
            last_avail_w: 0.0,
            cols_dirty: false,
        }
    }

    pub fn new_with_widths(widths: &[f32]) -> Self {
        let mut s = Self::new();
        if widths.len() == 4 {
            s.col_widths = [widths[0], widths[1], widths[2], widths[3]];
        }
        s
    }

    pub fn show(&mut self, ui: &mut Ui, data: &HwMonitorData) {
        // ── Toolbar ──────────────────────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_sparklines, "Sparklines");
            ui.separator();
            ui.label("Filter:");
            ui.text_edit_singleline(&mut self.filter);
            if ui.small_button("x").clicked() { self.filter.clear(); }
        });
        ui.separator();

        if data.groups.is_empty() {
            ui.centered_and_justified(|ui| { ui.label("Reading sensors..."); });
            return;
        }

        let filter_lc = self.filter.to_lowercase();
        let sparkline_w: f32 = if self.show_sparklines { 80.0 } else { 0.0 };

        // ── Column widths — auto-scale when the window is resized ─────────────
        let avail_w = ui.available_width();
        if self.last_avail_w > 0.0 && (avail_w - self.last_avail_w).abs() > 4.0 {
            let ratio = avail_w / self.last_avail_w.max(1.0);
            for w in &mut self.col_widths {
                *w = (*w * ratio).clamp(30.0, 300.0);
            }
        }
        self.last_avail_w = avail_w;

        let fixed_w: f32 = self.col_widths.iter().sum::<f32>() + sparkline_w;
        let col_name = (avail_w - fixed_w - 4.0).max(130.0);

        // Local copy of widths (before any drag deltas this frame)
        let [col_val, col_min, col_max, col_avg] = self.col_widths;

        // ── Global column header (outside scroll area) ────────────────────────
        let border_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let hdr_h = 22.0;
        let (hdr_rect, _) = ui.allocate_exact_size(
            Vec2::new(avail_w, hdr_h),
            egui::Sense::hover(),
        );
        ui.painter().rect_filled(
            hdr_rect,
            0.0,
            ui.visuals().widgets.noninteractive.bg_fill,
        );
        // Bottom separator doubles as the top border of the content below
        ui.painter().line_segment(
            [hdr_rect.left_bottom(), hdr_rect.right_bottom()],
            Stroke::new(1.0, border_color),
        );

        let hdr_color = ui.visuals().weak_text_color();
        let font = egui::FontId::proportional(11.0);
        let pad = 5.0;

        ui.painter().text(
            egui::pos2(hdr_rect.min.x + pad, hdr_rect.center().y),
            egui::Align2::LEFT_CENTER,
            "SENSOR",
            font.clone(),
            hdr_color,
        );

        let mut x = hdr_rect.min.x + col_name;
        for (w, label) in [(col_val, "VALUE"), (col_min, "MIN"), (col_max, "MAX"), (col_avg, "AVG")] {
            ui.painter().text(
                egui::pos2(x + pad, hdr_rect.center().y),
                egui::Align2::LEFT_CENTER,
                label,
                font.clone(),
                hdr_color,
            );
            x += w;
        }
        if self.show_sparklines {
            ui.painter().text(
                egui::pos2(x + pad, hdr_rect.center().y),
                egui::Align2::LEFT_CENTER,
                "HISTORY",
                font.clone(),
                hdr_color,
            );
        }

        // Separator after SENSOR column (not resizable — name fills the rest)
        let name_edge = hdr_rect.min.x + col_name;
        ui.painter().line_segment(
            [egui::pos2(name_edge, hdr_rect.min.y), egui::pos2(name_edge, hdr_rect.max.y)],
            Stroke::new(1.0, border_color),
        );

        // Resize handles between the four fixed columns (value/min/max/avg)
        let mut col_deltas = [0.0f32; 4];
        let mut hx = name_edge;
        for i in 0..4usize {
            hx += self.col_widths[i];
            let handle = egui::Rect::from_min_size(
                egui::pos2(hx - 3.0, hdr_rect.min.y),
                egui::vec2(6.0, hdr_h),
            );
            let resp = ui.interact(
                handle,
                egui::Id::new(("hw_col_resize", i)),
                egui::Sense::drag(),
            );
            let line_col = if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
                RESIZE_HIGHLIGHT
            } else {
                border_color
            };
            ui.painter().line_segment(
                [egui::pos2(hx, hdr_rect.min.y), egui::pos2(hx, hdr_rect.max.y)],
                Stroke::new(1.0, line_col),
            );
            if resp.dragged() {
                col_deltas[i] = resp.drag_delta().x;
            }
        }
        // Apply drag deltas for next frame
        self.cols_dirty = false;
        for (i, &d) in col_deltas.iter().enumerate() {
            if d.abs() > 0.001 {
                self.col_widths[i] = (self.col_widths[i] + d).max(30.0);
                self.cols_dirty = true;
            }
        }

        // ── Scrollable sensor groups ──────────────────────────────────────────
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut rendered_cats: std::collections::HashSet<&str> = Default::default();

                let all_cats: Vec<&str> = {
                    let mut v: Vec<&str> = CATEGORY_ORDER.to_vec();
                    for g in &data.groups {
                        if !v.contains(&g.category) { v.push(g.category); }
                    }
                    v
                };

                for category in all_cats {
                    if rendered_cats.contains(category) { continue; }
                    rendered_cats.insert(category);

                    let cat_groups: Vec<_> = data.groups.iter()
                        .filter(|g| g.category == category)
                        .collect();
                    if cat_groups.is_empty() { continue; }

                    // Category header
                    ui.add_space(6.0);
                    let cat_color = category_color(category);
                    ui.label(
                        egui::RichText::new(format!("| {category}"))
                            .strong()
                            .size(14.0)
                            .color(cat_color),
                    );
                    ui.add_space(2.0);

                    for group in &cat_groups {
                        let visible: Vec<&Sensor> = group.sensors.iter()
                            .filter(|s| {
                                filter_lc.is_empty()
                                    || s.label.to_lowercase().contains(&filter_lc)
                                    || group.name.to_lowercase().contains(&filter_lc)
                                    || category.to_lowercase().contains(&filter_lc)
                            })
                            .collect();
                        if visible.is_empty() { continue; }

                        let border_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
                        egui::Frame::new()
                            .stroke(Stroke::new(1.0, border_color))
                            .inner_margin(egui::Margin::same(1))
                            .show(ui, |ui| {
                                // Group name header (no column labels — those are in the global header)
                                let group_hdr_h = 20.0;
                                let (group_rect, _) = ui.allocate_exact_size(
                                    Vec2::new(ui.available_width(), group_hdr_h),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    group_rect,
                                    0.0,
                                    ui.visuals().widgets.noninteractive.bg_fill,
                                );
                                ui.painter().text(
                                    egui::pos2(group_rect.min.x + 6.0, group_rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    &group.name,
                                    egui::FontId::proportional(12.5),
                                    cat_color,
                                );
                                ui.painter().line_segment(
                                    [group_rect.left_bottom(), group_rect.right_bottom()],
                                    Stroke::new(1.0, border_color),
                                );

                                // Sensor rows
                                for (row_idx, sensor) in visible.iter().enumerate() {
                                    let row_h = 19.0;
                                    let (row_rect, _) = ui.allocate_exact_size(
                                        Vec2::new(ui.available_width(), row_h),
                                        egui::Sense::hover(),
                                    );

                                    let bg = if row_idx % 2 == 0 {
                                        ui.visuals().extreme_bg_color
                                    } else {
                                        ui.visuals().faint_bg_color
                                    };
                                    ui.painter().rect_filled(row_rect, 0.0, bg);

                                    if row_idx > 0 {
                                        ui.painter().line_segment(
                                            [row_rect.left_top(), row_rect.right_top()],
                                            Stroke::new(1.0, border_color),
                                        );
                                    }

                                    let pad_left = 4.0;

                                    // Sensor name (clipped to name column)
                                    ui.painter().text(
                                        egui::pos2(row_rect.min.x + 16.0, row_rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        sensor.label,
                                        egui::FontId::proportional(12.0),
                                        ui.visuals().text_color(),
                                    );

                                    let mut rx = row_rect.min.x + col_name;

                                    // Value (colored)
                                    ui.painter().text(
                                        egui::pos2(rx + pad_left, row_rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        fmt_val(sensor.value, sensor.unit),
                                        egui::FontId::proportional(12.0),
                                        value_color(sensor.value, sensor.unit, ui.visuals().text_color()),
                                    );
                                    rx += col_val;

                                    // Min
                                    ui.painter().text(
                                        egui::pos2(rx + pad_left, row_rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        fmt_val(sensor.min_display(), sensor.unit),
                                        egui::FontId::proportional(11.5),
                                        Color32::from_rgb(100, 200, 140),
                                    );
                                    rx += col_min;

                                    // Max
                                    ui.painter().text(
                                        egui::pos2(rx + pad_left, row_rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        fmt_val(sensor.max_display(), sensor.unit),
                                        egui::FontId::proportional(11.5),
                                        Color32::from_rgb(220, 110, 90),
                                    );
                                    rx += col_max;

                                    // Avg
                                    ui.painter().text(
                                        egui::pos2(rx + pad_left, row_rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        fmt_val(sensor.avg(), sensor.unit),
                                        egui::FontId::proportional(11.5),
                                        Color32::GRAY,
                                    );
                                    rx += col_avg;

                                    // Sparkline
                                    if self.show_sparklines {
                                        let spark_rect = egui::Rect::from_min_size(
                                            egui::pos2(rx + 2.0, row_rect.min.y + 2.0),
                                            Vec2::new(sparkline_w - 4.0, row_h - 4.0),
                                        );
                                        draw_sparkline(ui, spark_rect, &sensor.history, sensor.unit);
                                    }
                                }
                            }); // end group Frame

                        ui.add_space(4.0);
                    }
                }
            });
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn category_color(cat: &str) -> Color32 {
    match cat {
        "CPU"     => Color32::from_rgb(100, 180, 255),
        "GPU"     => Color32::from_rgb(140, 230, 100),
        "Memory"  => Color32::from_rgb(200, 140, 255),
        "Storage" => Color32::from_rgb(255, 190, 80),
        "Network" => Color32::from_rgb( 80, 220, 200),
        _         => Color32::GRAY,
    }
}

fn fmt_val(v: f32, unit: &str) -> String {
    match unit {
        "°C"   => format!("{v:.1} °C"),
        "RPM"  => format!("{v:.0} RPM"),
        "W"    => format!("{v:.2} W"),
        "V"    => format!("{v:.3} V"),
        "MHz"  => format!("{v:.0} MHz"),
        "GiB"  => format!("{v:.2} GiB"),
        "MB"   => format!("{v:.0} MB"),
        "MB/s" => format!("{v:.2} MB/s"),
        "%"    => format!("{v:.1}%"),
        "Wh"   => format!("{v:.2} Wh"),
        ""     => format!("{v:.2}"),
        u      => format!("{v:.2} {u}"),
    }
}

fn value_color(v: f32, unit: &str, fallback: Color32) -> Color32 {
    match unit {
        "°C" => temp_color(v),
        "%" => {
            if v >= 90.0      { Color32::from_rgb(240,  80,  60) }
            else if v >= 70.0 { Color32::from_rgb(240, 180,  60) }
            else              { Color32::from_rgb(100, 210, 140) }
        }
        "W" => {
            if v >= 300.0      { Color32::from_rgb(240, 100,  60) }
            else if v >= 150.0 { Color32::from_rgb(240, 200,  80) }
            else               { fallback }
        }
        _ => fallback,
    }
}

fn temp_color(c: f32) -> Color32 {
    if c >= 90.0      { Color32::from_rgb(255,  50,  30) }
    else if c >= 80.0 { Color32::from_rgb(255, 110,  40) }
    else if c >= 70.0 { Color32::from_rgb(240, 195,  55) }
    else if c >= 60.0 { Color32::from_rgb(180, 220,  75) }
    else              { Color32::from_rgb( 80, 210, 130) }
}

fn draw_sparkline(
    ui: &mut Ui,
    rect: egui::Rect,
    history: &std::collections::VecDeque<f32>,
    unit: &str,
) {
    if history.len() < 2 { return; }

    let painter = ui.painter_at(rect);
    let vals: Vec<f32> = history.iter().copied().collect();

    let lo = vals.iter().cloned().fold(f32::INFINITY, f32::min);
    let hi = vals.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = (hi - lo).max(0.001);

    let w = rect.width();
    let h = rect.height();

    let px = |i: usize| rect.left() + i as f32 / (HISTORY_LEN as f32 - 1.0) * w;
    let py = |v: f32|   rect.bottom() - (v - lo) / range * (h - 1.0);

    painter.rect_filled(rect, 2.0, Color32::from_black_alpha(50));

    let line_color = match unit {
        "°C"   => Color32::from_rgb(240, 120,  60),
        "%"    => Color32::from_rgb( 80, 180, 240),
        "W"    => Color32::from_rgb(220, 180,  60),
        "MHz"  => Color32::from_rgb(160, 100, 240),
        "MB/s" => Color32::from_rgb( 80, 220, 200),
        _      => Color32::from_rgb(120, 210, 140),
    };

    let points: Vec<egui::Pos2> = vals.iter().enumerate()
        .map(|(i, &v)| egui::pos2(px(i), py(v)))
        .collect();

    for pair in points.windows(2) {
        painter.line_segment([pair[0], pair[1]], Stroke::new(1.0, line_color));
    }
}
