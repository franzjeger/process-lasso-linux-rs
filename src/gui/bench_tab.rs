//! Benchmark tab — memory latency test (AIDA64-style pointer chasing).
//!
//! Results appear in a dedicated OS-level window (egui viewport) so they can
//! be moved and resized independently of the main application window.

use egui::{Color32, Pos2, Rect, RichText, Stroke, Vec2};

use crate::mem_bench::{
    BandwidthResult, CacheSizes, MemBandwidthBench, MemLatencyBench, MemLatencyResult, TEST_SIZES,
};

// ── State ─────────────────────────────────────────────────────────────────────

pub struct BenchTab {
    bench:        MemLatencyBench,
    last:         MemLatencyResult,
    cache:        CacheSizes,
    results_open: bool,
    auto_opened:  bool,
    graph_hover:  Option<usize>,
    // Bandwidth
    bw_bench:   MemBandwidthBench,
    last_bw:    BandwidthResult,
    /// History of completed bandwidth runs (newest last)
    bw_history: Vec<BandwidthResult>,
    // Async CSV save
    csv_tx: std::sync::mpsc::Sender<String>,
    csv_rx: std::sync::mpsc::Receiver<String>,
    csv_status: String,
}

impl BenchTab {
    pub fn new() -> Self {
        let (csv_tx, csv_rx) = std::sync::mpsc::channel();
        Self {
            bench:        MemLatencyBench::new(),
            last:         MemLatencyResult::default(),
            cache:        CacheSizes::read(),
            results_open: false,
            auto_opened:  false,
            graph_hover:  None,
            bw_bench:     MemBandwidthBench::new(),
            last_bw:      BandwidthResult::default(),
            bw_history:   Vec::new(),
            csv_tx,
            csv_rx,
            csv_status: String::new(),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        // Drain async CSV save status messages
        while let Ok(msg) = self.csv_rx.try_recv() {
            self.csv_status = msg;
        }

        let snap = self.bench.snapshot();
        if snap.running || snap.complete { self.last = snap; }

        if self.last.complete && !self.auto_opened {
            self.results_open = true;
            self.auto_opened  = true;
        }
        if self.last.running { self.auto_opened = false; }

        let border_color = ui.visuals().widgets.noninteractive.bg_stroke.color;

        // ── Progress view ─────────────────────────────────────────────────────
        if self.last.running {
            egui::Frame::new()
                .stroke(Stroke::new(1.0, border_color))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() { self.bench.cancel(); }
                        let pct      = self.last.progress * 100.0;
                        let size_str = self.last.current_size.map(fmt_size).unwrap_or_default();
                        ui.label(format!("Running… {pct:.0}%  —  current: {size_str}"));
                    });
                    ui.add_space(6.0);
                    ui.add(
                        egui::ProgressBar::new(self.last.progress)
                            .animate(true)
                            .desired_width(ui.available_width()),
                    );
                });
            ui.ctx().request_repaint();
            return;
        }

        // ── Wrap everything in a scroll area so both boxes are always accessible ─
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {

        // ── Memory Latency Benchmark ─────────────────────────────────────────
        bench_group(ui, border_color, "Memory Latency Benchmark", |ui| {
            ui.label(
                "Measures RAM latency using random pointer-chasing (one cache-line per hop).\n\
                 Identical method to AIDA64 Cache & Memory Benchmark — the CPU prefetcher\n\
                 cannot predict the next address, so the result reflects true hardware latency.",
            );
            ui.add_space(10.0);

            ui.label(RichText::new("Detected cache topology").strong());
            ui.add_space(4.0);
            egui::Grid::new("cache_grid").num_columns(2).spacing([20.0, 4.0]).show(ui, |ui| {
                let c = &self.cache;
                ui.label(RichText::new("L1 Data:").color(L1_COLOR).strong());
                ui.label(fmt_size(c.l1d));
                ui.end_row();
                ui.label(RichText::new("L2:").color(L2_COLOR).strong());
                ui.label(fmt_size(c.l2));
                ui.end_row();
                ui.label(RichText::new("L3:").color(L3_COLOR).strong());
                ui.label(fmt_size(c.l3));
                ui.end_row();
                ui.label(RichText::new("DRAM:").color(DRAM_COLOR).strong());
                ui.label(format!("> {}", fmt_size(c.l3)));
                ui.end_row();
            });

            ui.add_space(10.0);
            ui.label(
                RichText::new("Note: Test takes ~15-60 seconds and fully loads one CPU core.")
                    .color(Color32::from_rgb(240, 200, 70))
                    .size(11.5),
            );
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                if ui.button(RichText::new("Run latency test").size(13.0)).clicked() {
                    self.bench.start();
                }
                if self.last.complete {
                    ui.separator();
                    if ui.button("Show results").clicked() { self.results_open = true; }
                    if ui.button("Run again").clicked()    { self.bench.start(); }
                }
            });
        });

        ui.add_space(8.0);

        // ── Memory Bandwidth Benchmark ───────────────────────────────────────
        bench_group(ui, border_color, "Memory Bandwidth Benchmark", |ui| {
            ui.label(
                "Sequential read, write, and copy throughput — 256 MiB buffer, DRAM pressure.\n\
                 Results reported in GB/s.",
            );
            ui.add_space(8.0);

            // Bandwidth progress/results
            let bw = self.bw_bench.snapshot();
            let was_running = self.last_bw.running;
            if bw.running || bw.complete { self.last_bw = bw.clone(); }
            // Push to history when a run just completed
            if was_running && self.last_bw.complete {
                self.bw_history.push(self.last_bw.clone());
            }

            if self.last_bw.running {
                ui.horizontal(|ui| {
                    if ui.button("Cancel bandwidth").clicked() { self.bw_bench.cancel(); }
                    let stage = match (self.last_bw.progress * 3.0) as u32 {
                        0 => "Read…", 1 => "Write…", _ => "Copy…",
                    };
                    ui.label(format!("Running {stage}"));
                });
                ui.add(egui::ProgressBar::new(self.last_bw.progress)
                    .animate(true)
                    .desired_width(ui.available_width()));
                ui.ctx().request_repaint();
            } else if self.last_bw.complete {
                let highlight = Color32::from_rgb(140, 200, 100);
                let prev = if self.bw_history.len() >= 2 {
                    Some(&self.bw_history[self.bw_history.len() - 2])
                } else {
                    None
                };
                egui::Grid::new("bw_results")
                    .num_columns(if prev.is_some() { 3 } else { 2 })
                    .spacing([20.0, 4.0])
                    .show(ui, |ui| {
                        let bw = &self.last_bw;
                        if prev.is_some() {
                            ui.label(RichText::new("").strong());
                            ui.label(RichText::new("Current").strong());
                            ui.label(RichText::new("vs Prev").strong());
                            ui.end_row();
                        }
                        let delta_label = |cur: f64, prev: f64| -> RichText {
                            let d = cur - prev;
                            let s = format!("{:+.2} GB/s", d);
                            if d > 0.05f64 { RichText::new(s).color(Color32::from_rgb(100, 200, 100)) }
                            else if d < -0.05f64 { RichText::new(s).color(Color32::from_rgb(220, 80, 60)) }
                            else { RichText::new(s).color(Color32::GRAY) }
                        };
                        ui.label(RichText::new("Read:").strong());
                        ui.label(RichText::new(format!("{:.2} GB/s", bw.read_gb_s)).color(highlight).strong());
                        if let Some(p) = prev { ui.label(delta_label(bw.read_gb_s, p.read_gb_s)); }
                        ui.end_row();
                        ui.label(RichText::new("Write:").strong());
                        ui.label(RichText::new(format!("{:.2} GB/s", bw.write_gb_s)).color(highlight).strong());
                        if let Some(p) = prev { ui.label(delta_label(bw.write_gb_s, p.write_gb_s)); }
                        ui.end_row();
                        ui.label(RichText::new("Copy:").strong());
                        ui.label(RichText::new(format!("{:.2} GB/s", bw.copy_gb_s)).color(highlight).strong());
                        if let Some(p) = prev { ui.label(delta_label(bw.copy_gb_s, p.copy_gb_s)); }
                        ui.end_row();
                    });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Run bandwidth again").clicked() { self.bw_bench.start(); }
                    if self.bw_history.len() > 1 {
                        ui.label(RichText::new(format!("{} runs recorded", self.bw_history.len()))
                            .weak().size(11.5));
                    }
                });
            } else {
                if ui.button(RichText::new("Run bandwidth test").size(13.0)).clicked() {
                    self.bw_bench.start();
                }
            }

            if !self.csv_status.is_empty() {
                ui.add_space(4.0);
                ui.label(&self.csv_status);
            }
        });

        }); // end ScrollArea

        // ── Separate OS window for results ────────────────────────────────────
        if self.results_open && self.last.complete && !self.last.points.is_empty() {
            let ctx         = ui.ctx().clone();
            let points      = self.last.points.clone();
            let cache       = self.cache.clone();
            let old_hover   = self.graph_hover;
            let mut new_hover: Option<usize> = None;
            let mut close_requested = false;
            let csv_tx_clone = self.csv_tx.clone();

            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("bench_results"),
                egui::ViewportBuilder::default()
                    .with_title("Argus-Lasso — Memory Latency Results")
                    .with_app_id("argus-lasso")
                    .with_inner_size([800.0, 600.0])
                    .with_min_inner_size([540.0, 400.0])
                    .with_icon(egui::IconData {
                        rgba:   crate::icon::RGBA.to_vec(),
                        width:  crate::icon::W,
                        height: crate::icon::H,
                    }),
                |ctx, _class| {
                    if ctx.input(|i| i.viewport().close_requested()) {
                        close_requested = true;
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        new_hover = show_results(ui, &points, &cache, old_hover, csv_tx_clone.clone());
                    });
                },
            );

            self.graph_hover = new_hover;
            if close_requested { self.results_open = false; }
        }
    }
}

// ── Results content (rendered inside the separate viewport) ───────────────────

fn show_results(
    ui: &mut egui::Ui,
    points: &[crate::mem_bench::LatencyPoint],
    cache: &CacheSizes,
    hover_in: Option<usize>,
    csv_tx: std::sync::mpsc::Sender<String>,
) -> Option<usize> {
    let border_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let max_ns = points.iter().map(|p| p.latency_ns).fold(0.0_f64, f64::max);

    // ── Latency summary cards ─────────────────────────────────────────────────
    let avg_for = |lo: usize, hi: usize| -> Option<f64> {
        let v: Vec<f64> = points.iter()
            .filter(|p| p.size_bytes > lo && p.size_bytes <= hi)
            .map(|p| p.latency_ns).collect();
        if v.is_empty() { None } else { Some(v.iter().sum::<f64>() / v.len() as f64) }
    };
    let dram_ns: Option<f64> = {
        let v: Vec<f64> = points.iter().filter(|p| p.size_bytes > cache.l3)
            .map(|p| p.latency_ns).collect();
        if v.is_empty() { None } else {
            let s = v.len() / 2;
            Some(v[s..].iter().sum::<f64>() / (v.len() - s) as f64)
        }
    };

    egui::Frame::new()
        .stroke(Stroke::new(1.0, border_color))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                latency_card(ui, "L1",   avg_for(0, cache.l1d),          L1_COLOR,   border_color);
                latency_card(ui, "L2",   avg_for(cache.l1d, cache.l2),   L2_COLOR,   border_color);
                latency_card(ui, "L3",   avg_for(cache.l2,  cache.l3),   L3_COLOR,   border_color);
                latency_card(ui, "DRAM", dram_ns,                         DRAM_COLOR, border_color);
            });
        });

    ui.add_space(4.0);

    ui.horizontal(|ui| {
        // Run again button triggers a new bench — we can't access BenchTab here,
        // so we use a note label. The tab itself has the Run again button.
        ui.label(RichText::new("Close this window to run again from the Benchmark tab.").color(Color32::GRAY).size(11.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Save CSV").clicked() {
                let csv = build_csv(points);
                let tx = csv_tx.clone();
                std::thread::spawn(move || {
                    let path = match crate::file_dialog::save("memory_latency.csv", "*.csv") {
                        Some(p) => p,
                        None => return,
                    };
                    let msg = match std::fs::write(&path, &csv) {
                        Ok(_) => "CSV saved.".to_string(),
                        Err(e) => format!("Save failed: {e}"),
                    };
                    tx.send(msg).ok();
                });
            }
        });
    });

    ui.separator();

    // ── Graph ─────────────────────────────────────────────────────────────────
    let avail   = ui.available_size();
    let graph_h = (avail.y * 0.58).clamp(160.0, 420.0);

    let (outer_rect, response) = ui.allocate_exact_size(
        Vec2::new(avail.x, graph_h),
        egui::Sense::hover(),
    );

    let ml = 62.0; let mr = 12.0; let mt = 12.0; let mb = 34.0;
    let graph_rect = Rect::from_min_size(
        Pos2::new(outer_rect.min.x + ml, outer_rect.min.y + mt),
        Vec2::new(outer_rect.width() - ml - mr, graph_h - mt - mb),
    );

    let y_max   = nice_ceil(max_ns * 1.12);
    let log_min = (*TEST_SIZES.first().unwrap() as f64).log2();
    let log_max = (*TEST_SIZES.last().unwrap()  as f64).log2();
    let to_x = |b: usize| -> f32 {
        let t = ((b as f64).log2() - log_min) / (log_max - log_min);
        graph_rect.left() + t as f32 * graph_rect.width()
    };
    let to_y = |ns: f64| -> f32 {
        graph_rect.bottom() - (ns / y_max) as f32 * graph_rect.height()
    };

    let painter = ui.painter_at(outer_rect);

    // Cache region bands
    let regions: &[(usize, usize, Color32, &str, Color32)] = &[
        (0,         cache.l1d, Color32::from_rgba_premultiplied(30, 140, 55, 40),  "L1",   L1_COLOR),
        (cache.l1d, cache.l2,  Color32::from_rgba_premultiplied(90, 150, 30, 35),  "L2",   L2_COLOR),
        (cache.l2,  cache.l3,  Color32::from_rgba_premultiplied(170,130, 15, 30),  "L3",   L3_COLOR),
        (cache.l3,  *TEST_SIZES.last().unwrap() + 1,
                               Color32::from_rgba_premultiplied(170, 40, 40, 30),  "DRAM", DRAM_COLOR),
    ];
    for &(from, to, bg, label, tcol) in regions {
        let rx1 = to_x(from.max(*TEST_SIZES.first().unwrap()));
        let rx2 = to_x(to.min(*TEST_SIZES.last().unwrap()));
        if rx2 <= rx1 { continue; }
        let band = Rect::from_min_max(Pos2::new(rx1, graph_rect.top()), Pos2::new(rx2, graph_rect.bottom()));
        painter.rect_filled(band, 0.0, bg);
        painter.text(Pos2::new((rx1 + rx2) / 2.0, graph_rect.top() + 5.0),
            egui::Align2::CENTER_TOP, label, egui::FontId::proportional(11.0), tcol);
    }

    painter.rect_filled(graph_rect, 2.0, Color32::from_black_alpha(50));
    painter.rect_stroke(graph_rect, 2.0, Stroke::new(1.0, Color32::from_gray(70)), egui::StrokeKind::Outside);

    // Y grid
    for &ns in &nice_y_steps(y_max, 6) {
        let y = to_y(ns);
        if y < graph_rect.top() || y > graph_rect.bottom() { continue; }
        painter.line_segment([Pos2::new(graph_rect.left(), y), Pos2::new(graph_rect.right(), y)],
            Stroke::new(1.0, Color32::from_gray(45)));
        painter.text(Pos2::new(graph_rect.left() - 5.0, y), egui::Align2::RIGHT_CENTER,
            format!("{ns:.0} ns"), egui::FontId::proportional(10.0), Color32::GRAY);
    }

    // X grid
    for &size in TEST_SIZES {
        let x = to_x(size);
        painter.line_segment([Pos2::new(x, graph_rect.top()), Pos2::new(x, graph_rect.bottom())],
            Stroke::new(1.0, Color32::from_gray(40)));
        painter.text(Pos2::new(x, graph_rect.bottom() + 5.0), egui::Align2::CENTER_TOP,
            fmt_size_short(size), egui::FontId::proportional(9.5), Color32::GRAY);
    }

    let mut hover_out: Option<usize> = None;

    if points.len() >= 2 {
        let pts: Vec<Pos2> = points.iter()
            .map(|p| Pos2::new(to_x(p.size_bytes), to_y(p.latency_ns)))
            .collect();

        // Fill
        let mut fill = pts.clone();
        fill.push(Pos2::new(pts.last().unwrap().x, graph_rect.bottom()));
        fill.push(Pos2::new(pts[0].x, graph_rect.bottom()));
        painter.add(egui::Shape::convex_polygon(fill,
            Color32::from_rgba_premultiplied(60, 150, 255, 22), Stroke::NONE));

        for seg in pts.windows(2) {
            painter.line_segment([seg[0], seg[1]], Stroke::new(2.0, Color32::from_rgb(80, 165, 255)));
        }

        // Hover detection
        if let Some(mouse) = response.hover_pos() {
            let mut best = f32::MAX;
            for (i, pt) in pts.iter().enumerate() {
                let d = (pt.x - mouse.x).abs();
                if d < best && d < 18.0 { best = d; hover_out = Some(i); }
            }
        }

        // Dots
        for (i, (p, pt)) in points.iter().zip(pts.iter()).enumerate() {
            let hover = hover_in == Some(i) || hover_out == Some(i);
            let r   = if hover { 6.0 } else { 4.0 };
            let col = latency_color(p.latency_ns, max_ns);
            painter.circle_filled(*pt, r, col);
            painter.circle_stroke(*pt, r, Stroke::new(1.0, Color32::WHITE));

            if hover {
                let tip  = format!("{}  →  {:.1} ns", fmt_size(p.size_bytes), p.latency_ns);
                let tpos = Pos2::new(
                    (pt.x + 10.0).min(graph_rect.right() - 148.0),
                    (pt.y - 26.0).max(graph_rect.top() + 4.0),
                );
                let bg = Rect::from_min_size(tpos - Vec2::splat(3.0), Vec2::new(152.0, 22.0));
                painter.rect_filled(bg, 3.0, Color32::from_black_alpha(190));
                painter.text(tpos, egui::Align2::LEFT_TOP, tip,
                    egui::FontId::proportional(11.5), Color32::WHITE);
            }
        }
    }

    ui.add_space(6.0);
    ui.separator();

    // ── Detail table ──────────────────────────────────────────────────────────
    ui.label(RichText::new("Details").strong());
    ui.add_space(4.0);

    egui::Frame::new()
        .stroke(Stroke::new(1.0, border_color))
        .inner_margin(egui::Margin::same(0))
        .show(ui, |ui| {
            let hdr_bg = ui.visuals().widgets.noninteractive.bg_fill;
            let (hr, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), egui::Sense::hover());
            ui.painter().rect_filled(hr, 0.0, hdr_bg);
            let fh = egui::FontId::proportional(11.0);
            for (lbl, off) in [("Working Set", 10.0_f32), ("Latency", 130.0), ("Region", 230.0)] {
                ui.painter().text(Pos2::new(hr.min.x + off, hr.center().y),
                    egui::Align2::LEFT_CENTER, lbl, fh.clone(), Color32::GRAY);
            }
            ui.painter().line_segment([hr.left_bottom(), hr.right_bottom()],
                Stroke::new(1.0, border_color));

            egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                ui.style_mut().spacing.item_spacing.y = 0.0;
                for (idx, p) in points.iter().enumerate() {
                    let row_h = 19.0;
                    let (rr, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), egui::Sense::hover());
                    let bg = if idx % 2 == 0 { ui.visuals().extreme_bg_color } else { ui.visuals().faint_bg_color };
                    ui.painter().rect_filled(rr, 0.0, bg);
                    if idx > 0 {
                        ui.painter().line_segment([rr.left_top(), rr.right_top()],
                            Stroke::new(1.0, border_color));
                    }
                    let region = if p.size_bytes <= cache.l1d { "L1" }
                        else if p.size_bytes <= cache.l2 { "L2" }
                        else if p.size_bytes <= cache.l3 { "L3" }
                        else { "DRAM" };
                    let rcol = match region { "L1" => L1_COLOR, "L2" => L2_COLOR, "L3" => L3_COLOR, _ => DRAM_COLOR };
                    let font = egui::FontId::proportional(12.0);
                    let cy = rr.center().y; let rx = rr.min.x;
                    ui.painter().text(Pos2::new(rx + 10.0,  cy), egui::Align2::LEFT_CENTER,
                        fmt_size(p.size_bytes), font.clone(), ui.visuals().text_color());
                    ui.painter().text(Pos2::new(rx + 130.0, cy), egui::Align2::LEFT_CENTER,
                        format!("{:.2} ns", p.latency_ns), font.clone(), latency_color(p.latency_ns, max_ns));
                    ui.painter().text(Pos2::new(rx + 230.0, cy), egui::Align2::LEFT_CENTER,
                        region, font.clone(), rcol);
                }
            });
        });

    hover_out
}

// ── Latency card ──────────────────────────────────────────────────────────────

fn latency_card(ui: &mut egui::Ui, label: &str, ns: Option<f64>, color: Color32, border: Color32) {
    egui::Frame::new()
        .stroke(Stroke::new(1.0, border))
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.set_min_width(110.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(label).color(color).strong().size(12.0));
                ui.label(RichText::new(match ns {
                    Some(v) => format!("{v:.1} ns"),
                    None    => "—".into(),
                }).color(color).size(20.0).strong());
            });
        });
}

// ── Colours ───────────────────────────────────────────────────────────────────

const L1_COLOR:   Color32 = Color32::from_rgb( 80, 220, 120);
const L2_COLOR:   Color32 = Color32::from_rgb(160, 220,  80);
const L3_COLOR:   Color32 = Color32::from_rgb(240, 200,  60);
const DRAM_COLOR: Color32 = Color32::from_rgb(240,  90,  60);

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_csv(points: &[crate::mem_bench::LatencyPoint]) -> String {
    let mut s = String::from("size_bytes,size_label,latency_ns\n");
    for p in points {
        s.push_str(&format!("{},{},{:.3}\n", p.size_bytes, fmt_size(p.size_bytes), p.latency_ns));
    }
    s
}

pub fn fmt_size(bytes: usize) -> String {
    if bytes < 1024             { format!("{bytes} B") }
    else if bytes < 1024 * 1024 { format!("{} KiB", bytes / 1024) }
    else                        { format!("{} MiB", bytes / (1024 * 1024)) }
}

fn fmt_size_short(bytes: usize) -> &'static str {
    match bytes {
              4096 => "4K",       8192 => "8K",
             16384 => "16K",    32768 => "32K",
             65536 => "64K",   131072 => "128K",
            262144 => "256K",  524288 => "512K",
           1048576 => "1M",   2097152 => "2M",
           4194304 => "4M",   8388608 => "8M",
          16777216 => "16M", 33554432 => "32M",
          67108864 => "64M", 134217728 => "128M",
         268435456 => "256M",         _ => "?",
    }
}

fn latency_color(ns: f64, max_ns: f64) -> Color32 {
    let t = (ns / max_ns.max(1.0)).clamp(0.0, 1.0) as f32;
    if t < 0.5 {
        let u = t * 2.0;
        Color32::from_rgb((80.0 + u * 160.0) as u8, 220, (120.0 - u * 60.0) as u8)
    } else {
        let u = (t - 0.5) * 2.0;
        Color32::from_rgb(240, (200.0 - u * 110.0) as u8, (60.0 - u * 30.0) as u8)
    }
}

fn nice_ceil(v: f64) -> f64 {
    if v <= 0.0 { return 100.0; }
    let mag  = 10.0_f64.powf(v.log10().floor());
    let frac = v / mag;
    (if frac <= 1.0 { 1.0 } else if frac <= 2.0 { 2.0 } else if frac <= 5.0 { 5.0 } else { 10.0 }) * mag
}

fn nice_y_steps(max: f64, n: usize) -> Vec<f64> {
    let step = nice_ceil(max / n as f64);
    let mut v = Vec::new();
    let mut cur = step;
    while cur <= max + step * 0.5 { v.push(cur); cur += step; }
    v
}

/// Render a titled bordered group box (matches the style used in settings_tab and other tabs).
fn bench_group(ui: &mut egui::Ui, border_color: egui::Color32, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, border_color))
        .inner_margin(egui::Margin::same(12))
        .corner_radius(egui::CornerRadius::same(4))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(egui::RichText::new(title).size(14.0).strong().color(ui.visuals().strong_text_color()));
            ui.add_space(6.0);
            add_contents(ui);
        });
}
