use std::{
    collections::BTreeMap,
    hash::BuildHasher,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub type Vars = BTreeMap<String, String>;

pub const COLLECTIONS: &str = "collections.json";
pub const ENVIRONMENTS: &str = "environments.json";
pub const AGENTS: &str = "agents.json";
pub const SETTINGS: &str = "settings.json";

/// Preferencias de la aplicación.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub claude_timeout_secs: u64,
    /// Repos de Azure DevOps usados recientemente.
    pub repos: Vec<RepoCfg>,
    /// Baterías de estrés (editables en settings.json).
    pub batteries: Vec<crate::runner::Battery>,
    /// Pestañas abiertas al cerrar la app: (colección, request), para restaurarlas.
    pub open_tabs: Vec<(String, String)>,
    pub active_tab: usize,
    /// Tema One Light en vez de One Dark.
    pub light_theme: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { claude_timeout_secs: 120, repos: Vec::new(), batteries: crate::runner::default_batteries(), open_tabs: Vec::new(), active_tab: 0, light_theme: false }
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

// ---- Almacenamiento ----

/// Ok(None) si el archivo no existe. Cualquier otro error se reporta para no sobrescribirlo al guardar.
pub fn load<T: DeserializeOwned>(path: &str) -> Result<Option<T>, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).map(Some).map_err(|e| format!("{path}: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{path}: {e}")),
    }
}

/// Escribe a temporal y renombra: un corte a mitad no corrompe el archivo.
pub fn save<T: Serialize>(path: &str, value: &T) -> Result<(), String> {
    let tmp = format!("{path}.tmp");
    let json = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("{path}: {e}"))
}

// ---- Variables ----

pub fn parse_vars(text: &str) -> Vars {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .filter(|(k, _)| !k.trim().is_empty())
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

pub fn format_vars(vars: &Vars) -> String {
    vars.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\n")
}

/// "Clave: valor" por línea; las que empiezan con "//" están desactivadas y no se envían.
pub fn parse_headers(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .filter_map(|l| l.split_once(':'))
        .filter(|(k, _)| !k.trim().is_empty())
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// Reemplaza {{var}}; las que no existen se dejan tal cual para que se note.
pub fn resolve(text: &str, vars: &Vars) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let inner = &after[..end];
        if inner.contains(['{', '}']) || inner.trim().is_empty() {
            out.push_str(&rest[..start + 1]);
            rest = &rest[start + 1..];
            continue;
        }
        out.push_str(&rest[..start]);
        match dynamic(inner.trim()).or_else(|| vars.get(inner.trim()).cloned()) {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[start..start + 4 + end]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

fn dynamic(key: &str) -> Option<String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    Some(match key {
        "$guid" => {
            let h = format!("{:016x}{:016x}", rand_u64(), rand_u64());
            format!("{}-{}-4{}-a{}-{}", &h[..8], &h[8..12], &h[13..16], &h[17..20], &h[20..])
        }
        "$timestamp" => now.as_secs().to_string(),
        "$isoTimestamp" => iso8601(now.as_secs() as i64, now.subsec_millis()),
        "$randomInt" => (rand_u64() % 1001).to_string(),
        _ => return None,
    })
}

// ponytail: aleatoriedad vía RandomState de std; suficiente para ids de prueba, no para criptografía.
fn rand_u64() -> u64 {
    std::collections::hash_map::RandomState::new().hash_one(Instant::now())
}

/// Segundos Unix -> "AAAA-MM-DDTHH:MM:SS.mmmZ" (algoritmo civil_from_days de H. Hinnant).
pub fn iso8601(secs: i64, ms: u32) -> String {
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}Z", sod / 3600, sod % 3600 / 60, sod % 60)
}

// ---- HTTP ----

/// Cliente HTTP compartido (clonarlo es barato: es un Arc por dentro).
pub fn client() -> reqwest::blocking::Client {
    static CLIENT: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| reqwest::blocking::Client::builder().timeout(Duration::from_secs(60)).build().expect("cliente http"))
        .clone()
}

pub fn send_http(
    client: &reqwest::blocking::Client,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &str,
) -> Result<Response, String> {
    let method = reqwest::Method::from_bytes(method.trim().to_uppercase().as_bytes()).map_err(err_chain)?;
    let mut req = client.request(method, url.trim());
    for (k, v) in headers {
        req = req.header(k, v);
    }
    // Un header Cookie escrito a mano manda sobre el jar.
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("cookie")) {
        if let Some(c) = crate::cookies::header_for(url.trim()) {
            req = req.header("Cookie", c);
        }
    }
    if !body.is_empty() {
        req = req.body(body.to_string());
    }
    let t = Instant::now();
    let res = req.send().map_err(err_chain)?;
    let status = res.status();
    let set_cookies: Vec<String> = res.headers().get_all("set-cookie").iter().map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).collect();
    crate::cookies::store(res.url().as_str(), &set_cookies);
    let headers = res
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
        .collect();
    let bytes = res.bytes().map_err(err_chain)?;
    Ok(Response {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or("").into(),
        ms: t.elapsed().as_millis() as u64,
        size: bytes.len(),
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// reqwest esconde la causa real (DNS, TLS, conexión) en la cadena de source().
pub fn err_chain(e: impl std::error::Error) -> String {
    let mut msg = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        msg.push_str(&format!(": {s}"));
        src = s.source();
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_variables() {
        let vars: Vars = [("base".into(), "http://h".into()), ("id".into(), "7".into())].into();
        assert_eq!(resolve("{{base}}/u/{{ id }}?x={{nope}}", &vars), "http://h/u/7?x={{nope}}");
        assert_eq!(resolve("{{{id}}} {{", &vars), "{7} {{");
        let g = resolve("{{$guid}}", &vars);
        assert_eq!((g.len(), g.matches('-').count()), (36, 4));
    }

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

    #[test]
    fn parses_vars_and_dates() {
        let v = parse_vars("a=1\nb = x=y\nbasura");
        assert_eq!(format_vars(&v), "a=1\nb=x=y");
        assert_eq!(iso8601(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601(1_700_000_000, 5), "2023-11-14T22:13:20.005Z");
        assert_eq!(iso8601(951_782_400, 0), "2000-02-29T00:00:00.000Z");
    }
}
