//! Aplica la autorización al request (Bearer, Basic, API Key, OAuth 2.0) y obtiene tokens de OAuth 2.0.

use std::{
    collections::HashMap,
    io::{Read, Write},
    net::TcpListener,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::STANDARD, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::blocking::Client;

use crate::model::{self, Auth, OAuth2, ReqParts, Vars, parse_headers, resolve};

pub const GRANTS: [(&str, &str); 3] = [
    ("client_credentials", "Client credentials"),
    ("password", "Password"),
    ("authorization_code", "Authorization code (PKCE)"),
];

pub const DEFAULT_CALLBACK: &str = "http://localhost:53682/callback";

/// Margen antes del vencimiento para pedir un token nuevo.
const SKEW_SECS: u64 = 30;
/// Tiempo para completar el login en el navegador.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(180);

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Token {
    pub access: String,
    pub refresh: String,
    pub expires_at: u64,
}

impl Token {
    fn valid(&self) -> bool {
        !self.access.is_empty() && (self.expires_at == 0 || self.expires_at > now() + SKEW_SECS)
    }
}

/// Guarda el token obtenido en la configuración (para verlo en la UI y persistirlo).
pub fn store(o: &mut OAuth2, t: &Token) {
    (o.access_token, o.expires_at) = (t.access.clone(), t.expires_at);
    if !t.refresh.is_empty() {
        o.refresh_token = t.refresh.clone();
    }
}

/// Agrega la autorización a `req`. Un header que ya pusiste a mano (activo) tiene prioridad.
pub fn apply(client: &Client, auth: &Auth, req: &mut ReqParts, vars: &Vars) -> Result<(), String> {
    let r = |s: &str| resolve(s, vars);
    match auth {
        Auth::Inherit | Auth::None => Ok(()),
        Auth::Bearer { token } => Ok(add_header(req, "Authorization", &format!("Bearer {}", r(token)))),
        Auth::Basic { user, pass } => Ok(add_header(req, "Authorization", &format!("Basic {}", STANDARD.encode(format!("{}:{}", r(user), r(pass)))))),
        Auth::ApiKey { key, value, in_query } => {
            let (k, v) = (r(key), r(value));
            if k.trim().is_empty() {
                return Err("API Key: falta el nombre de la clave".into());
            }
            if *in_query {
                let enc = |s: &str| form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
                let sep = if req.url.contains('?') { '&' } else { '?' };
                req.url = format!("{}{sep}{}={}", req.url, enc(&k), enc(&v));
            } else {
                add_header(req, &k, &v);
            }
            Ok(())
        }
        Auth::Oauth2(o) => {
            let t = token_for(client, o, vars)?;
            Ok(add_header(req, "Authorization", &format!("Bearer {}", t.access)))
        }
    }
}

fn add_header(req: &mut ReqParts, key: &str, value: &str) {
    if parse_headers(&req.headers).iter().any(|(k, _)| k.eq_ignore_ascii_case(key)) {
        return;
    }
    let line = format!("{key}: {value}");
    req.headers = if req.headers.trim().is_empty() { line } else { format!("{}\n{line}", req.headers.trim_end()) };
}

fn cache() -> &'static Mutex<HashMap<String, Token>> {
    static C: OnceLock<Mutex<HashMap<String, Token>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Token vigente: el guardado, el de la caché de esta ejecución, uno renovado con el refresh token,
/// o uno nuevo (client credentials / password). Authorization code necesita el navegador: se pide desde la UI.
pub fn token_for(client: &Client, o: &OAuth2, vars: &Vars) -> Result<Token, String> {
    let saved = Token { access: o.access_token.clone(), refresh: o.refresh_token.clone(), expires_at: o.expires_at };
    if saved.valid() {
        return Ok(saved);
    }
    let r = |s: &str| resolve(s, vars);
    let key = format!("{}|{}|{}|{}|{}", o.grant, r(&o.token_url), r(&o.client_id), r(&o.scope), r(&o.username));
    if let Some(t) = cache().lock().ok().and_then(|c| c.get(&key).cloned()).filter(Token::valid) {
        return Ok(t);
    }
    let t = if !o.refresh_token.is_empty() {
        request_token(client, o, vars, &[("grant_type", "refresh_token".into()), ("refresh_token", o.refresh_token.clone())])
    } else {
        match o.grant.as_str() {
            "password" => request_token(client, o, vars, &[("grant_type", "password".into()), ("username", r(&o.username)), ("password", r(&o.password))]),
            "authorization_code" => Err("OAuth 2.0: no hay token. Pulsa \"Obtener token\" en la pestaña Auth para iniciar sesión en el navegador.".into()),
            _ => request_token(client, o, vars, &[("grant_type", "client_credentials".into())]),
        }
    }?;
    if let Ok(mut c) = cache().lock() {
        c.insert(key, t.clone());
    }
    Ok(t)
}

/// POST al token endpoint (application/x-www-form-urlencoded, credenciales del cliente en el body).
fn request_token(client: &Client, o: &OAuth2, vars: &Vars, extra: &[(&str, String)]) -> Result<Token, String> {
    let r = |s: &str| resolve(s, vars);
    let url = r(&o.token_url);
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(format!("OAuth 2.0: la URL del token no es válida: \"{url}\""));
    }
    let mut form = form_urlencoded::Serializer::new(String::new());
    for (k, v) in extra {
        form.append_pair(k, v);
    }
    for (k, v) in [("client_id", r(&o.client_id)), ("client_secret", r(&o.client_secret)), ("scope", r(&o.scope))] {
        if !v.is_empty() {
            form.append_pair(k, &v);
        }
    }
    let headers = [("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string()), ("Accept".into(), "application/json".into())];
    let res = model::send_http(client, "POST", &url, &headers, &form.finish())?;
    parse_token(res.status, &res.body)
}

fn parse_token(status: u16, body: &str) -> Result<Token, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|_| format!("OAuth 2.0: respuesta {status} sin JSON: {}", body.chars().take(300).collect::<String>()))?;
    let access = v["access_token"].as_str().unwrap_or_default().to_string();
    if access.is_empty() {
        let err = [&v["error_description"], &v["error"]].iter().find_map(|x| x.as_str()).unwrap_or("sin access_token");
        return Err(format!("OAuth 2.0 ({status}): {err}"));
    }
    let expires_in = v["expires_in"].as_u64().or_else(|| v["expires_in"].as_str().and_then(|s| s.parse().ok())).unwrap_or(0);
    Ok(Token {
        access,
        refresh: v["refresh_token"].as_str().unwrap_or_default().into(),
        expires_at: if expires_in == 0 { 0 } else { now() + expires_in },
    })
}

/// Obtiene un token nuevo ahora (botón "Obtener token"). Authorization code abre el navegador y espera el regreso.
pub fn fetch_token(client: &Client, o: &OAuth2, vars: &Vars) -> Result<Token, String> {
    let fresh = OAuth2 { access_token: String::new(), refresh_token: String::new(), expires_at: 0, ..o.clone() };
    if o.grant != "authorization_code" {
        if let Ok(mut c) = cache().lock() {
            c.clear();
        }
        return token_for(client, &fresh, vars);
    }
    authorization_code(client, o, vars)
}

fn random_b64(n: usize) -> Result<String, String> {
    use ring::rand::SecureRandom;
    let mut bytes = vec![0u8; n];
    ring::rand::SystemRandom::new().fill(&mut bytes).map_err(|_| "no se pudo generar un valor aleatorio".to_string())?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Authorization code con PKCE (S256): abre el navegador, escucha el regreso en `callback_url` y canjea el código.
fn authorization_code(client: &Client, o: &OAuth2, vars: &Vars) -> Result<Token, String> {
    let r = |s: &str| resolve(s, vars);
    let auth_url = r(&o.auth_url);
    if !auth_url.starts_with("https://") && !auth_url.starts_with("http://") {
        return Err(format!("OAuth 2.0: la URL de autorización no es válida: \"{auth_url}\""));
    }
    let callback = if o.callback_url.trim().is_empty() { DEFAULT_CALLBACK.to_string() } else { r(&o.callback_url) };
    let cb = reqwest::Url::parse(&callback).map_err(|e| format!("callback_url: {e}"))?;
    if !matches!(cb.host_str(), Some("localhost" | "127.0.0.1")) || cb.scheme() != "http" {
        return Err("callback_url debe ser http://localhost:PUERTO/... (la app recibe ahí el código)".into());
    }
    let port = cb.port_or_known_default().unwrap_or(80);
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("no se pudo escuchar en el puerto {port}: {e}"))?;

    let verifier = random_b64(32)?;
    let challenge = URL_SAFE_NO_PAD.encode(ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes()));
    let state = random_b64(16)?;
    let mut url = reqwest::Url::parse(&auth_url).map_err(|e| format!("auth_url: {e}"))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &r(&o.client_id))
        .append_pair("redirect_uri", &callback)
        .append_pair("scope", &r(&o.scope))
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    open_browser(url.as_str())?;

    let code = wait_for_code(&listener, &state)?;
    request_token(
        client,
        o,
        vars,
        &[("grant_type", "authorization_code".into()), ("code", code), ("redirect_uri", callback), ("code_verifier", verifier)],
    )
}

/// Navegador por defecto del sistema. Solo URLs http(s): nunca se le pasa otra cosa al sistema.
fn open_browser(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("URL no permitida".into());
    }
    #[cfg(windows)]
    let r = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(not(windows))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    r.map(|_| ()).map_err(|e| format!("no se pudo abrir el navegador: {e}"))
}

fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        match listener.accept() {
            Ok((mut s, _)) => {
                let _ = s.set_nonblocking(false);
                let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 8192];
                let n = s.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]);
                let target = head.split_whitespace().nth(1).unwrap_or("/");
                let q: HashMap<String, String> = reqwest::Url::parse(&format!("http://localhost{target}"))
                    .map(|u| u.query_pairs().into_owned().collect())
                    .unwrap_or_default();
                if !q.contains_key("code") && !q.contains_key("error") {
                    respond(&mut s, "Esperando el inicio de sesión…");
                    continue; // p. ej. /favicon.ico
                }
                let result = match (q.get("code"), q.get("error"), q.get("state")) {
                    (_, Some(e), _) => Err(format!("el proveedor respondió: {e} {}", q.get("error_description").map(String::as_str).unwrap_or(""))),
                    (Some(_), _, st) if st.map(String::as_str) != Some(state) => Err("state no coincide: se descartó la respuesta".into()),
                    (Some(c), _, _) => Ok(c.clone()),
                    _ => Err("respuesta sin código".into()),
                };
                respond(&mut s, if result.is_ok() { "Listo: ya puedes volver a api-connect." } else { "No se pudo iniciar sesión; revisa api-connect." });
                return result;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() > LOGIN_TIMEOUT {
                    return Err("se agotó el tiempo esperando el inicio de sesión en el navegador".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn respond(s: &mut std::net::TcpStream, msg: &str) {
    let body = format!("<!doctype html><meta charset=utf-8><title>api-connect</title><p style=\"font:16px sans-serif;margin:40px\">{msg}</p>");
    let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn parts() -> ReqParts {
        ReqParts { method: "GET".into(), url: "http://h/x".into(), headers: "Accept: */*".into(), body: String::new() }
    }

    #[test]
    fn applies_simple_auth() {
        let vars: Vars = [("t".into(), "abc".into()), ("u".into(), "ana".into())].into();
        let c = model::client();
        let mut p = parts();
        apply(&c, &Auth::Bearer { token: "{{t}}".into() }, &mut p, &vars).unwrap();
        assert!(p.headers.ends_with("Authorization: Bearer abc"));
        let mut p = parts();
        apply(&c, &Auth::Basic { user: "{{u}}".into(), pass: "x:y".into() }, &mut p, &vars).unwrap();
        assert!(p.headers.contains(&format!("Basic {}", STANDARD.encode("ana:x:y"))));
        let mut p = parts();
        apply(&c, &Auth::ApiKey { key: "api key".into(), value: "a&b".into(), in_query: true }, &mut p, &vars).unwrap();
        assert_eq!(p.url, "http://h/x?api+key=a%26b");
        // Un header manual activo gana; uno desactivado (//) no.
        let mut p = ReqParts { headers: "Authorization: Mio".into(), ..parts() };
        apply(&c, &Auth::Bearer { token: "x".into() }, &mut p, &vars).unwrap();
        assert_eq!(p.headers, "Authorization: Mio");
        let mut p = ReqParts { headers: "// Authorization: Mio".into(), ..parts() };
        apply(&c, &Auth::Bearer { token: "x".into() }, &mut p, &vars).unwrap();
        assert!(p.headers.ends_with("Authorization: Bearer x"));
    }

    #[test]
    fn oauth_client_credentials_fetches_and_caches() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        static HITS: AtomicUsize = AtomicUsize::new(0);
        std::thread::spawn(move || {
            for mut s in l.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                HITS.fetch_add(1, Ordering::SeqCst);
                let body = if req.contains("grant_type=client_credentials") && req.contains("client_id=app") {
                    r#"{"access_token":"tok-1","expires_in":3600,"token_type":"Bearer"}"#
                } else {
                    r#"{"error":"invalid_client","error_description":"cliente desconocido"}"#
                };
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        let o = OAuth2 { grant: "client_credentials".into(), token_url: format!("http://{addr}/token"), client_id: "{{cid}}".into(), ..Default::default() };
        let vars: Vars = [("cid".into(), "app".into())].into();
        let c = model::client();
        let mut p = parts();
        apply(&c, &Auth::Oauth2(o.clone()), &mut p, &vars).unwrap();
        assert!(p.headers.ends_with("Authorization: Bearer tok-1"));
        apply(&c, &Auth::Oauth2(o.clone()), &mut parts(), &vars).unwrap();
        assert_eq!(HITS.load(Ordering::SeqCst), 1, "el segundo envío usa la caché");
        let bad = OAuth2 { client_id: "otro".into(), ..o };
        let e = fetch_token(&c, &bad, &vars).unwrap_err();
        assert!(e.contains("cliente desconocido"), "{e}");
        let e = token_for(&c, &OAuth2 { grant: "authorization_code".into(), ..Default::default() }, &vars).unwrap_err();
        assert!(e.contains("Obtener token"));
    }
}
