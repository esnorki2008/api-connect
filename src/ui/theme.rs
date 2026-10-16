//! Paletas Atom One Dark y One Light. Los colores se piden en cada cuadro, así el cambio de tema es inmediato.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use eframe::egui::Color32;

static LIGHT: AtomicBool = AtomicBool::new(false);

pub fn set_light(light: bool) {
    LIGHT.store(light, Relaxed);
}

pub fn is_light() -> bool {
    LIGHT.load(Relaxed)
}

const fn rgb(h: u32) -> Color32 {
    Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

macro_rules! palette {
    ($($(#[$doc:meta])* $name:ident = $dark:literal, $light:literal;)*) => {
        $( $(#[$doc])* pub fn $name() -> Color32 { rgb(if is_light() { $light } else { $dark }) } )*
    };
}

// En el tema claro los acentos se oscurecen respecto de One Light para que el texto de color se lea sobre blanco.
palette! {
    ok = 0x98c379, 0x3b8a3a;
    err = 0xe06c75, 0xc0392b;
    warn = 0xe5c07b, 0x8a6100;
    accent = 0x61afef, 0x2f6fdf;
    purple = 0xc678dd, 0x9a2a98;
    cyan = 0x56b6c2, 0x0a7aa8;
    /// Texto secundario.
    muted = 0x9aa1ad, 0x62656f;
    /// Texto sobre botones de acento.
    on_accent = 0x1b1d23, 0xffffff;
    /// Barras superior e inferior, campos de texto.
    deep = 0x1b1d23, 0xe6e7ea;
    /// Área central.
    surface = 0x282c34, 0xfafafa;
    /// Paneles laterales.
    panel = 0x21252b, 0xf0f1f3;
    faint = 0x2c313a, 0xe9eaed;
    line = 0x3a3f4b, 0xd3d5da;
    text = 0xc8cdd5, 0x383a42;
    bright = 0xeef0f3, 0x15171b;
    field = 0x1b1d23, 0xffffff;
    selection = 0x2f4a6d, 0xd3e2fb;
    hover = 0x3a404b, 0xdfe2e7;
    active = 0x454c59, 0xcfd4dc;
    // Sintaxis
    syn_key = 0xe06c75, 0xd93a2b;
    syn_str = 0x98c379, 0x3b8a3a;
    syn_num = 0xd19a66, 0x9a5a00;
    syn_kw = 0xc678dd, 0x9a2a98;
    syn_fn = 0x61afef, 0x2f6fdf;
    syn_comment = 0x7f848e, 0x8a8d96;
    syn_punct = 0xabb2bf, 0x383a42;
}

/// Fondo tenue detrás de {{variables}}.
pub fn var_bg() -> Color32 {
    if is_light() { Color32::from_rgba_premultiplied(60, 45, 0, 28) } else { Color32::from_rgba_premultiplied(40, 34, 20, 90) }
}
