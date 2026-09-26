//! Custom dark theme + Phosphor icon font. Everything the UI needs to stop looking
//! like the egui defaults.

use eframe::egui::{
    self, epaint, style::WidgetVisuals, Color32, CornerRadius, FontDefinitions, FontFamily,
    Margin, Stroke, Vec2, Visuals,
};

/// Semantic palette — used both by the Visuals below and by app code that needs to
/// paint bespoke shapes (viewer overlays, mode-toggle chip, timeline playhead).
pub mod palette {
    use eframe::egui::Color32;

    // Background surfaces: darkest for the app frame, lifting slightly for panels
    // and cards so we get a subtle depth cue without hard borders.
    pub const BG_DEEPEST: Color32 = Color32::from_rgb(14, 15, 18);
    pub const BG_APP: Color32 = Color32::from_rgb(20, 22, 26);
    pub const BG_PANEL: Color32 = Color32::from_rgb(26, 29, 34);
    pub const BG_ELEVATED: Color32 = Color32::from_rgb(34, 38, 44);
    pub const BG_HOVER: Color32 = Color32::from_rgb(42, 47, 54);

    // Text.
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(230, 232, 236);
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(150, 155, 165);
    pub const TEXT_DIM: Color32 = Color32::from_rgb(100, 105, 115);

    // Brand — a warm amber that stays legible on the deep background and evokes
    // a lit projector without screaming.
    pub const ACCENT: Color32 = Color32::from_rgb(232, 176, 74);
    pub const ACCENT_DEEP: Color32 = Color32::from_rgb(196, 138, 48);
    pub const ACCENT_MUTED: Color32 = Color32::from_rgb(120, 92, 42);

    // Semantic status.
    pub const OK: Color32 = Color32::from_rgb(90, 190, 130);
    pub const WARN: Color32 = Color32::from_rgb(230, 180, 70);
    pub const ERROR: Color32 = Color32::from_rgb(228, 100, 90);
    pub const LIVE: Color32 = Color32::from_rgb(224, 92, 92);

    pub const BORDER: Color32 = Color32::from_rgb(48, 52, 58);
    pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(38, 42, 48);
}

/// Install fonts + custom Visuals. Call from eframe's setup closure.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        style.visuals = build_visuals();
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 6.0);
        style.spacing.window_margin = Margin::same(12);
        style.spacing.menu_margin = Margin::same(6);
        style.spacing.indent = 18.0;
        style.spacing.icon_width = 16.0;
        style.spacing.icon_width_inner = 10.0;
        style.spacing.icon_spacing = 6.0;
        style.spacing.scroll.bar_width = 8.0;

        // Text sizes — slightly larger than the egui defaults so the UI reads well
        // at a comfortable viewing distance for someone lit only by a monitor.
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

fn build_visuals() -> Visuals {
    let mut v = Visuals::dark();
    let p = palette::BG_APP;
    let cr = CornerRadius::same(6);
    let border = Stroke::new(1.0, palette::BORDER_SUBTLE);

    v.override_text_color = Some(palette::TEXT_PRIMARY);
    v.hyperlink_color = palette::ACCENT;
    v.faint_bg_color = palette::BG_PANEL;
    v.extreme_bg_color = palette::BG_DEEPEST;
    v.code_bg_color = palette::BG_ELEVATED;
    v.window_fill = palette::BG_PANEL;
    v.window_stroke = border;
    v.window_corner_radius = CornerRadius::same(10);
    v.window_shadow = epaint::Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(96),
    };
    v.panel_fill = palette::BG_APP;
    v.menu_corner_radius = CornerRadius::same(8);

    v.widgets.noninteractive = WidgetVisuals {
        bg_fill: p,
        weak_bg_fill: p,
        bg_stroke: border,
        corner_radius: cr,
        fg_stroke: Stroke::new(1.0, palette::TEXT_MUTED),
        expansion: 0.0,
    };
    v.widgets.inactive = WidgetVisuals {
        bg_fill: palette::BG_ELEVATED,
        weak_bg_fill: palette::BG_PANEL,
        bg_stroke: Stroke::new(1.0, palette::BORDER),
        corner_radius: cr,
        fg_stroke: Stroke::new(1.0, palette::TEXT_PRIMARY),
        expansion: 0.0,
    };
    v.widgets.hovered = WidgetVisuals {
        bg_fill: palette::BG_HOVER,
        weak_bg_fill: palette::BG_HOVER,
        bg_stroke: Stroke::new(1.0, palette::ACCENT_MUTED),
        corner_radius: cr,
        fg_stroke: Stroke::new(1.0, palette::TEXT_PRIMARY),
        expansion: 1.0,
    };
    v.widgets.active = WidgetVisuals {
        bg_fill: palette::ACCENT_DEEP,
        weak_bg_fill: palette::ACCENT_DEEP,
        bg_stroke: Stroke::new(1.0, palette::ACCENT),
        corner_radius: cr,
        fg_stroke: Stroke::new(1.0, Color32::BLACK),
        expansion: 1.0,
    };
    v.widgets.open = v.widgets.hovered;

    v.selection.bg_fill = palette::ACCENT_DEEP;
    v.selection.stroke = Stroke::new(1.0, palette::ACCENT);

    v
}
