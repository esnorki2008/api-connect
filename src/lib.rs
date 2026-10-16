//! Núcleo compartido por la app (api-connect) y la consola (api-connect-cli).
//!
//! Capas, de adentro hacia afuera (cada una solo usa las de adentro):
//! `domain` → `engine` → `infra` / `import` → `ui`.

pub mod domain;
pub mod engine;
pub mod i18n;
pub mod import;
pub mod infra;
pub mod ui;
