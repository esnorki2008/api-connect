//! Lectura y escritura de los JSON de la app, y carpeta donde viven.

use std::path::{Path, PathBuf};

use serde::{Serialize, de::DeserializeOwned};

pub const COLLECTIONS: &str = "collections.json";
pub const ENVIRONMENTS: &str = "environments.json";
pub const AGENTS: &str = "agents.json";
pub const SETTINGS: &str = "settings.json";

/// Ok(None) si el archivo no existe. Cualquier otro error se reporta para no sobrescribirlo al guardar.
pub fn load<T: DeserializeOwned>(path: impl AsRef<Path>) -> Result<Option<T>, String> {
    let path = path.as_ref();
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).map(Some).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Escribe a temporal y renombra: un corte a mitad no corrompe el archivo.
pub fn save<T: Serialize>(path: impl AsRef<Path>, value: &T) -> Result<(), String> {
    let path = path.as_ref();
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let json = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Carpeta de datos. La actual si ya tiene datos de la app (modo portable, p. ej. junto al ejecutable);
/// si no, la del usuario: `%APPDATA%\api-connect` en Windows, `$XDG_DATA_HOME/api-connect` o
/// `~/.local/share/api-connect` en Linux.
pub fn data_dir() -> PathBuf {
    if [COLLECTIONS, SETTINGS].iter().any(|f| Path::new(f).exists()) {
        return PathBuf::from(".");
    }
    std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
        .map_or_else(|| PathBuf::from("."), |b| b.join("api-connect"))
}
