//! Per-CPU bar widget and rolling CPU history chart.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke, Ui, Vec2};
use std::collections::VecDeque;

use crate::gui::theme;
use crate::utils;

// ── CpuBarsWidget ─────────────────────────────────────────────────────────────

/// Compact grid of per-CPU horizontal bars with load color coding and frequency.
pub struct CpuBarsWidget {
    pub cpu_pcts: Vec<f32>,
    pub cpu_freqs: Vec<Option<u64>>,
    pub offline: std::collections::HashSet<u32>,
}

impl CpuBarsWidget {
    pub fn new() -> Self {
        Self {
            cpu_pcts: Vec::new(),
            cpu_freqs: Vec::new(),
            offline: Default::default(),
        }
    }

    pub fn update(&mut self, pcts: Vec<f32>) {
        self.offline = utils::get_offline_cpus();
        self.cpu_freqs = (0..pcts.len())
            .map(|i| {
                let path = format!("/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_cur_freq");
                std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| s.trim().parse::<u64>().ok())
            })
            .collect();
        self.cpu_pcts = pcts;
    }

    pub fn show(&self, ui: &mut Ui) {
        let n = self.cpu_pcts.len();
        if n == 0 {
            return;
        }

        let avail_w = ui.available_width();
        let bar_h = 36.0;
        let gap = 4.0;
        let label_w = 30.0;

        // Choose column count to fill width with minimal wasted slots
        let bar_min_w = 110.0;
        let max_cols = ((avail_w / bar_min_w) as usize).max(1).min(n);
        let cols = (1..=max_cols)
            .rev()
            .find(|&c| n % c == 0)
            .unwrap_or(max_cols);
        let rows = (n + cols - 1) / cols;
        let bar_w = ((avail_w - gap * (cols as f32 + 1.0)) / cols as f32).max(60.0);

        let total_h = rows as f32 * (bar_h + gap) + gap;
        let (resp, painter) = ui.allocate_painter(Vec2::new(avail_w, total_h), egui::Sense::hover());
        let base = resp.rect.min;

        let border_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let text_color   = ui.visuals().text_color();
        let offline_text = ui.visuals().weak_text_color();
        let bar_bg       = ui.visuals().extreme_bg_color;
        let offline_bg   = ui.visuals().faint_bg_color;

        for i in 0..n {
            let col = (i % cols) as f32;
            let row = (i / cols) as f32;
            let x = base.x + gap + col * (bar_w + gap);
            let y = base.y + gap + row * (bar_h + gap);
            let rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(bar_w, bar_h));

            let pct = self.cpu_pcts[i];
            let is_offline = self.offline.contains(&(i as u32));

            // Background: dimmer for offline bars
            let bg_color = if is_offline { offline_bg } else { bar_bg };
            painter.rect_filled(rect, CornerRadius::same(4), bg_color);

            // Filled portion
            if !is_offline && pct > 0.0 {
                let fill_w = ((bar_w - label_w - 2.0) * pct / 100.0).max(0.0);
                let fill_rect = Rect::from_min_size(
                    Pos2::new(x + label_w + 1.0, y + 2.0),
                    Vec2::new(fill_w, bar_h - 4.0),
                );
                painter.rect_filled(fill_rect, CornerRadius::same(2), theme::cpu_load_color(pct));
            }

            // Border
            painter.rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, border_color), egui::StrokeKind::Middle);

            // CPU index label
            let idx_color = if is_offline { offline_text } else { text_color };
            painter.text(
                Pos2::new(x + label_w - 3.0, y + bar_h / 2.0 - 6.0),
                egui::Align2::RIGHT_CENTER,
                i.to_string(),
                egui::FontId::monospace(12.0),
                idx_color,
            );

            // Percentage / "off" text
            let pct_text = if is_offline { "off".to_string() } else { format!("{pct:.0}%") };
            let pct_color = if is_offline { offline_text } else { text_color };
            painter.text(
                Pos2::new(x + bar_w - 3.0, y + bar_h / 2.0 - 6.0),
                egui::Align2::RIGHT_CENTER,
                pct_text,
                egui::FontId::monospace(12.0),
                pct_color,
            );

            // Frequency line (second row inside bar)
            if !is_offline {
                let freq_text = if let Some(khz) = self.cpu_freqs.get(i).copied().flatten() {
                    if khz >= 1_000_000 {
                        format!("{:.1}G", khz as f64 / 1_000_000.0)
                    } else {
                        format!("{}M", khz / 1_000)
                    }
                } else {
                    String::new()
                };
                if !freq_text.is_empty() {
                    painter.text(
                        Pos2::new(x + bar_w - 3.0, y + bar_h / 2.0 + 8.0),
                        egui::Align2::RIGHT_CENTER,
                        freq_text,
                        egui::FontId::monospace(11.0),
                        offline_text,
                    );
                }
            }
        }
    }
}

// ── CpuHistoryWidget ──────────────────────────────────────────────────────────

/// Rolling area chart of average CPU utilisation (120 samples).
pub struct CpuHistoryWidget {
    pub history: VecDeque<f32>,
    pub max_samples: usize,
}

impl CpuHistoryWidget {
    pub fn new() -> Self {
        Self {
            history: VecDeque::new(),
            max_samples: 120,
        }
    }

    pub fn push(&mut self, avg: f32) {
        self.history.push_back(avg);
        while self.history.len() > self.max_samples {
            self.history.pop_front();
        }
    }

    pub fn show(&self, ui: &mut Ui) {
        if self.history.len() < 2 {
            return;
        }

        let h = 48.0;
        let avail_w = ui.available_width();
        let (resp, painter) = ui.allocate_painter(Vec2::new(avail_w, h), egui::Sense::hover());
        let rect = resp.rect;

        painter.rect_filled(rect, CornerRadius::ZERO, ui.visuals().extreme_bg_color);

        let n = self.history.len();
        let slot_w = avail_w / self.max_samples as f32;

        let pts: Vec<Pos2> = self
            .history
            .iter()
            .enumerate()
            .map(|(i, &pct)| {
                let offset = self.max_samples - n;
                let x = rect.min.x + (offset + i) as f32 * slot_w;
                let y = rect.max.y - 2.0 - (h - 4.0) * pct / 100.0;
                Pos2::new(x, y)
            })
            .collect();

        // Build filled path
        let last_avg = *self.history.back().unwrap_or(&0.0);
        let fill_color = {
            let c = theme::cpu_load_color(last_avg);
            Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 100)
        };

        let mut path: Vec<Pos2> = Vec::with_capacity(pts.len() + 2);
        path.push(Pos2::new(pts[0].x, rect.max.y - 2.0));
        path.extend_from_slice(&pts);
        path.push(Pos2::new(pts.last().unwrap().x, rect.max.y - 2.0));

        painter.add(egui::Shape::convex_polygon(path, fill_color, Stroke::NONE));

        // Line on top
        painter.add(egui::Shape::line(pts, Stroke::new(1.5, theme::cpu_load_color(last_avg))));

        // Border
        let border_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
        painter.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, border_color), egui::StrokeKind::Middle);
    }
}
