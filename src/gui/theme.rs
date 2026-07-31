//! KDE Breeze Dark and Breeze Light colour palettes + egui theme application.

use egui::{Color32, Context, CornerRadius, FontId, Stroke, Style, Visuals};

// ── Theme selection ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum AppTheme {
    BreezeDark,
    BreezeLight,
}

impl AppTheme {
    pub fn label(&self) -> &'static str {
        match self {
            AppTheme::BreezeDark => "Breeze Dark",
            AppTheme::BreezeLight => "Breeze Light",
        }
    }

    /// Stable string key stored in config.toml.
    pub fn to_str(&self) -> &'static str {
        match self {
            AppTheme::BreezeDark => "BreezeDark",
            AppTheme::BreezeLight => "BreezeLight",
        }
    }

    /// Parse from config.toml value; unknown → BreezeDark.
    pub fn from_str(s: &str) -> Self {
        match s {
            "BreezeLight" => AppTheme::BreezeLight,
            _ => AppTheme::BreezeDark,
        }
    }
}

/// Returns the WINDOW_BG (r, g, b) for the active theme (used by opacity fallback).
pub fn window_bg_rgb(theme: &AppTheme) -> (u8, u8, u8) {
    match theme {
        AppTheme::BreezeDark => (0x31, 0x36, 0x3b),
        AppTheme::BreezeLight => (0xef, 0xf0, 0xf1),
    }
}

/// Apply opacity to the panel/window fills of a child viewport context.
/// Call at the START of a `show_viewport_immediate` callback.
/// Returns the original fills so they can be restored at the END of the callback.
pub fn push_viewport_opacity(ctx: &Context, opacity: f32) -> (Color32, Color32) {
    let orig_panel = ctx.style().visuals.panel_fill;
    let orig_window = ctx.style().visuals.window_fill;
    if opacity < 0.999 {
        let a = (opacity * 255.0) as u8;
        let tint = |c: Color32| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a);
        ctx.style_mut(|s| {
            s.visuals.panel_fill = tint(s.visuals.panel_fill);
            s.visuals.window_fill = tint(s.visuals.window_fill);
        });
    }
    (orig_panel, orig_window)
}

/// Restore panel/window fills saved by [`push_viewport_opacity`].
pub fn pop_viewport_opacity(ctx: &Context, saved: (Color32, Color32)) {
    ctx.style_mut(|s| {
        s.visuals.panel_fill = saved.0;
        s.visuals.window_fill = saved.1;
    });
}

/// Apply the selected theme.
pub fn apply_theme(ctx: &Context, native_ppp: f32, theme: &AppTheme) {
    match theme {
        AppTheme::BreezeDark => apply(ctx, native_ppp),
        AppTheme::BreezeLight => apply_light(ctx, native_ppp),
    }
}

// ── Breeze Dark palette ───────────────────────────────────────────────────────

pub struct Breeze;

impl Breeze {
    // Backgrounds
    pub const WINDOW_BG: Color32 = Color32::from_rgb(0x31, 0x36, 0x3b); // #31363b
    pub const BASE: Color32 = Color32::from_rgb(0x23, 0x26, 0x29); // #232629
    pub const ALT_BASE: Color32 = Color32::from_rgb(0x2a, 0x2e, 0x32); // #2a2e32

    // Text (dark theme values — use ui.visuals().text_color() in widgets for theme-awareness)
    pub const TEXT: Color32 = Color32::from_rgb(0xef, 0xf0, 0xf1); // #eff0f1

    // Accent — same Breeze blue in both themes, safe to use directly for accent labels
    pub const HIGHLIGHT: Color32 = Color32::from_rgb(0x3d, 0xae, 0xe9); // #3daee9 Breeze blue
    pub const LINK: Color32 = Color32::from_rgb(0x29, 0x80, 0xb9); // #2980b9

    // Dark-theme border (only used inside theme.rs apply(); use ui.visuals() everywhere else)
    pub const BORDER: Color32 = Color32::from_rgb(0x4d, 0x4d, 0x4d); // #4d4d4d

    // Buttons / widgets
    pub const BUTTON_BG: Color32 = Color32::from_rgb(0x31, 0x36, 0x3b); // #31363b
    pub const BUTTON_HOVER: Color32 = Color32::from_rgb(0x3a, 0x3f, 0x44); // #3a3f44

    // Semantic colours — used for CPU load, status indicators, log lines
    pub const POSITIVE: Color32 = Color32::from_rgb(0x27, 0xae, 0x60); // #27ae60  green
    pub const WARNING: Color32 = Color32::from_rgb(0xf6, 0x74, 0x00); // #f67400  orange
    pub const NEGATIVE: Color32 = Color32::from_rgb(0xda, 0x44, 0x53); // #da4453  red
}

// ── Design tokens ─────────────────────────────────────────────────────────────
// One source of truth for type scale and spacing, so tabs stop inventing
// their own magic numbers. Use these for all new UI.

pub mod tokens {
    /// Fine print: table meta, thread lists, sublabels
    pub const FONT_SMALL: f32 = 11.0;
    /// Column headers, chip labels, badges
    pub const FONT_LABEL: f32 = 11.5;
    /// Help text under group titles
    pub const FONT_HELP: f32 = 12.5;
    /// Default body/table text
    pub const FONT_BODY: f32 = 13.5;
    /// Section headings inside cards
    pub const FONT_HEADING: f32 = 15.0;
    /// Hero status headline (Gaming Mode / ProBalance status cards)
    pub const FONT_HERO: f32 = 17.0;
    /// KPI card value
    pub const FONT_KPI: f32 = 26.0;

    /// Tight spacing inside a group
    pub const SPACE_XS: f32 = 4.0;
    /// Between related elements
    pub const SPACE_S: f32 = 8.0;
    /// Between sections/cards
    pub const SPACE_M: f32 = 12.0;

    /// Standard data row height
    pub const ROW_H: f32 = 26.0;
    /// Dense data row height (HW Monitor sensors)
    pub const ROW_H_DENSE: f32 = 22.0;
    /// Left label column width in settings forms
    pub const FORM_LABEL_W: f32 = 220.0;
}

// ── Component primitives ──────────────────────────────────────────────────────
//
// Shared widgets so every tab renders chips/badges/toggles identically.

/// Pill-shaped filter chip in the accent colour.
pub fn chip(ui: &mut egui::Ui, label: &str, active: bool) -> bool {
    let accent = sem(ui).accent;
    chip_colored(ui, label, active, accent)
}

/// Pill-shaped filter chip in an explicit colour (log/HW categories carry
/// their own semantic colour). Returns true when clicked; the active state
/// gets a filled tint and a trailing × hinting that clicking clears it.
pub fn chip_colored(ui: &mut egui::Ui, label: &str, active: bool, color: Color32) -> bool {
    let s = Sem {
        accent: color,
        ..sem(ui)
    };
    let text = if active {
        format!("{label} ×")
    } else {
        label.to_string()
    };
    let galley = ui.painter().layout_no_wrap(
        text.clone(),
        egui::FontId::proportional(tokens::FONT_LABEL),
        if active {
            s.accent
        } else {
            ui.visuals().weak_text_color()
        },
    );
    let pad = egui::vec2(9.0, 4.0);
    let size = galley.size() + pad * 2.0;
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let fill = if active {
            tint(s.accent, 46)
        } else if hovered {
            ui.visuals().widgets.hovered.bg_fill
        } else {
            Color32::TRANSPARENT
        };
        let stroke = if active {
            Stroke::new(1.0_f32, s.accent)
        } else {
            Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color)
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(12),
            fill,
            stroke,
            egui::StrokeKind::Inside,
        );
        ui.painter().galley(rect.min + pad, galley, Color32::WHITE);
    }
    resp.clicked()
}

/// Small status badge (filled tint + coloured text), e.g. "Throttled".
pub fn badge(ui: &mut egui::Ui, label: &str, color: Color32) {
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(tokens::FONT_LABEL),
        color,
    );
    let pad = egui::vec2(7.0, 1.0);
    let size = galley.size() + pad * 2.0;
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(3), tint(color, 46));
        ui.painter().galley(rect.min + pad, galley, Color32::WHITE);
    }
}

/// Paint a badge into an explicit rect (for painter-driven tables).
pub fn badge_at(painter: &egui::Painter, pos: egui::Pos2, label: &str, color: Color32) {
    let galley = painter.layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(tokens::FONT_LABEL),
        color,
    );
    let pad = egui::vec2(7.0, 1.0);
    let rect = egui::Rect::from_min_size(
        egui::pos2(pos.x, pos.y - galley.size().y * 0.5 - pad.y),
        galley.size() + pad * 2.0,
    );
    painter.rect_filled(rect, CornerRadius::same(3), tint(color, 46));
    painter.galley(rect.min + pad, galley, Color32::WHITE);
}

/// Neutral outlined badge (match types, kinds).
pub fn badge_outline(ui: &mut egui::Ui, label: &str) {
    let col = ui.visuals().weak_text_color();
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(tokens::FONT_LABEL),
        col,
    );
    let pad = egui::vec2(7.0, 1.0);
    let size = galley.size() + pad * 2.0;
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(3),
            Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
            egui::StrokeKind::Inside,
        );
        ui.painter().galley(rect.min + pad, galley, Color32::WHITE);
    }
}

/// iOS-style toggle switch (26×14 pill). Returns true when toggled.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> bool {
    let s = sem(ui);
    let size = egui::vec2(26.0, 14.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
    }
    if ui.is_rect_visible(rect) {
        let fill = if *on {
            s.accent
        } else {
            ui.visuals().widgets.inactive.bg_fill
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(7),
            fill,
            Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
            egui::StrokeKind::Inside,
        );
        let cx = if *on {
            rect.right() - 7.0
        } else {
            rect.left() + 7.0
        };
        ui.painter().circle_filled(
            egui::pos2(cx, rect.center().y),
            5.0,
            if *on {
                s.on_accent
            } else {
                ui.visuals().text_color()
            },
        );
    }
    resp.clicked()
}

/// Segmented button group. Returns the index clicked, if any.
pub fn segmented(ui: &mut egui::Ui, options: &[&str], selected: usize) -> Option<usize> {
    let s = sem(ui);
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, opt) in options.iter().enumerate() {
            let is_sel = i == selected;
            let galley = ui.painter().layout_no_wrap(
                opt.to_string(),
                egui::FontId::proportional(tokens::FONT_BODY),
                if is_sel {
                    s.on_accent
                } else {
                    ui.visuals().text_color()
                },
            );
            let pad = egui::vec2(10.0, 4.0);
            let size = galley.size() + pad * 2.0;
            let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
            if ui.is_rect_visible(rect) {
                // Round only the outer corners of the group
                let r = 3;
                let radius = CornerRadius {
                    nw: if i == 0 { r } else { 0 },
                    sw: if i == 0 { r } else { 0 },
                    ne: if i == options.len() - 1 { r } else { 0 },
                    se: if i == options.len() - 1 { r } else { 0 },
                };
                let fill = if is_sel {
                    s.accent
                } else if resp.hovered() {
                    ui.visuals().widgets.hovered.bg_fill
                } else {
                    ui.visuals().widgets.inactive.bg_fill
                };
                ui.painter().rect(
                    rect,
                    radius,
                    fill,
                    Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
                    egui::StrokeKind::Inside,
                );
                ui.painter().galley(rect.min + pad, galley, Color32::WHITE);
            }
            if resp.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// KPI card: uppercase label, big tabular value, weak detail line, and a
/// 3px accent stripe down the left edge in `stripe`.
pub fn kpi_card(
    ui: &mut egui::Ui,
    width: f32,
    label: &str,
    value: &str,
    detail: &str,
    stripe: Color32,
) {
    let resp = egui::Frame::new()
        .stroke(Stroke::new(
            1.0_f32,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: 10,
            bottom: 10,
        })
        .corner_radius(CornerRadius::same(4))
        .show(ui, |ui| {
            ui.set_width(width);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(label.to_uppercase())
                        .size(tokens::FONT_SMALL)
                        .color(ui.visuals().weak_text_color()),
                );
                ui.label(
                    egui::RichText::new(value)
                        .size(tokens::FONT_KPI)
                        .strong()
                        .monospace(),
                );
                ui.label(
                    egui::RichText::new(detail)
                        .size(tokens::FONT_HELP)
                        .color(ui.visuals().weak_text_color()),
                );
            });
        });
    // Left accent stripe, painted on the card's own rect so it lands inside
    // the border rather than in the gap before the next card.
    let card = resp.response.rect;
    let stripe_rect = egui::Rect::from_min_size(card.min, egui::vec2(3.0, card.height()));
    ui.painter().rect_filled(
        stripe_rect,
        CornerRadius {
            nw: 4,
            sw: 4,
            ne: 0,
            se: 0,
        },
        stripe,
    );
}

/// Bottom action bar for settings-style tabs: dirty indicator on the left,
/// Discard + Apply on the right. Returns (discard_clicked, apply_clicked).
pub fn apply_bar(ui: &mut egui::Ui, dirty: bool) -> (bool, bool) {
    let s = sem(ui);
    let mut discard = false;
    let mut apply = false;
    ui.add_space(tokens::SPACE_S);
    ui.separator();
    ui.horizontal(|ui| {
        if dirty {
            ui.colored_label(s.warning, "● Unsaved changes");
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let apply_btn = egui::Button::new(
                egui::RichText::new("Apply changes")
                    .color(s.on_accent)
                    .strong(),
            )
            .fill(s.accent);
            if ui.add_enabled(dirty, apply_btn).clicked() {
                apply = true;
            }
            if ui
                .add_enabled(dirty, egui::Button::new("Discard"))
                .clicked()
            {
                discard = true;
            }
        });
    });
    (discard, apply)
}

/// Column-header label: weak, small — never accent blue, which reads as
/// "interactive/selected". Only the active sort column gets strong text.
pub fn header_text(ui: &egui::Ui, label: &str, active: bool) -> egui::RichText {
    let t = egui::RichText::new(label).size(tokens::FONT_LABEL);
    if active {
        t.strong().color(ui.visuals().strong_text_color())
    } else {
        t.color(ui.visuals().weak_text_color())
    }
}

/// Monospace font for numeric cells — prevents jitter on live refresh.
pub fn num_font(size: f32) -> egui::FontId {
    egui::FontId::monospace(size)
}

/// QGroupBox-style bordered card with a top-left title — THE section container
/// for every tab (single definition; per-tab copies are deprecated).
pub fn card(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    let border_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0_f32, border_color))
        .inner_margin(egui::Margin::same(8))
        .corner_radius(egui::CornerRadius::same(4))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                egui::RichText::new(title)
                    .strong()
                    .size(tokens::FONT_HEADING)
                    .color(ui.visuals().strong_text_color()),
            );
            ui.add_space(tokens::SPACE_XS);
            add_contents(ui);
        });
}

/// Full-width alert/notice banner with semantic colour and an optional action
/// button. Returns true if the action button was clicked.
pub fn banner(ui: &mut egui::Ui, color: Color32, text: &str, action: Option<&str>) -> bool {
    let mut clicked = false;
    egui::Frame::new()
        .fill(Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            26,
        ))
        .stroke(egui::Stroke::new(1.0_f32, color))
        .inner_margin(egui::Margin::same(8))
        .corner_radius(egui::CornerRadius::same(4))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.colored_label(color, text);
                if let Some(label) = action {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(label).clicked() {
                            clicked = true;
                        }
                    });
                }
            });
        });
    clicked
}

// ── Semantic colour system ────────────────────────────────────────────────────
//
// ONE source of truth for every semantic colour in the app. Tabs must never
// hardcode rgb literals — call `sem(ui)` (or the ramp below) so Breeze Light
// automatically gets its darker, AA-contrast variants.

/// Semantic palette resolved for the active theme.
#[derive(Debug, Clone, Copy)]
pub struct Sem {
    /// Healthy / low load / success
    pub ok: Color32,
    /// Mid load (≥50%)
    pub mid: Color32,
    /// Elevated / attention (≥70%)
    pub warning: Color32,
    /// Critical / destructive (≥85%)
    pub negative: Color32,
    /// Accent + selection (also "rules"/"network" info blue)
    pub accent: Color32,
    /// Readable text colour on top of an accent-filled surface
    pub on_accent: Color32,
    /// Manual/user actions (log category, network TX)
    pub manual: Color32,
}

/// Semantic palette for the active theme (dark vs. Breeze Light).
pub fn sem(ui: &egui::Ui) -> Sem {
    sem_for(ui.visuals().dark_mode)
}

/// Semantic palette for an explicit theme mode.
pub fn sem_for(dark_mode: bool) -> Sem {
    if dark_mode {
        Sem {
            ok: Color32::from_rgb(0x27, 0xae, 0x60),
            mid: Color32::from_rgb(0x8b, 0xc3, 0x4a),
            warning: Color32::from_rgb(0xf6, 0x74, 0x00),
            negative: Color32::from_rgb(0xda, 0x44, 0x53),
            accent: Color32::from_rgb(0x3d, 0xae, 0xe9),
            on_accent: Color32::from_rgb(0x0d, 0x14, 0x18),
            manual: Color32::from_rgb(0x9b, 0x59, 0xb6),
        }
    } else {
        // Breeze Light: accent darkens (#3daee9 has too little contrast on
        // white) and the semantic ramp darkens for AA contrast on light fills.
        Sem {
            ok: Color32::from_rgb(0x1e, 0x84, 0x49),
            mid: Color32::from_rgb(0x68, 0x9f, 0x38),
            warning: Color32::from_rgb(0xc0, 0x50, 0x00),
            negative: Color32::from_rgb(0xc0, 0x39, 0x2b),
            accent: Color32::from_rgb(0x29, 0x80, 0xb9),
            on_accent: Color32::WHITE,
            manual: Color32::from_rgb(0x7d, 0x3c, 0x98),
        }
    }
}

/// Same colour at a given alpha — for chip fills, heat strips, badges.
pub fn tint(c: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

// ── CPU load colour ramp (Breeze semantic colours) ────────────────────────────

/// Load colour for `pct` ∈ [0,100] in the ACTIVE theme. Use this for bars,
/// table values, sparklines and graphs so one ramp drives everything.
pub fn load_color(ui: &egui::Ui, pct: f32) -> Color32 {
    load_color_for(ui.visuals().dark_mode, pct)
}

/// Load colour for an explicit theme mode.
pub fn load_color_for(dark_mode: bool, pct: f32) -> Color32 {
    let s = sem_for(dark_mode);
    let stops = [
        (0.0, s.ok),
        (50.0, s.mid),
        (70.0, s.warning),
        (85.0, s.negative),
        (100.0, s.negative),
    ];
    let pct = pct.clamp(0.0, 100.0);
    for i in 0..stops.len() - 1 {
        let (p0, c0) = stops[i];
        let (p1, c1) = stops[i + 1];
        if pct <= p1 {
            let t = if (p1 - p0).abs() > f32::EPSILON {
                (pct - p0) / (p1 - p0)
            } else {
                0.0
            };
            let lerp = |a: u8, b: u8| (a as f32 + t * (b as f32 - a as f32)).round() as u8;
            return Color32::from_rgb(
                lerp(c0.r(), c1.r()),
                lerp(c0.g(), c1.g()),
                lerp(c0.b(), c1.b()),
            );
        }
    }
    s.negative
}

/// Dark-theme ramp kept for painters that have no `Ui` at hand.
pub fn cpu_load_color(pct: f32) -> Color32 {
    const RAMP: &[(f32, u8, u8, u8)] = &[
        (0.0, 0x27, 0xae, 0x60),   // Breeze green  (#27ae60)
        (50.0, 0x8b, 0xc3, 0x4a),  // mid green-yellow
        (70.0, 0xf6, 0x74, 0x00),  // Breeze orange (#f67400)
        (85.0, 0xda, 0x44, 0x53),  // Breeze red    (#da4453)
        (100.0, 0xb0, 0x20, 0x2e), // deep red
    ];

    let pct = pct.clamp(0.0, 100.0);
    for i in 0..RAMP.len() - 1 {
        let (p0, r0, g0, b0) = RAMP[i];
        let (p1, r1, g1, b1) = RAMP[i + 1];
        if pct <= p1 {
            let t = if (p1 - p0).abs() > f32::EPSILON {
                (pct - p0) / (p1 - p0)
            } else {
                0.0
            };
            let lerp =
                |a: u8, b: u8| -> u8 { (a as f32 + t * (b as f32 - a as f32)).round() as u8 };
            return Color32::from_rgb(lerp(r0, r1), lerp(g0, g1), lerp(b0, b1));
        }
    }
    let (_, r, g, b) = *RAMP.last().unwrap();
    Color32::from_rgb(r, g, b)
}

// ── Row highlight colours for the process table ───────────────────────────────

/// Text colour for a process row given its CPU load and throttle state.
/// `text_color` should be `ui.visuals().text_color()` so it adapts to Breeze Dark/Light.
pub fn row_color(cpu_pct: f32, throttled: bool, text_color: Color32, dark_mode: bool) -> Color32 {
    // The dark-theme accents (Breeze yellow/green/orange) have poor contrast
    // as TEXT on a light background — use darkened variants there.
    let (warn, hot, warm, calm) = if dark_mode {
        (
            Breeze::WARNING,
            Breeze::NEGATIVE,
            Color32::from_rgb(0xfd, 0xbc, 0x4b), // Breeze yellow
            Breeze::POSITIVE,
        )
    } else {
        (
            Color32::from_rgb(0xa7, 0x4f, 0x00), // dark amber
            Color32::from_rgb(0xb2, 0x27, 0x36), // dark red
            Color32::from_rgb(0x8a, 0x66, 0x00), // dark yellow-brown
            Color32::from_rgb(0x1d, 0x7d, 0x46), // dark green
        )
    };
    if throttled {
        warn
    } else if cpu_pct >= 80.0 {
        hot
    } else if cpu_pct >= 40.0 {
        warm
    } else if cpu_pct >= 10.0 {
        calm
    } else {
        text_color
    }
}

// ── Theme application ─────────────────────────────────────────────────────────

pub fn apply(ctx: &Context, native_ppp: f32) {
    // Ensure the rendering scale matches the display's native DPI so fonts
    // don't shrink when the theme is reapplied (e.g. after toggling system theme).
    ctx.set_pixels_per_point(native_ppp);
    let mut style = Style::default();

    let mut vis = Visuals::dark();

    // ── Backgrounds ──────────────────────────────────────────────────────
    vis.window_fill = Breeze::WINDOW_BG;
    vis.panel_fill = Breeze::WINDOW_BG;
    vis.faint_bg_color = Breeze::ALT_BASE;
    vis.extreme_bg_color = Breeze::BASE;

    // ── Text ─────────────────────────────────────────────────────────────
    vis.override_text_color = Some(Breeze::TEXT);

    // ── Widgets ──────────────────────────────────────────────────────────
    let rounding = CornerRadius::same(4);

    // non-interactive (labels, separators)
    vis.widgets.noninteractive.bg_fill = Breeze::WINDOW_BG;
    vis.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Breeze::BORDER);
    vis.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Breeze::BORDER);
    vis.widgets.noninteractive.corner_radius = rounding;

    // inactive (buttons, checkboxes at rest)
    vis.widgets.inactive.bg_fill = Breeze::BUTTON_BG;
    vis.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Breeze::BORDER);
    vis.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, Breeze::TEXT);
    vis.widgets.inactive.corner_radius = rounding;

    // hovered
    vis.widgets.hovered.bg_fill = Breeze::BUTTON_HOVER;
    vis.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Breeze::HIGHLIGHT);
    vis.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Breeze::TEXT);
    vis.widgets.hovered.corner_radius = rounding;

    // active (pressed)
    vis.widgets.active.bg_fill = Breeze::HIGHLIGHT;
    vis.widgets.active.bg_stroke = Stroke::new(1.0_f32, Breeze::HIGHLIGHT);
    vis.widgets.active.fg_stroke = Stroke::new(1.5_f32, Breeze::TEXT);
    vis.widgets.active.corner_radius = rounding;

    // open (combo boxes, menus)
    vis.widgets.open.bg_fill = Breeze::ALT_BASE;
    vis.widgets.open.bg_stroke = Stroke::new(1.0_f32, Breeze::HIGHLIGHT);
    vis.widgets.open.fg_stroke = Stroke::new(1.0_f32, Breeze::TEXT);
    vis.widgets.open.corner_radius = rounding;

    // ── Selection ────────────────────────────────────────────────────────
    vis.selection.bg_fill = Color32::from_rgba_unmultiplied(0x3d, 0xae, 0xe9, 0x66); // 40% alpha
    vis.selection.stroke = Stroke::new(1.0_f32, Breeze::HIGHLIGHT);

    // ── Misc ─────────────────────────────────────────────────────────────
    vis.hyperlink_color = Breeze::LINK;
    vis.window_stroke = Stroke::new(1.0_f32, Breeze::BORDER);
    vis.window_shadow = egui::epaint::Shadow::NONE;
    vis.window_corner_radius = CornerRadius::same(4);

    // Striped table alternate row colour
    vis.faint_bg_color = Breeze::ALT_BASE;

    style.visuals = vis;

    // ── Typography ───────────────────────────────────────────────────────
    style.text_styles = {
        use egui::TextStyle::*;
        [
            (Small, FontId::proportional(12.0)),
            (Body, FontId::proportional(14.0)),
            (Button, FontId::proportional(14.0)),
            (Heading, FontId::proportional(16.0)),
            (Monospace, FontId::monospace(13.0)),
        ]
        .into()
    };

    // ── Spacing — comfortable row height ─────────────────────────────────
    style.spacing.interact_size.y = 24.0;
    style.spacing.item_spacing = egui::vec2(8.0, 4.0);

    // ── Striped table — more visible alt row ─────────────────────────────
    // faint_bg_color is set in vis above; override again after style.visuals assignment
    // to ensure it survives (vis already set above, so we patch style.visuals here)
    style.visuals.faint_bg_color = Color32::from_rgb(0x3d, 0x41, 0x47); // visibly lighter than WINDOW_BG

    ctx.set_style(style);
}

// ── Breeze Light theme ────────────────────────────────────────────────────────

pub fn apply_light(ctx: &Context, native_ppp: f32) {
    ctx.set_pixels_per_point(native_ppp);
    let mut style = Style::default();

    let mut vis = Visuals::light();

    // Backgrounds
    let window_bg = Color32::from_rgb(0xef, 0xf0, 0xf1); // #eff0f1
    let base = Color32::from_rgb(0xff, 0xff, 0xff); // #ffffff  table row even
    let alt_base = Color32::from_rgb(0xf4, 0xf4, 0xf4); // #f4f4f4  table row odd
    let tab_bar = Color32::from_rgb(0xd5, 0xd9, 0xde); // #d5d9de  tab bar / panel bg

    vis.window_fill = window_bg;
    vis.panel_fill = tab_bar;
    vis.faint_bg_color = alt_base;
    vis.extreme_bg_color = base;

    // Text
    let text = Color32::from_rgb(0x31, 0x36, 0x3b); // #31363b
    vis.override_text_color = Some(text);

    // Borders / buttons
    let border = Color32::from_rgb(0xb0, 0xb8, 0xc0); // #b0b8c0 — sharper than before
    let button_bg = Color32::from_rgb(0xef, 0xf0, 0xf1);
    let button_hov = Color32::from_rgb(0xe0, 0xe4, 0xe8);
    let button_dis = Color32::from_rgb(0xdd, 0xe1, 0xe5); // #dde1e5 disabled bg
    let text_dis = Color32::from_rgb(0xa0, 0xa8, 0xb0); // #a0a8b0 disabled fg
    let highlight = Color32::from_rgb(0x3d, 0xae, 0xe9); // #3daee9 Breeze blue

    let rounding = CornerRadius::same(4);

    vis.widgets.noninteractive.bg_fill = window_bg;
    vis.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, border);
    vis.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, border);
    vis.widgets.noninteractive.corner_radius = rounding;

    vis.widgets.inactive.bg_fill = button_bg;
    vis.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, border);
    vis.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, text);
    vis.widgets.inactive.corner_radius = rounding;

    vis.widgets.hovered.bg_fill = button_hov;
    vis.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, highlight);
    vis.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, text);
    vis.widgets.hovered.corner_radius = rounding;

    // Active (pressed) — Breeze blue fill, white text
    vis.widgets.active.bg_fill = highlight;
    vis.widgets.active.bg_stroke = Stroke::new(1.0_f32, highlight);
    vis.widgets.active.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);
    vis.widgets.active.corner_radius = rounding;

    // Open (combo / menu dropdown) — slightly darker base
    vis.widgets.open.bg_fill = button_hov;
    vis.widgets.open.bg_stroke = Stroke::new(1.0_f32, highlight);
    vis.widgets.open.fg_stroke = Stroke::new(1.0_f32, text);
    vis.widgets.open.corner_radius = rounding;

    // Selection — Breeze blue at ~31% alpha (premultiplied: 61,174,233 * 80/255 ≈ 19,55,73)
    vis.selection.bg_fill = Color32::from_rgba_premultiplied(19, 55, 73, 80);
    vis.selection.stroke = Stroke::new(1.0_f32, highlight);

    vis.hyperlink_color = Color32::from_rgb(0x29, 0x80, 0xb9);
    vis.window_stroke = Stroke::new(1.0_f32, border);
    vis.window_shadow = egui::epaint::Shadow::NONE;
    vis.window_corner_radius = CornerRadius::same(4);

    style.visuals = vis;

    style.text_styles = {
        use egui::TextStyle::*;
        [
            (Small, FontId::proportional(12.0)),
            (Body, FontId::proportional(14.0)),
            (Button, FontId::proportional(14.0)),
            (Heading, FontId::proportional(16.0)),
            (Monospace, FontId::monospace(13.0)),
        ]
        .into()
    };

    style.spacing.interact_size.y = 24.0;
    style.spacing.item_spacing = egui::vec2(8.0, 4.0);
    // Striped table alternate row — explicitly set after visuals assignment
    style.visuals.faint_bg_color = alt_base;

    ctx.set_style(style);

    let _ = (button_dis, text_dis); // available for disabled-widget callers
}
