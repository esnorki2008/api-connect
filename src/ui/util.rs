//! Utilidades de presentación (colores, recortes, tamaños, tiempos).

use super::*;

/// Colores por método HTTP, como en la mayoría de clientes de APIs.
pub(super) fn method_color(m: &str) -> Color32 {
    match m.to_uppercase().as_str() {
        "GET" => theme::ok(),
        "POST" => theme::warn(),
        "PUT" => theme::accent(),
        "PATCH" => theme::purple(),
        "DELETE" => theme::err(),
        _ => theme::cyan(),
    }
}

/// Cambios sin guardar: distinto del guardado, o un request nuevo con algo escrito.
pub(super) fn is_dirty(cols: &[Collection], cur: Option<(usize, usize)>, ed: &Request) -> bool {
    match cur {
        Some((c, i)) => cols.get(c).and_then(|c| c.requests.get(i)) != Some(ed),
        None => !ed.url.trim().is_empty() || !ed.body.trim().is_empty(),
    }
}

pub(super) fn vars_rows(v: Option<&Vars>) -> Vec<Row> {
    v.map(|v| v.iter().map(|(k, v)| Row { on: true, key: k.clone(), value: v.clone() }).collect()).unwrap_or_default()
}

pub(super) fn rows_vars(rows: &[Row]) -> Vars {
    rows.iter().filter(|r| !r.key.trim().is_empty()).map(|r| (r.key.trim().to_string(), r.value.clone())).collect()
}

/// Fila de derecha a izquierda del alto de un control (with_layout sola ocuparía todo el alto disponible).
pub(super) fn row_rtl(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y + 4.0);
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), add);
}

pub(super) fn status_color(s: u16) -> Color32 {
    match s {
        ..=299 => theme::ok(),
        300..=399 => theme::warn(),
        _ => theme::err(),
    }
}

/// DELETE y OPTIONS no caben en la columna de métodos.
pub(super) fn short_method(m: &str) -> String {
    match m.to_uppercase().as_str() {
        "DELETE" => "DEL".into(),
        "OPTIONS" => "OPT".into(),
        other => other.into(),
    }
}

pub(super) fn human_size(n: usize) -> String {
    match n {
        ..1024 => format!("{n} B"),
        ..1_048_576 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

pub(super) fn ago(t: Instant) -> String {
    match t.elapsed().as_secs() {
        s @ ..60 => tr!("hace {s} s", "{s} s ago"),
        s @ ..3600 => tr!("hace {} min", "{} min ago", s / 60),
        s => tr!("hace {} h", "{} h ago", s / 3600),
    }
}

/// Primera línea, recortada a `n` caracteres.
pub(super) fn cut_line(s: &str, n: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() <= n { line.into() } else { line.chars().take(n - 1).chain(['…']).collect() }
}

pub(super) fn floor_boundary(s: &str, max: usize) -> usize {
    (0..=max.min(s.len())).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0)
}

/// Recorta a `n` caracteres respetando UTF-8.
pub(super) fn cut(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => tr!("{}\n…(recortado)", "{}\n…(truncated)", &s[..i]),
        None => s.to_string(),
    }
}

pub(super) fn source_of(scan: &repo::Scan, file: &str) -> Source {
    Source { repo: scan.url.clone(), branch: scan.branch.clone(), file: file.into(), commit: scan.commit.clone() }
}
