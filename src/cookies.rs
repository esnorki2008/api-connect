//! Cookie jar: guarda las Set-Cookie de las respuestas y las envía en los requests que correspondan
//! (dominio, path, Secure y vencimiento), como un navegador. La app lo persiste en cookies.json.

use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering::Relaxed},
};

use serde::{Deserialize, Serialize};

pub const FILE: &str = "cookies.json";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cookie {
    /// Sin punto inicial.
    pub domain: String,
    /// true = solo ese host exacto (no llevaba atributo Domain).
    pub host_only: bool,
    pub path: String,
    pub name: String,
    pub value: String,
    /// Segundos Unix; None = de sesión.
    pub expires: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
}

static JAR: Mutex<Vec<Cookie>> = Mutex::new(Vec::new());
static ENABLED: AtomicBool = AtomicBool::new(false);
static DIRTY: AtomicBool = AtomicBool::new(false);

/// La app y la consola lo activan; las pruebas de otros módulos quedan sin cookies.
pub fn enable(on: bool) {
    ENABLED.store(on, Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Relaxed)
}

pub fn snapshot() -> Vec<Cookie> {
    JAR.lock().map(|j| j.clone()).unwrap_or_default()
}

pub fn replace(cookies: Vec<Cookie>) {
    if let Ok(mut j) = JAR.lock() {
        *j = cookies;
        DIRTY.store(true, Relaxed);
    }
}

/// true si cambió desde la última llamada (para guardar en disco).
pub fn take_dirty() -> bool {
    DIRTY.swap(false, Relaxed)
}

fn now() -> i64 {
    crate::auth::now() as i64
}

/// Valor del header Cookie para esta URL, si hay alguna.
pub fn header_for(url: &str) -> Option<String> {
    if !enabled() {
        return None;
    }
    let u = reqwest::Url::parse(url).ok()?;
    let (host, path, https) = (u.host_str()?.to_lowercase(), u.path().to_string(), u.scheme() == "https");
    let mut jar = JAR.lock().ok()?;
    let before = jar.len();
    let t = now();
    jar.retain(|c| c.expires.is_none_or(|e| e > t));
    if jar.len() != before {
        DIRTY.store(true, Relaxed);
    }
    let mut matching: Vec<&Cookie> = jar.iter().filter(|c| domain_match(c, &host) && path_match(&c.path, &path) && (!c.secure || https)).collect();
    // Paths más específicos primero, como los navegadores.
    matching.sort_by_key(|c| std::cmp::Reverse(c.path.len()));
    let v: Vec<String> = matching.iter().map(|c| format!("{}={}", c.name, c.value)).collect();
    (!v.is_empty()).then(|| v.join("; "))
}

fn domain_match(c: &Cookie, host: &str) -> bool {
    host == c.domain || (!c.host_only && host.ends_with(&format!(".{}", c.domain)))
}

fn path_match(cookie_path: &str, path: &str) -> bool {
    path == cookie_path || (path.starts_with(cookie_path) && (cookie_path.ends_with('/') || path[cookie_path.len()..].starts_with('/')))
}

/// Guarda las Set-Cookie de una respuesta de `url`.
pub fn store(url: &str, set_cookies: &[String]) {
    if !enabled() || set_cookies.is_empty() {
        return;
    }
    let Ok(u) = reqwest::Url::parse(url) else { return };
    let Some(host) = u.host_str().map(str::to_lowercase) else { return };
    let Ok(mut jar) = JAR.lock() else { return };
    for sc in set_cookies {
        let Some(c) = parse(sc, &host, u.path(), now()) else { continue };
        jar.retain(|x| !(x.domain == c.domain && x.path == c.path && x.name == c.name));
        if c.expires.is_none_or(|e| e > now()) {
            jar.push(c);
        }
        DIRTY.store(true, Relaxed);
    }
}

/// Interpreta una Set-Cookie. Rechaza un Domain que no corresponda al host (un sitio no puede fijar cookies a otro).
pub fn parse(set_cookie: &str, host: &str, req_path: &str, now: i64) -> Option<Cookie> {
    let mut parts = set_cookie.split(';');
    let (name, value) = parts.next()?.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let default_path = match req_path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => req_path[..i].to_string(),
    };
    let mut c = Cookie { domain: host.into(), host_only: true, path: default_path, name: name.into(), value: value.trim().into(), ..Default::default() };
    let mut max_age = None;
    for attr in parts {
        let (k, v) = attr.split_once('=').map_or((attr.trim(), ""), |(k, v)| (k.trim(), v.trim()));
        match k.to_ascii_lowercase().as_str() {
            "domain" if !v.is_empty() => {
                let d = v.trim_start_matches('.').to_lowercase();
                if host != d && !host.ends_with(&format!(".{d}")) {
                    return None;
                }
                (c.domain, c.host_only) = (d, false);
            }
            "path" if v.starts_with('/') => c.path = v.into(),
            "max-age" => max_age = v.parse::<i64>().ok(),
            "expires" => c.expires = c.expires.or(http_date(v)),
            "secure" => c.secure = true,
            "httponly" => c.http_only = true,
            _ => {}
        }
    }
    // Max-Age manda sobre Expires.
    if let Some(m) = max_age {
        c.expires = Some(now + m);
    }
    Some(c)
}

/// "Wed, 21 Oct 2015 07:28:00 GMT" (y la variante con guiones "21-Oct-2015") -> segundos Unix.
fn http_date(s: &str) -> Option<i64> {
    let s = s.replace('-', " ");
    let f: Vec<&str> = s.split_whitespace().collect();
    let f = if f.first()?.ends_with(',') || f.first()?.chars().all(char::is_alphabetic) { &f[1..] } else { &f[..] };
    let (day, mon, year, time) = (f.first()?.parse::<i64>().ok()?, *f.get(1)?, f.get(2)?.parse::<i64>().ok()?, f.get(3)?);
    let year = if year < 100 { year + if year < 70 { 2000 } else { 1900 } } else { year };
    let m = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|x| mon.to_lowercase().starts_with(x))? as i64 + 1;
    let t: Vec<i64> = time.split(':').filter_map(|x| x.parse().ok()).collect();
    let (h, mi, se) = (*t.first()?, *t.get(1)?, *t.get(2).unwrap_or(&0));
    // days_from_civil (H. Hinnant)
    let y = if m <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + h * 3600 + mi * 60 + se)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_set_cookie_and_matches_requests() {
        let c = parse("sid=abc; Path=/api; Domain=.ejemplo.com; Secure; HttpOnly; Max-Age=60", "app.ejemplo.com", "/x", 1000).unwrap();
        assert_eq!((c.domain.as_str(), c.host_only, c.path.as_str(), c.expires, c.secure, c.http_only), ("ejemplo.com", false, "/api", Some(1060), true, true));
        assert!(parse("x=1; Domain=otro.com", "app.ejemplo.com", "/", 0).is_none(), "no se aceptan cookies de otro dominio");
        assert_eq!(parse("x=1", "h.com", "/a/b/c", 0).unwrap().path, "/a/b");
        assert_eq!(http_date("Wed, 21 Oct 2015 07:28:00 GMT"), Some(1_445_412_480));
        assert_eq!(http_date("Thu, 01-Jan-1970 00:00:10 GMT"), Some(10));
        assert!(path_match("/api", "/api/v1") && !path_match("/api", "/apiv1") && path_match("/", "/x"));

        enable(true);
        replace(Vec::new());
        store("https://app.ejemplo.com/login", &["sid=abc; Path=/; Domain=ejemplo.com; Secure".into(), "pref=dark".into(), "old=1; Max-Age=0".into()]);
        assert_eq!(header_for("https://api.ejemplo.com/x").as_deref(), Some("sid=abc"));
        assert_eq!(header_for("http://api.ejemplo.com/x"), None, "Secure solo por https");
        let both = header_for("https://app.ejemplo.com/").unwrap();
        assert!(both.contains("sid=abc") && both.contains("pref=dark") && !both.contains("old"));
        store("https://app.ejemplo.com/", &["sid=; Max-Age=0; Domain=ejemplo.com; Path=/".into()]);
        assert_eq!(header_for("https://app.ejemplo.com/").as_deref(), Some("pref=dark"), "Max-Age=0 la borra");
        assert!(take_dirty());
        replace(Vec::new());
        enable(false);
    }
}
