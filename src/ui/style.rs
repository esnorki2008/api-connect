//! Fuentes y tema visual (One Dark / One Light).

use super::*;

/// Segoe UI y Consolas del sistema leen mejor que la letra delgada por defecto; las de egui quedan de respaldo
/// para símbolos (✔ ✖ ⚡ ▶).
pub(super) fn setup_fonts(ctx: &egui::Context) {
    use egui::FontFamily;
    let mut fonts = egui::FontDefinitions::default();
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    for (name, file, family) in [("segoe", "segoeui.ttf", FontFamily::Proportional), ("consolas", "consola.ttf", FontFamily::Monospace)] {
        if let Ok(bytes) = std::fs::read(Path::new(&dir).join("Fonts").join(file)) {
            fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
            fonts.families.entry(family).or_default().insert(0, name.into());
        }
    }
    ctx.set_fonts(fonts);
}

/// Tema Atom One Dark / One Light: fondos, texto, acentos de la paleta y letra más grande.
pub(super) fn apply_theme(ctx: &egui::Context, light: bool) {
    use egui::{FontId, Stroke, TextStyle};
    theme::set_light(light);
    ctx.set_theme(if light { egui::ThemePreference::Light } else { egui::ThemePreference::Dark });
    let mut style = (*ctx.style()).clone();
    style.visuals = if light { egui::Visuals::light() } else { egui::Visuals::dark() };
    style.text_styles = [
        (TextStyle::Small, FontId::proportional(12.5)),
        (TextStyle::Body, FontId::proportional(14.5)),
        (TextStyle::Button, FontId::proportional(14.5)),
        (TextStyle::Heading, FontId::proportional(19.0)),
        (TextStyle::Monospace, FontId::monospace(13.5)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    style.spacing.interact_size.y = 24.0;

    let (panel, line, text, bright) = (theme::panel(), theme::line(), theme::text(), theme::bright());
    let v = &mut style.visuals;
    (v.panel_fill, v.window_fill, v.extreme_bg_color, v.faint_bg_color, v.code_bg_color) = (panel, theme::surface(), theme::field(), theme::faint(), theme::field());
    v.window_stroke = Stroke::new(1.0_f32, line);
    v.override_text_color = None;
    (v.hyperlink_color, v.warn_fg_color, v.error_fg_color) = (theme::accent(), theme::warn(), theme::err());
    v.selection.bg_fill = theme::selection();
    v.selection.stroke = Stroke::new(1.0_f32, bright);
    v.text_cursor.stroke = Stroke::new(2.0_f32, theme::accent());
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = panel;
    w.noninteractive.weak_bg_fill = panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, line);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, text);
    for (state, bg, border, fg) in [
        (&mut w.inactive, theme::faint(), line, text),
        (&mut w.hovered, theme::hover(), theme::accent(), bright),
        (&mut w.active, theme::active(), theme::accent(), bright),
        (&mut w.open, theme::hover(), line, bright),
    ] {
        (state.bg_fill, state.weak_bg_fill) = (bg, bg);
        state.bg_stroke = Stroke::new(1.0_f32, border);
        state.fg_stroke = Stroke::new(1.5_f32, fg);
    }
    ctx.set_style(style);
}
