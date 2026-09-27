//! Themeing: multiple palettes, install/switch at runtime.
//!
//! Palette values are read at call sites via `palette()`, which returns the
//! currently installed palette. Change theme with `install(ctx, choice)` — the
//! egui `Visuals` and the global palette are updated in sync, so anything that
//! already read the previous palette repaints against the new one on the next
//! frame.

use std::sync::{OnceLock, RwLock};

use eframe::egui::{
    self, epaint, style::WidgetVisuals, Color32, CornerRadius, FontDefinitions, FontFamily,
    Margin, Stroke, Vec2, Visuals,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThemeChoice {
    /// Cool cinema teal on deep charcoal. Default.
    DarkTeal,
    /// Warm amber on deep charcoal — Dragonframe-esque.
    DarkAmber,
    /// Bright paper background for daytime editing.
    Light,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::DarkTeal, ThemeChoice::DarkAmber, ThemeChoice::Light];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::DarkTeal => "Dark · Teal",
            ThemeChoice::DarkAmber => "Dark · Amber",
            ThemeChoice::Light => "Light",
        }
    }
}

/// Everything the app needs to paint bespoke shapes: colours the egui `Visuals`
/// doesn't cover (viewer overlays, PIP borders, timeline playhead, kbd chips).
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg_deepest: Color32,
    pub bg_app: Color32,
    pub bg_panel: Color32,
    pub bg_elevated: Color32,
    pub bg_hover: Color32,

    pub text_primary: Color32,
    pub text_muted: Color32,
    pub text_dim: Color32,

    pub accent: Color32,
    pub accent_deep: Color32,
    pub accent_muted: Color32,
    /// Text placed directly on top of `accent` / `accent_deep`.
    pub on_accent: Color32,

    pub ok: Color32,
    pub warn: Color32,
    pub error: Color32,
    pub live: Color32,

    pub border: Color32,
    pub border_subtle: Color32,
}

const DARK_TEAL: Palette = Palette {
    bg_deepest: Color32::from_rgb(14, 15, 18),
    bg_app: Color32::from_rgb(20, 22, 26),
    bg_panel: Color32::from_rgb(26, 29, 34),
    bg_elevated: Color32::from_rgb(34, 38, 44),
    bg_hover: Color32::from_rgb(42, 47, 54),
    text_primary: Color32::from_rgb(230, 232, 236),
    text_muted: Color32::from_rgb(150, 155, 165),
    text_dim: Color32::from_rgb(100, 105, 115),
    accent: Color32::from_rgb(72, 190, 205),
    accent_deep: Color32::from_rgb(36, 132, 152),
    accent_muted: Color32::from_rgb(40, 84, 96),
    on_accent: Color32::from_rgb(8, 20, 26),
    ok: Color32::from_rgb(90, 190, 130),
    warn: Color32::from_rgb(230, 180, 70),
    error: Color32::from_rgb(228, 100, 90),
    live: Color32::from_rgb(224, 92, 92),
    border: Color32::from_rgb(48, 52, 58),
    border_subtle: Color32::from_rgb(38, 42, 48),
};

const DARK_AMBER: Palette = Palette {
    accent: Color32::from_rgb(232, 176, 74),
    accent_deep: Color32::from_rgb(196, 138, 48),
    accent_muted: Color32::from_rgb(120, 92, 42),
    on_accent: Color32::from_rgb(20, 14, 4),
    ..DARK_TEAL
};

const LIGHT: Palette = Palette {
    bg_deepest: Color32::from_rgb(220, 222, 226),
    bg_app: Color32::from_rgb(238, 240, 244),
    bg_panel: Color32::from_rgb(248, 249, 251),
    bg_elevated: Color32::from_rgb(255, 255, 255),
    bg_hover: Color32::from_rgb(224, 232, 240),
    text_primary: Color32::from_rgb(24, 26, 30),
    text_muted: Color32::from_rgb(88, 94, 105),
    text_dim: Color32::from_rgb(140, 145, 155),
    accent: Color32::from_rgb(22, 118, 138),
    accent_deep: Color32::from_rgb(16, 96, 116),
    accent_muted: Color32::from_rgb(180, 216, 224),
    on_accent: Color32::from_rgb(248, 253, 255),
    ok: Color32::from_rgb(30, 140, 90),
    warn: Color32::from_rgb(190, 130, 30),
    error: Color32::from_rgb(200, 60, 55),
    live: Color32::from_rgb(200, 60, 55),
    border: Color32::from_rgb(200, 204, 210),
    border_subtle: Color32::from_rgb(220, 224, 230),
};

fn slot() -> &'static RwLock<Palette> {
    static SLOT: OnceLock<RwLock<Palette>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(DARK_TEAL))
}

/// Current palette. Cheap: `Palette` is Copy and the RwLock read is uncontended.
pub fn palette() -> Palette {
    *slot().read().expect("theme palette poisoned")
}

/// Install the given theme (fonts, Visuals, palette). Safe to call at startup
/// and every time the user picks a new theme.
pub fn install(ctx: &egui::Context, choice: ThemeChoice) {
    let mut fonts = FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);

    let p = match choice {
        ThemeChoice::DarkTeal => DARK_TEAL,
        ThemeChoice::DarkAmber => DARK_AMBER,
        ThemeChoice::Light => LIGHT,
    };
    *slot().write().expect("theme palette poisoned") = p;

    let is_dark = !matches!(choice, ThemeChoice::Light);
    ctx.all_styles_mut(|style| {
        style.visuals = build_visuals(&p, is_dark);
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 6.0);
        style.spacing.window_margin = Margin::same(12);
        style.spacing.menu_margin = Margin::same(6);
        style.spacing.indent = 18.0;
        style.spacing.icon_width = 16.0;
        style.spacing.icon_width_inner = 10.0;
        style.spacing.icon_spacing = 6.0;
        style.spacing.scroll.bar_width = 8.0;

        use egui::{FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(13.5, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(17.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
        ]
        .into();
    });
}

fn build_visuals(p: &Palette, dark: bool) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    let cr = CornerRadius::same(6);
    let border = Stroke::new(1.0, p.border_subtle);

    // No global override: it beat the selection colour, so a selected toggle in the light
    // theme had dark text on dark teal. Labels get text_primary via `noninteractive` instead.
    v.override_text_color = None;
    v.hyperlink_color = p.accent;
    v.faint_bg_color = p.bg_panel;
    v.extreme_bg_color = p.bg_deepest;
    v.code_bg_color = p.bg_elevated;
    v.window_fill = p.bg_panel;
    v.window_stroke = border;
    v.window_corner_radius = CornerRadius::same(10);
    v.window_shadow = epaint::Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: if dark { Color32::from_black_alpha(96) } else { Color32::from_black_alpha(32) },
    };
    v.panel_fill = p.bg_app;
    v.menu_corner_radius = CornerRadius::same(8);

    v.widgets.noninteractive = WidgetVisuals {
        bg_fill: p.bg_app,
        weak_bg_fill: p.bg_app,
        bg_stroke: border,
        corner_radius: cr,
        fg_stroke: Stroke::new(1.0, p.text_primary),
        expansion: 0.0,
    };
    v.widgets.inactive = WidgetVisuals {
        bg_fill: p.bg_elevated,
        weak_bg_fill: p.bg_panel,
        bg_stroke: Stroke::new(1.0, p.border),
        corner_radius: cr,
        fg_stroke: Stroke::new(1.0, p.text_primary),
        expansion: 0.0,
    };
    v.widgets.hovered = WidgetVisuals {
        bg_fill: p.bg_hover,
        weak_bg_fill: p.bg_hover,
        bg_stroke: Stroke::new(1.0, p.accent_muted),
        corner_radius: cr,
        fg_stroke: Stroke::new(1.0, p.text_primary),
        expansion: 1.0,
    };
    // egui also uses the active text colour for every `.strong()` label, so it must read on
    // the window background. It used to be `on_accent` (dark), which made bold headings
    // invisible in the dark themes. `accent_muted` keeps pressed buttons readable with it.
    v.widgets.active = WidgetVisuals {
        bg_fill: p.accent_muted,
        weak_bg_fill: p.accent_muted,
        bg_stroke: Stroke::new(1.0, p.accent),
        corner_radius: cr,
        fg_stroke: Stroke::new(1.0, p.text_primary),
        expansion: 1.0,
    };
    v.widgets.open = v.widgets.hovered;

    v.selection.bg_fill = p.accent_deep;
    // Text on the selection (selected scene, the Capture/Preview toggle) must read on
    // `accent_deep` in every theme; the accent itself was near-invisible on it.
    v.selection.stroke = Stroke::new(1.0, readable_on(p.accent_deep));

    v
}

/// Near-white or near-black, whichever contrasts more with `bg` (WCAG relative luminance).
pub fn readable_on(bg: Color32) -> Color32 {
    let lin = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let l = 0.2126 * lin(bg.r()) + 0.7152 * lin(bg.g()) + 0.0722 * lin(bg.b());
    // Contrast against white (L=1) vs black (L=0); pick the larger.
    if (1.05 / (l + 0.05)) >= ((l + 0.05) / 0.05) { Color32::from_gray(250) } else { Color32::from_gray(15) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: Color32, b: Color32) -> f32 {
        let lum = |c: Color32| {
            let lin = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// Every theme: body text, bold text and selected text must be readable (WCAG AA 4.5:1
    /// for text, 3:1 for the large selected labels).
    #[test]
    fn all_themes_have_readable_text() {
        for (name, p, dark) in [("teal", DARK_TEAL, true), ("amber", DARK_AMBER, true), ("light", LIGHT, false)] {
            let v = build_visuals(&p, dark);
            let on_window = v.window_fill;
            assert!(contrast(v.text_color(), on_window) >= 4.5, "{name}: body text");
            assert!(contrast(v.strong_text_color(), on_window) >= 4.5, "{name}: bold text on windows");
            assert!(contrast(v.strong_text_color(), v.panel_fill) >= 4.5, "{name}: bold text on panels");
            assert!(contrast(v.selection.stroke.color, v.selection.bg_fill) >= 3.0, "{name}: selected text");
            assert!(v.override_text_color.is_none(), "{name}: an override would beat the selection colour");
            assert!(contrast(v.weak_text_color(), v.panel_fill) >= 3.0, "{name}: weak text");
            assert!(contrast(v.widgets.active.text_color(), v.widgets.active.bg_fill) >= 3.0, "{name}: pressed button");
            assert!(contrast(p.on_accent, p.accent) >= 4.5, "{name}: text on accent buttons");
            assert!(contrast(p.text_muted, p.bg_panel) >= 4.5, "{name}: muted text");
            for (what, c) in [("ok", p.ok), ("warn", p.warn), ("error", p.error)] {
                assert!(contrast(c, p.bg_panel) >= 3.0, "{name}: {what} colour");
            }
        }
    }
}
