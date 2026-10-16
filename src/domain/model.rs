//! Entidades del dominio: colecciones, requests, autorización, entornos, agentes y baterías de estrés.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use super::vars::Vars;

/// Preferencias de la aplicación.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub claude_timeout_secs: u64,
    /// Repos de Azure DevOps usados recientemente.
    pub repos: Vec<RepoCfg>,
    /// Baterías de estrés (editables en settings.json).
    pub batteries: Vec<Battery>,
    /// Pestañas abiertas al cerrar la app: (colección, request), para restaurarlas.
    pub open_tabs: Vec<(String, String)>,
    pub active_tab: usize,
    /// Tema One Light en vez de One Dark.
    pub light_theme: bool,
    /// Panel del agente abierto (por defecto sí: la app es agentic first).
    pub agent_panel: bool,
    /// Idioma de la interfaz, de la consola y de las respuestas del agente.
    pub language: crate::i18n::Language,
}

impl Default for Settings {
    fn default() -> Self {
        Self { claude_timeout_secs: 120, repos: Vec::new(), batteries: default_batteries(), open_tabs: Vec::new(), active_tab: 0, light_theme: false, agent_panel: true, language: Default::default() }
    }
}

#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepoCfg {
    pub url: String,
    #[serde(default)]
    pub branch: String,
}

/// Lo que viene del repo y se compara al sincronizar.
#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReqParts {
    pub method: String,
    pub url: String,
    pub headers: String,
    pub body: String,
}

/// Endpoint que vino de un repo. `base` es el CONTRATO en la última sincronización (solo lectura);
/// los campos del request son TU VERSIÓN. `key` lo identifica ("GET /users/{id}").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    pub key: String,
    pub base: ReqParts,
    /// Schema JSON del body según el contrato ($ref ya resueltos), para validar tu versión.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub schema: serde_json::Value,
}

/// Archivo del repo del que sale una colección.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub repo: String,
    #[serde(default)]
    pub branch: String,
    pub file: String,
    #[serde(default)]
    pub commit: String,
}

/// Autorización de un request o de una colección. Se aplica al enviar, después del pre-request.
#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Auth {
    /// En un request: usa la de su colección. En una colección: sin auth.
    #[default]
    Inherit,
    None,
    Bearer { token: String },
    Basic { user: String, pass: String },
    ApiKey { key: String, value: String, #[serde(default)] in_query: bool },
    Oauth2(OAuth2),
}

impl Auth {
    pub fn is_inherit(&self) -> bool {
        *self == Auth::Inherit
    }
}

/// OAuth 2.0. Los campos admiten {{vars}}. El token obtenido se guarda aquí para reutilizarlo hasta que venza.
#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OAuth2 {
    /// client_credentials | password | authorization_code
    pub grant: String,
    pub token_url: String,
    pub auth_url: String,
    /// Debe estar registrada en el proveedor; la app escucha ahí el regreso del navegador.
    pub callback_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub scope: String,
    pub username: String,
    pub password: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Segundos Unix; 0 = sin vencimiento conocido.
    pub expires_at: u64,
}

#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Request {
    pub name: String,
    pub method: String,
    pub url: String,
    pub headers: String,
    pub body: String,
    pub pre: String,
    pub tests: String,
    #[serde(skip_serializing_if = "Auth::is_inherit")]
    pub auth: Auth,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

impl Request {
    pub fn parts(&self) -> ReqParts {
        ReqParts { method: self.method.clone(), url: self.url.clone(), headers: self.headers.clone(), body: self.body.clone() }
    }

    pub fn set_parts(&mut self, p: ReqParts) {
        (self.method, self.url, self.headers, self.body) = (p.method, p.url, p.headers, p.body);
    }

    /// Vino del repo y se cambió localmente desde la última sincronización.
    pub fn modified(&self) -> bool {
        self.origin.as_ref().is_some_and(|o| o.base != self.parts())
    }
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Collection {
    pub name: String,
    pub variables: Vars,
    pub requests: Vec<Request>,
    #[serde(skip_serializing_if = "Auth::is_inherit")]
    pub auth: Auth,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

/// Mueve un request (arrastrar y soltar). `to_idx` es la posición de destino calculada con el request todavía en
/// su lugar. Devuelve cómo queda cada posición (para corregir las pestañas abiertas), o None si no se movió.
pub fn move_request(cols: &mut [Collection], from: (usize, usize), to_col: usize, to_idx: usize) -> Option<impl Fn((usize, usize)) -> (usize, usize) + use<>> {
    if from.1 >= cols.get(from.0)?.requests.len() || to_col >= cols.len() {
        return None;
    }
    let to_idx = if from.0 == to_col && to_idx > from.1 { to_idx - 1 } else { to_idx }.min(cols[to_col].requests.len() - usize::from(from.0 == to_col));
    if (to_col, to_idx) == from {
        return None;
    }
    let req = cols[from.0].requests.remove(from.1);
    cols[to_col].requests.insert(to_idx, req);
    Some(move |(c, i): (usize, usize)| {
        if (c, i) == from {
            return (to_col, to_idx);
        }
        let i = if c == from.0 && i > from.1 { i - 1 } else { i };
        (c, if c == to_col && i >= to_idx { i + 1 } else { i })
    })
}

/// Copia del request con la auth que realmente aplica (la propia o la heredada de la colección).
pub fn with_effective_auth(r: &Request, col: Option<&Collection>) -> Request {
    let mut r = r.clone();
    if r.auth.is_inherit() {
        r.auth = col.map(|c| c.auth.clone()).filter(|a| !a.is_inherit()).unwrap_or(Auth::None);
    }
    r
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Environments {
    pub active: String,
    pub envs: BTreeMap<String, Vars>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AgentCfg {
    pub name: String,
    pub kind: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub effort: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
}

pub fn default_agents() -> Vec<AgentCfg> {
    let a = |name: &str, kind: &str, model: &str, effort: &str, url: &str| AgentCfg {
        name: name.into(),
        kind: kind.into(),
        model: model.into(),
        effort: effort.into(),
        url: url.into(),
    };
    vec![
        a("Claude Haiku", "claude", "claude-haiku-5-5", "low", ""),
        a("Ollama local", "ollama", "llama3.2", "", "http://127.0.0.1:11434"),
    ]
}

#[derive(Clone, Default, Debug, Serialize)]
pub struct Response {
    pub status: u16,
    pub status_text: String,
    pub ms: u64,
    pub size: usize,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

// ---- Estrés ----

/// Una etapa: `vus` usuarios virtuales en paralelo durante `secs` segundos, sin pausa entre requests.
/// Con `ramp`, los usuarios suben (o bajan) en línea recta desde los de la etapa anterior (1 si es la primera).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    pub vus: u32,
    pub secs: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ramp: bool,
}

impl Stage {
    /// Usuarios activos a los `t` segundos de la etapa.
    pub fn vus_at(&self, from: u32, t: f64) -> u32 {
        if !self.ramp {
            return self.vus;
        }
        let k = (t / self.secs.max(1) as f64).clamp(0.0, 1.0);
        ((from as f64 + (self.vus as f64 - from as f64) * k).round() as u32).max(1)
    }

    pub fn label(&self) -> String {
        if self.ramp { crate::tr!("rampa→{} VUs·{} s", "ramp→{} VUs·{} s", self.vus, self.secs) } else { format!("{} VUs·{} s", self.vus, self.secs) }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Battery {
    pub name: String,
    pub description: String,
    pub stages: Vec<Stage>,
}

pub const MAX_VUS: u32 = 500;

pub fn default_batteries() -> Vec<Battery> {
    // (vus, segundos, rampa)
    let b = |name: &str, description: &str, stages: &[(u32, u64, bool)]| Battery {
        name: name.into(),
        description: description.into(),
        stages: stages.iter().map(|&(vus, secs, ramp)| Stage { vus, secs, ramp }).collect(),
    };
    vec![
        b("Humo", "1 usuario, 10 s: ¿responde y en cuánto?", &[(1, 10, false)]),
        b("Carga", "Rampa a 10 usuarios y 60 s sostenidos: tráfico normal", &[(10, 15, true), (10, 60, false)]),
        b(
            "Estrés",
            "Escalera con rampas 10→50→100 usuarios: ¿dónde se degrada?",
            &[(10, 10, true), (10, 20, false), (50, 20, true), (50, 20, false), (100, 20, true), (100, 20, false)],
        ),
        b("Pico", "1→100→1 usuarios de golpe: ráfaga repentina y recuperación", &[(1, 10, false), (100, 15, false), (1, 10, false)]),
        b("Rampa", "Sube de 1 a 100 usuarios en 2 min: encuentra el punto de quiebre", &[(100, 120, true)]),
        b("Resistencia", "Rampa a 20 usuarios y 10 min sostenidos: fugas de memoria / conexiones", &[(20, 30, true), (20, 600, false)]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_requests_and_remaps_positions() {
        let col = |n: &str, reqs: &[&str]| Collection {
            name: n.into(),
            requests: reqs.iter().map(|r| Request { name: r.to_string(), ..Default::default() }).collect(),
            ..Default::default()
        };
        let names = |cols: &[Collection]| cols.iter().map(|c| c.requests.iter().map(|r| r.name.as_str()).collect::<Vec<_>>().join(",")).collect::<Vec<_>>();
        let mut cols = vec![col("A", &["a0", "a1", "a2", "a3"]), col("B", &["b0"])];
        // a3 arriba de a0 (soltar en la mitad superior de a0)
        let m = move_request(&mut cols, (0, 3), 0, 0).unwrap();
        assert_eq!(names(&cols), ["a3,a0,a1,a2", "b0"]);
        assert_eq!((m((0, 3)), m((0, 0)), m((0, 2)), m((1, 0))), ((0, 0), (0, 1), (0, 3), (1, 0)));
        // a3 debajo de a1 (soltar en la mitad inferior de a1 = posición 3 con a3 todavía en 0)
        let m = move_request(&mut cols, (0, 0), 0, 3).unwrap();
        assert_eq!(names(&cols), ["a0,a1,a3,a2", "b0"]);
        assert_eq!((m((0, 0)), m((0, 1)), m((0, 3))), ((0, 2), (0, 0), (0, 3)));
        // a otra colección, al final (soltar en su título)
        let m = move_request(&mut cols, (0, 1), 1, usize::MAX).unwrap();
        assert_eq!(names(&cols), ["a0,a3,a2", "b0,a1"]);
        assert_eq!((m((0, 1)), m((0, 2)), m((1, 0))), ((1, 1), (0, 1), (1, 0)));
        assert!(move_request(&mut cols, (0, 1), 0, 1).is_none() && move_request(&mut cols, (0, 1), 0, 2).is_none(), "soltar en su mismo lugar");
        assert!(move_request(&mut cols, (5, 0), 0, 0).is_none());
    }
}
