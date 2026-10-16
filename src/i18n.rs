//! Textos en español (por defecto) e inglés. Cada texto se escribe en los dos idiomas donde se usa:
//! `t("Guardar", "Save")` o `tr!("{n} colecciones", "{n} collections")`. Sin claves ni archivos aparte, el
//! compilador obliga a que exista la traducción. ponytail: con más idiomas convendría Fluent o rust-i18n.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use serde::{Deserialize, Serialize};

static EN: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Es,
    En,
}

impl Language {
    /// "es" / "en" (o "english", "español", "EN"...).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().get(..2)? {
            "es" => Some(Language::Es),
            "en" => Some(Language::En),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Language::Es => "Español",
            Language::En => "English",
        }
    }
}

pub fn set(l: Language) {
    EN.store(l == Language::En, Relaxed);
}

pub fn current() -> Language {
    if en() { Language::En } else { Language::Es }
}

pub fn en() -> bool {
    EN.load(Relaxed)
}

/// Texto fijo en el idioma actual.
pub fn t(es: &'static str, en: &'static str) -> &'static str {
    if self::en() { en } else { es }
}

/// `API_CONNECT_LANG=en` (o `es`) fuerza el idioma, p. ej. en CI.
pub fn from_env() -> Option<Language> {
    std::env::var("API_CONNECT_LANG").ok().as_deref().and_then(Language::parse)
}

/// Texto con formato en el idioma actual: `tr!("{n} colecciones", "{n} collections")`.
#[macro_export]
macro_rules! tr {
    ($es:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if $crate::i18n::en() { format!($en $(, $arg)*) } else { format!($es $(, $arg)*) }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_languages() {
        assert_eq!((Language::parse("EN"), Language::parse("español"), Language::parse("fr")), (Some(Language::En), Some(Language::Es), None));
        assert_eq!(Language::default(), Language::Es);
    }
}
