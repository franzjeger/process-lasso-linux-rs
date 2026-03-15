//! KDE Breeze Dark and Breeze Light colour palettes + egui theme application.

use egui::{
    Color32, Context, CornerRadius, FontId, Stroke, Style, Visuals,
};

// ── Theme selection ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum AppTheme {
    BreezeDark,
    BreezeLight,
}

impl AppTheme {
    pub fn label(&self) -> &'static str {
        match self {
            AppTheme::BreezeDark  => "Breeze Dark",
            AppTheme::BreezeLight => "Breeze Light",
        }
    }

    /// Stable string key stored in config.toml.
    pub fn to_str(&self) -> &'static str {
        match self {
            AppTheme::BreezeDark  => "BreezeDark",
            AppTheme::BreezeLight => "BreezeLight",
        }
    }

    /// Parse from config.toml value; unknown → BreezeDark.
    pub fn from_str(s: &str) -> Self {
        match s {
            "BreezeLight" => AppTheme::BreezeLight,
            _             => AppTheme::BreezeDark,
        }
    }
}

/// Returns the WINDOW_BG (r, g, b) for the active theme (used by opacity fallback).
pub fn window_bg_rgb(theme: &AppTheme) -> (u8, u8, u8) {
    match theme {
        AppTheme::BreezeDark  => (0x31, 0x36, 0x3b),
        AppTheme::BreezeLight => (0xef, 0xf0, 0xf1),
    }
}

/// Apply opacity to the panel/window fills of a child viewport context.
/// Call at the START of a `show_viewport_immediate` callback.
/// Returns the original fills so they can be restored at the END of the callback.
pub fn push_viewport_opacity(ctx: &Context, opacity: f32) -> (Color32, Color32) {
    let orig_panel  = ctx.style().visuals.panel_fill;
    let orig_window = ctx.style().visuals.window_fill;
    if opacity < 0.999 {
        let a = (opacity * 255.0) as u8;
        let tint = |c: Color32| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a);
        ctx.style_mut(|s| {
            s.visuals.panel_fill  = tint(s.visuals.panel_fill);
            s.visuals.window_fill = tint(s.visuals.window_fill);
        });
    }
    (orig_panel, orig_window)
}

/// Restore panel/window fills saved by [`push_viewport_opacity`].
pub fn pop_viewport_opacity(ctx: &Context, saved: (Color32, Color32)) {
    ctx.style_mut(|s| {
        s.visuals.panel_fill  = saved.0;
        s.visuals.window_fill = saved.1;
    });
}

/// Apply the selected theme.
pub fn apply_theme(ctx: &Context, native_ppp: f32, theme: &AppTheme) {
    match theme {
        AppTheme::BreezeDark  => apply(ctx, native_ppp),
        AppTheme::BreezeLight => apply_light(ctx, native_ppp),
    }
}

// ── Breeze Dark palette ───────────────────────────────────────────────────────

pub struct Breeze;

impl Breeze {
    // Backgrounds
    pub const WINDOW_BG:    Color32 = Color32::from_rgb(0x31, 0x36, 0x3b); // #31363b
    pub const BASE:         Color32 = Color32::from_rgb(0x23, 0x26, 0x29); // #232629
    pub const ALT_BASE:     Color32 = Color32::from_rgb(0x2a, 0x2e, 0x32); // #2a2e32

    // Text (dark theme values — use ui.visuals().text_color() in widgets for theme-awareness)
    pub const TEXT:         Color32 = Color32::from_rgb(0xef, 0xf0, 0xf1); // #eff0f1

    // Accent — same Breeze blue in both themes, safe to use directly for accent labels
    pub const HIGHLIGHT:    Color32 = Color32::from_rgb(0x3d, 0xae, 0xe9); // #3daee9 Breeze blue
    pub const LINK:         Color32 = Color32::from_rgb(0x29, 0x80, 0xb9); // #2980b9

    // Dark-theme border (only used inside theme.rs apply(); use ui.visuals() everywhere else)
    pub const BORDER:       Color32 = Color32::from_rgb(0x4d, 0x4d, 0x4d); // #4d4d4d

    // Buttons / widgets
    pub const BUTTON_BG:    Color32 = Color32::from_rgb(0x31, 0x36, 0x3b); // #31363b
    pub const BUTTON_HOVER: Color32 = Color32::from_rgb(0x3a, 0x3f, 0x44); // #3a3f44

    // Semantic colours — used for CPU load, status indicators, log lines
    pub const POSITIVE:     Color32 = Color32::from_rgb(0x27, 0xae, 0x60); // #27ae60  green
    pub const WARNING:      Color32 = Color32::from_rgb(0xf6, 0x74, 0x00); // #f67400  orange
    pub const NEGATIVE:     Color32 = Color32::from_rgb(0xda, 0x44, 0x53); // #da4453  red
}

// ── CPU load colour ramp (Breeze semantic colours) ────────────────────────────

/// Returns a colour along the green→orange→red Breeze ramp for `pct` ∈ [0,100].
pub fn cpu_load_color(pct: f32) -> Color32 {
    const RAMP: &[(f32, u8, u8, u8)] = &[
        (  0.0, 0x27, 0xae, 0x60),  // Breeze green  (#27ae60)
        ( 50.0, 0x8b, 0xc3, 0x4a),  // mid green-yellow
        ( 70.0, 0xf6, 0x74, 0x00),  // Breeze orange (#f67400)
        ( 85.0, 0xda, 0x44, 0x53),  // Breeze red    (#da4453)
        (100.0, 0xb0, 0x20, 0x2e),  // deep red
    ];

    let pct = pct.clamp(0.0, 100.0);
    for i in 0..RAMP.len() - 1 {
        let (p0, r0, g0, b0) = RAMP[i];
        let (p1, r1, g1, b1) = RAMP[i + 1];
        if pct <= p1 {
            let t = if (p1 - p0).abs() > f32::EPSILON { (pct - p0) / (p1 - p0) } else { 0.0 };
            let lerp = |a: u8, b: u8| -> u8 { (a as f32 + t * (b as f32 - a as f32)).round() as u8 };
            return Color32::from_rgb(lerp(r0, r1), lerp(g0, g1), lerp(b0, b1));
        }
    }
    let (_, r, g, b) = *RAMP.last().unwrap();
    Color32::from_rgb(r, g, b)
}

// ── Row highlight colours for the process table ───────────────────────────────

/// Text colour for a process row given its CPU load and throttle state.
/// `text_color` should be `ui.visuals().text_color()` so it adapts to Breeze Dark/Light.
pub fn row_color(cpu_pct: f32, throttled: bool, text_color: Color32) -> Color32 {
    if throttled            { Breeze::WARNING }
    else if cpu_pct >= 80.0 { Breeze::NEGATIVE }
    else if cpu_pct >= 40.0 { Color32::from_rgb(0xfd, 0xbc, 0x4b) }  // Breeze yellow
    else if cpu_pct >= 10.0 { Breeze::POSITIVE }
    else                    { text_color }
}

// ── Theme application ─────────────────────────────────────────────────────────

pub fn apply(ctx: &Context, native_ppp: f32) {
    // Ensure the rendering scale matches the display's native DPI so fonts
    // don't shrink when the theme is reapplied (e.g. after toggling system theme).
    ctx.set_pixels_per_point(native_ppp);
    let mut style = Style::default();

    let mut vis = Visuals::dark();

    // ── Backgrounds ──────────────────────────────────────────────────────
    vis.window_fill      = Breeze::WINDOW_BG;
    vis.panel_fill       = Breeze::WINDOW_BG;
    vis.faint_bg_color   = Breeze::ALT_BASE;
    vis.extreme_bg_color = Breeze::BASE;

    // ── Text ─────────────────────────────────────────────────────────────
    vis.override_text_color = Some(Breeze::TEXT);

    // ── Widgets ──────────────────────────────────────────────────────────
    let rounding = CornerRadius::same(4);

    // non-interactive (labels, separators)
    vis.widgets.noninteractive.bg_fill    = Breeze::WINDOW_BG;
    vis.widgets.noninteractive.bg_stroke  = Stroke::new(1.0, Breeze::BORDER);
    vis.widgets.noninteractive.fg_stroke  = Stroke::new(1.0, Breeze::BORDER);
    vis.widgets.noninteractive.corner_radius = rounding;

    // inactive (buttons, checkboxes at rest)
    vis.widgets.inactive.bg_fill          = Breeze::BUTTON_BG;
    vis.widgets.inactive.bg_stroke        = Stroke::new(1.0, Breeze::BORDER);
    vis.widgets.inactive.fg_stroke        = Stroke::new(1.0, Breeze::TEXT);
    vis.widgets.inactive.corner_radius     = rounding;

    // hovered
    vis.widgets.hovered.bg_fill           = Breeze::BUTTON_HOVER;
    vis.widgets.hovered.bg_stroke         = Stroke::new(1.0, Breeze::HIGHLIGHT);
    vis.widgets.hovered.fg_stroke         = Stroke::new(1.0, Breeze::TEXT);
    vis.widgets.hovered.corner_radius      = rounding;

    // active (pressed)
    vis.widgets.active.bg_fill            = Breeze::HIGHLIGHT;
    vis.widgets.active.bg_stroke          = Stroke::new(1.0, Breeze::HIGHLIGHT);
    vis.widgets.active.fg_stroke          = Stroke::new(1.5, Breeze::TEXT);
    vis.widgets.active.corner_radius       = rounding;

    // open (combo boxes, menus)
    vis.widgets.open.bg_fill              = Breeze::ALT_BASE;
    vis.widgets.open.bg_stroke            = Stroke::new(1.0, Breeze::HIGHLIGHT);
    vis.widgets.open.fg_stroke            = Stroke::new(1.0, Breeze::TEXT);
    vis.widgets.open.corner_radius         = rounding;

    // ── Selection ────────────────────────────────────────────────────────
    vis.selection.bg_fill = Color32::from_rgba_unmultiplied(0x3d, 0xae, 0xe9, 0x66); // 40% alpha
    vis.selection.stroke  = Stroke::new(1.0, Breeze::HIGHLIGHT);

    // ── Misc ─────────────────────────────────────────────────────────────
    vis.hyperlink_color  = Breeze::LINK;
    vis.window_stroke    = Stroke::new(1.0, Breeze::BORDER);
    vis.window_shadow    = egui::epaint::Shadow::NONE;
    vis.window_corner_radius = CornerRadius::same(4);

    // Striped table alternate row colour
    vis.faint_bg_color   = Breeze::ALT_BASE;

    style.visuals = vis;

    // ── Typography ───────────────────────────────────────────────────────
    style.text_styles = {
        use egui::TextStyle::*;
        [
            (Small,     FontId::proportional(12.0)),
            (Body,      FontId::proportional(14.0)),
            (Button,    FontId::proportional(14.0)),
            (Heading,   FontId::proportional(16.0)),
            (Monospace, FontId::monospace(13.0)),
        ]
        .into()
    };

    // ── Spacing — comfortable row height ─────────────────────────────────
    style.spacing.interact_size.y = 24.0;
    style.spacing.item_spacing    = egui::vec2(8.0, 4.0);

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
    let base      = Color32::from_rgb(0xff, 0xff, 0xff); // #ffffff  table row even
    let alt_base  = Color32::from_rgb(0xf4, 0xf4, 0xf4); // #f4f4f4  table row odd
    let tab_bar   = Color32::from_rgb(0xd5, 0xd9, 0xde); // #d5d9de  tab bar / panel bg

    vis.window_fill      = window_bg;
    vis.panel_fill       = tab_bar;
    vis.faint_bg_color   = alt_base;
    vis.extreme_bg_color = base;

    // Text
    let text = Color32::from_rgb(0x31, 0x36, 0x3b); // #31363b
    vis.override_text_color = Some(text);

    // Borders / buttons
    let border      = Color32::from_rgb(0xb0, 0xb8, 0xc0); // #b0b8c0 — sharper than before
    let button_bg   = Color32::from_rgb(0xef, 0xf0, 0xf1);
    let button_hov  = Color32::from_rgb(0xe0, 0xe4, 0xe8);
    let button_dis  = Color32::from_rgb(0xdd, 0xe1, 0xe5); // #dde1e5 disabled bg
    let text_dis    = Color32::from_rgb(0xa0, 0xa8, 0xb0); // #a0a8b0 disabled fg
    let highlight   = Color32::from_rgb(0x3d, 0xae, 0xe9); // #3daee9 Breeze blue

    let rounding = CornerRadius::same(4);

    vis.widgets.noninteractive.bg_fill        = window_bg;
    vis.widgets.noninteractive.bg_stroke      = Stroke::new(1.0, border);
    vis.widgets.noninteractive.fg_stroke      = Stroke::new(1.0, border);
    vis.widgets.noninteractive.corner_radius  = rounding;

    vis.widgets.inactive.bg_fill              = button_bg;
    vis.widgets.inactive.bg_stroke            = Stroke::new(1.0, border);
    vis.widgets.inactive.fg_stroke            = Stroke::new(1.0, text);
    vis.widgets.inactive.corner_radius        = rounding;

    vis.widgets.hovered.bg_fill               = button_hov;
    vis.widgets.hovered.bg_stroke             = Stroke::new(1.0, highlight);
    vis.widgets.hovered.fg_stroke             = Stroke::new(1.0, text);
    vis.widgets.hovered.corner_radius         = rounding;

    // Active (pressed) — Breeze blue fill, white text
    vis.widgets.active.bg_fill                = highlight;
    vis.widgets.active.bg_stroke              = Stroke::new(1.0, highlight);
    vis.widgets.active.fg_stroke              = Stroke::new(1.5, Color32::WHITE);
    vis.widgets.active.corner_radius          = rounding;

    // Open (combo / menu dropdown) — slightly darker base
    vis.widgets.open.bg_fill                  = button_hov;
    vis.widgets.open.bg_stroke                = Stroke::new(1.0, highlight);
    vis.widgets.open.fg_stroke                = Stroke::new(1.0, text);
    vis.widgets.open.corner_radius            = rounding;

    // Selection — Breeze blue at ~31% alpha (premultiplied: 61,174,233 * 80/255 ≈ 19,55,73)
    vis.selection.bg_fill = Color32::from_rgba_premultiplied(19, 55, 73, 80);
    vis.selection.stroke  = Stroke::new(1.0, highlight);

    vis.hyperlink_color      = Color32::from_rgb(0x29, 0x80, 0xb9);
    vis.window_stroke        = Stroke::new(1.0, border);
    vis.window_shadow        = egui::epaint::Shadow::NONE;
    vis.window_corner_radius = CornerRadius::same(4);

    style.visuals = vis;

    style.text_styles = {
        use egui::TextStyle::*;
        [
            (Small,     FontId::proportional(12.0)),
            (Body,      FontId::proportional(14.0)),
            (Button,    FontId::proportional(14.0)),
            (Heading,   FontId::proportional(16.0)),
            (Monospace, FontId::monospace(13.0)),
        ]
        .into()
    };

    style.spacing.interact_size.y = 24.0;
    style.spacing.item_spacing    = egui::vec2(8.0, 4.0);
    // Striped table alternate row — explicitly set after visuals assignment
    style.visuals.faint_bg_color  = alt_base;

    ctx.set_style(style);

    let _ = (button_dis, text_dis); // available for disabled-widget callers
}
