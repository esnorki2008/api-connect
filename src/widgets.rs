//! Piezas de UI reutilizables: resaltado de sintaxis, tablas clave/valor (como en Postman) y etiquetas.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, FontId, Galley, RichText, TextEdit, TextStyle, Ui,
    text::{LayoutJob, TextFormat},
};

// Colores de sintaxis del tema activo (Atom One Dark / One Light).
#[allow(non_snake_case)]
mod c {
    pub use crate::theme::{
        cyan as CYAN, syn_comment as COMMENT, syn_fn as FUNC, syn_key as KEY, syn_kw as KW, syn_num as NUM, syn_punct as PUNCT, syn_str as STR,
        text as TEXT, var_bg as VAR_BG, warn as VAR,
    };
}
use c::*;

/// Más allá de esto se muestra sin colores: resaltar y maquetar megas por cuadro vuelve lenta la UI.
const HIGHLIGHT_LIMIT: usize = 400_000;

const JS_KEYWORDS: [&str; 24] = [
    "const", "let", "var", "function", "return", "if", "else", "for", "while", "of", "in", "new", "await", "async", "try", "catch",
    "throw", "typeof", "break", "continue", "switch", "case", "default", "this",
];

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub enum Lang {
    Json,
    Js,
    /// Solo resalta {{variables}} (URL, valores).
    Plain,
}

impl Lang {
    /// JSON si parece JSON; si no, texto plano.
    pub fn guess(text: &str) -> Self {
        if text.trim_start().starts_with(['{', '[']) { Lang::Json } else { Lang::Plain }
    }
}

fn fmt(color: Color32, size: f32) -> TextFormat {
    TextFormat::simple(FontId::monospace(size), color)
}

/// Divide en tramos coloreados. Los {{vars}} se marcan en cualquier parte (también dentro de strings).
pub fn highlight(code: &str, lang: Lang, size: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    if code.len() > HIGHLIGHT_LIMIT {
        job.append(code, 0.0, fmt(TEXT(), size));
        return job;
    }
    let mut plain = String::new();
    let flush = |job: &mut LayoutJob, plain: &mut String| {
        if !plain.is_empty() {
            job.append(plain, 0.0, fmt(PUNCT(), size));
            plain.clear();
        }
    };
    let mut i = 0;
    while i < code.len() {
        let rest = &code[i..];
        let c = rest.chars().next().unwrap_or(' ');
        if let Some(n) = var_len(rest) {
            flush(&mut job, &mut plain);
            push_var(&mut job, &rest[..n], size);
            i += n;
            continue;
        }
        if lang == Lang::Plain {
            plain.push(c);
            i += c.len_utf8();
            continue;
        }
        if lang == Lang::Js && (rest.starts_with("//") || rest.starts_with("/*")) {
            let n = if rest.starts_with("//") { rest.find('\n').unwrap_or(rest.len()) } else { rest.find("*/").map_or(rest.len(), |e| e + 2) };
            flush(&mut job, &mut plain);
            job.append(&rest[..n], 0.0, TextFormat { italics: true, ..fmt(COMMENT(), size) });
            i += n;
            continue;
        }
        if c == '"' || (lang == Lang::Js && (c == '\'' || c == '`')) {
            let n = string_len(rest, c);
            let is_key = lang == Lang::Json && rest[n..].trim_start().starts_with(':');
            flush(&mut job, &mut plain);
            push_with_vars(&mut job, &rest[..n], if is_key { KEY() } else { STR() }, size);
            i += n;
            continue;
        }
        let prev_word = code[..i].chars().next_back().is_some_and(|p| p.is_alphanumeric() || p == '_' || p == '$');
        if !prev_word && (c.is_ascii_digit() || (c == '-' && rest[1..].starts_with(|d: char| d.is_ascii_digit()))) {
            let n = 1 + rest[1..].find(|d: char| !(d.is_ascii_alphanumeric() || d == '.')).unwrap_or(rest.len() - 1);
            flush(&mut job, &mut plain);
            job.append(&rest[..n], 0.0, fmt(NUM(), size));
            i += n;
            continue;
        }
        if !prev_word && (c.is_alphabetic() || c == '_' || c == '$') {
            let n = rest.find(|d: char| !(d.is_alphanumeric() || d == '_' || d == '$')).unwrap_or(rest.len());
            let word = &rest[..n];
            let color = match word {
                "true" | "false" | "null" | "undefined" | "NaN" => NUM(),
                "pm" | "console" | "JSON" => CYAN(),
                w if lang == Lang::Js && JS_KEYWORDS.contains(&w) => KW(),
                _ if lang == Lang::Js && rest[n..].trim_start().starts_with('(') => FUNC(),
                _ => TEXT(),
            };
            flush(&mut job, &mut plain);
            job.append(word, 0.0, fmt(color, size));
            i += n;
            continue;
        }
        plain.push(c);
        i += c.len_utf8();
    }
    flush(&mut job, &mut plain);
    job
}

/// Largo de "{{nombre}}" al inicio de `s`, si lo hay.
fn var_len(s: &str) -> Option<usize> {
    let inner = s.strip_prefix("{{")?;
    let end = inner.find("}}")?;
    (!inner[..end].contains(['{', '}', '\n']) && end > 0).then_some(end + 4)
}

fn push_var(job: &mut LayoutJob, s: &str, size: f32) {
    job.append(s, 0.0, TextFormat { background: VAR_BG(), ..fmt(VAR(), size) });
}

fn push_with_vars(job: &mut LayoutJob, s: &str, color: Color32, size: f32) {
    let mut start = 0;
    let mut i = 0;
    while i < s.len() {
        match var_len(&s[i..]) {
            Some(n) => {
                if start < i {
                    job.append(&s[start..i], 0.0, fmt(color, size));
                }
                push_var(job, &s[i..i + n], size);
                i += n;
                start = i;
            }
            None => i += s[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    if start < s.len() {
        job.append(&s[start..], 0.0, fmt(color, size));
    }
}

/// Largo de un string que empieza en `s` con la comilla `q` (incluida la de cierre; respeta escapes).
fn string_len(s: &str, q: char) -> usize {
    let mut escaped = false;
    for (i, c) in s.char_indices().skip(1) {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '\n' if q != '`' => return i,
            c if c == q => return i + 1,
            _ => {}
        }
    }
    s.len()
}

#[derive(Default)]
struct Highlighter;

impl<'a> egui::cache::ComputerMut<(&'a str, Lang, u32, bool), LayoutJob> for Highlighter {
    fn compute(&mut self, (code, lang, size, _light): (&'a str, Lang, u32, bool)) -> LayoutJob {
        highlight(code, lang, size as f32 / 10.0)
    }
}

type HighlightCache = egui::cache::FrameCache<LayoutJob, Highlighter>;

/// Layouter para TextEdit: colorea con caché (solo se recalcula cuando cambia el texto).
pub fn layouter(lang: Lang, size: f32) -> impl FnMut(&Ui, &str, f32) -> Arc<Galley> {
    move |ui, text, wrap| {
        let mut job = ui.ctx().memory_mut(|m| m.caches.cache::<HighlightCache>().get((text, lang, (size * 10.0) as u32, crate::theme::is_light())));
        job.wrap.max_width = wrap;
        ui.fonts(|f| f.layout_job(job))
    }
}

/// Para campos de una línea: resalta sin partir el texto en varias líneas.
pub fn line_layouter(lang: Lang, size: f32) -> impl FnMut(&Ui, &str, f32) -> Arc<Galley> {
    let mut inner = layouter(lang, size);
    move |ui, text, _| inner(ui, text, f32::INFINITY)
}

/// Editor de código con resaltado, ocupando todo el ancho.
pub fn code_editor(ui: &mut Ui, text: &mut String, lang: Lang, hint: &str, rows: usize) -> egui::Response {
    let mut lay = layouter(lang, 13.5);
    ui.add(
        TextEdit::multiline(text)
            .font(TextStyle::Monospace)
            .code_editor()
            .desired_rows(rows)
            .desired_width(f32::INFINITY)
            .lock_focus(true)
            .hint_text(hint)
            .layouter(&mut lay),
    )
}

/// Visor de solo lectura con resaltado (se puede seleccionar y copiar).
pub fn code_viewer(ui: &mut Ui, text: &str, lang: Lang) {
    let mut lay = layouter(lang, 13.5);
    let mut t = text;
    ui.add(TextEdit::multiline(&mut t).font(TextStyle::Monospace).code_editor().desired_width(f32::INFINITY).layouter(&mut lay));
}

// ---- Tablas clave/valor ----

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub on: bool,
    pub key: String,
    pub value: String,
}

impl Row {
    fn empty() -> Self {
        Row { on: true, key: String::new(), value: String::new() }
    }
}

/// Tabla estilo Postman; siempre deja una fila vacía al final para agregar. Devuelve true si cambió algo.
pub fn kv_table(ui: &mut Ui, id: &str, rows: &mut Vec<Row>, toggles: bool, hints: (&str, &str)) -> bool {
    if rows.last().is_none_or(|r| !r.key.is_empty() || !r.value.is_empty()) {
        rows.push(Row::empty());
    }
    let (mut changed, mut remove) = (false, None);
    // Anchos fijos por fila (no Grid): el Grid mide en el cuadro anterior y el primero sale angosto.
    let width = ui.available_width();
    let check_w = if toggles { 24.0 } else { 0.0 };
    let key_w = (width * 0.34).clamp(110.0, 320.0);
    let val_w = (width - key_w - check_w - 60.0).max(120.0);
    let n = rows.len();
    ui.push_id(id, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(check_w + 2.0);
            let head = |ui: &mut Ui, w: f32, t: &str| {
                ui.allocate_ui_with_layout(egui::vec2(w, 16.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_width(w);
                    ui.label(RichText::new(t).small().color(COMMENT()));
                });
            };
            head(ui, key_w, "CLAVE");
            head(ui, val_w, "VALOR");
        });
        for (i, r) in rows.iter_mut().enumerate() {
            let last = i + 1 == n;
            ui.horizontal(|ui| {
                if toggles {
                    if last {
                        ui.add_space(check_w);
                    } else {
                        changed |= ui.add_sized([check_w - 4.0, 22.0], egui::Checkbox::without_text(&mut r.on)).on_hover_text("Incluir en el envío").changed();
                    }
                }
                let dim = !r.on && !last;
                let key = TextEdit::singleline(&mut r.key).font(TextStyle::Monospace).hint_text(if last { hints.0 } else { "" });
                changed |= ui.add(if dim { key.text_color(COMMENT()) } else { key }.desired_width(key_w)).changed();
                let mut lay = line_layouter(Lang::Plain, 13.5);
                let val = TextEdit::singleline(&mut r.value).font(TextStyle::Monospace).hint_text(if last { hints.1 } else { "" }).layouter(&mut lay);
                changed |= ui.add(val.desired_width(val_w)).changed();
                if !last && ui.small_button("×").on_hover_text("Quitar").clicked() {
                    remove = Some(i);
                }
            });
        }
    });
    if let Some(i) = remove {
        rows.remove(i);
        changed = true;
    }
    changed
}

/// "Clave: valor" por línea; "// Clave: valor" = desactivado (no se envía).
pub fn headers_to_rows(text: &str) -> Vec<Row> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (on, l) = match l.trim_start().strip_prefix("//") {
                Some(rest) => (false, rest.trim_start()),
                None => (true, l),
            };
            let (k, v) = l.split_once(':').unwrap_or((l, ""));
            Row { on, key: k.trim().into(), value: v.trim().into() }
        })
        .collect()
}

pub fn rows_to_headers(rows: &[Row]) -> String {
    rows.iter()
        .filter(|r| !r.key.is_empty() || !r.value.is_empty())
        .map(|r| format!("{}{}: {}", if r.on { "" } else { "// " }, r.key, r.value))
        .collect::<Vec<_>>()
        .join("\n")
}

/// URL -> (base, parámetros de query) sin decodificar, para no tocar los {{vars}}.
pub fn url_to_rows(url: &str) -> (String, Vec<Row>) {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let rows = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            Row { on: true, key: k.into(), value: v.into() }
        })
        .collect();
    (base.into(), rows)
}

pub fn rows_to_url(base: &str, rows: &[Row]) -> String {
    let q: Vec<String> = rows
        .iter()
        .filter(|r| !r.key.is_empty() || !r.value.is_empty())
        .map(|r| if r.value.is_empty() { r.key.clone() } else { format!("{}={}", r.key, r.value) })
        .collect();
    if q.is_empty() { base.into() } else { format!("{base}?{}", q.join("&")) }
}

// ---- Etiquetas ----

/// Etiqueta redondeada con fondo tenue del color dado.
pub fn pill(ui: &mut Ui, text: impl Into<String>, color: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| ui.label(RichText::new(text.into()).color(color).strong().small()))
        .response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(job: &LayoutJob) -> Vec<(String, Color32)> {
        job.sections.iter().map(|s| (job.text[s.byte_range.clone()].to_string(), s.format.color)).collect()
    }

    #[test]
    fn highlights_json_js_and_vars() {
        let j = spans(&highlight(r#"{"id": 7, "ok": true, "t": "Bearer {{tok}}"}"#, Lang::Json, 13.0));
        assert!(j.contains(&("\"id\"".into(), KEY())) && j.contains(&("7".into(), NUM())) && j.contains(&("true".into(), NUM())));
        assert!(j.contains(&("\"Bearer ".into(), STR())) && j.contains(&("{{tok}}".into(), VAR())));
        let s = spans(&highlight("const x = pm.response.json(); // fin\nlet ñ = 'a\\'b';", Lang::Js, 13.0));
        assert!(s.contains(&("const".into(), KW())) && s.contains(&("pm".into(), CYAN())) && s.contains(&("json".into(), FUNC())));
        assert!(s.contains(&("// fin".into(), COMMENT())) && s.contains(&("'a\\'b'".into(), STR())));
        let all: String = s.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(all, "const x = pm.response.json(); // fin\nlet ñ = 'a\\'b';", "no se pierde ni duplica texto");
        assert!(spans(&highlight("{{base}}/u/{{ id }}", Lang::Plain, 13.0)).iter().filter(|x| x.1 == VAR()).count() == 2);
    }

    #[test]
    fn kv_conversions_roundtrip() {
        let rows = headers_to_rows("Accept: */*\n// X-Off: 1\nAuthorization: Bearer {{t}}");
        assert_eq!((rows.len(), rows[1].on, rows[1].key.as_str()), (3, false, "X-Off"));
        assert_eq!(rows_to_headers(&rows), "Accept: */*\n// X-Off: 1\nAuthorization: Bearer {{t}}");
        let (base, q) = url_to_rows("{{base}}/x?a=1&b={{v}}&flag");
        assert_eq!((base.as_str(), q.len(), q[1].value.as_str()), ("{{base}}/x", 3, "{{v}}"));
        assert_eq!(rows_to_url(&base, &q), "{{base}}/x?a=1&b={{v}}&flag");
        assert_eq!(rows_to_url("h", &[]), "h");
    }
}
