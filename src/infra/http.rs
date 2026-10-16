//! Cliente HTTP compartido y envío de requests (con el cookie jar).

use std::time::{Duration, Instant};

use crate::{domain::model::Response, infra::cookies};

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
        if let Some(c) = cookies::header_for(url.trim()) {
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
    cookies::store(res.url().as_str(), &set_cookies);
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
