//! Importa un comando cURL a un request, o una colección de Postman v2.x a una colección.

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde_json::Value;

use crate::model::{Collection, Request};

fn tokenize(s: &str) -> Vec<String> {
    let s = s.replace("\\\r\n", " ").replace("\\\n", " ");
    let c: Vec<char> = s.chars().collect();
    let (mut out, mut cur, mut i) = (Vec::new(), None::<String>, 0);
    while i < c.len() {
        if c[i].is_whitespace() {
            out.extend(cur.take());
            i += 1;
            continue;
        }
        let t = cur.get_or_insert_with(String::new);
        match c[i] {
            '\'' => {
                i += 1;
                while i < c.len() && c[i] != '\'' {
                    t.push(c[i]);
                    i += 1;
                }
                i += 1;
            }
            '"' => {
                i += 1;
                while i < c.len() && c[i] != '"' {
                    if c[i] == '\\' && i + 1 < c.len() && "\"\\$`".contains(c[i + 1]) {
                        i += 1;
                    }
                    t.push(c[i]);
                    i += 1;
                }
                i += 1;
            }
            '\\' if i + 1 < c.len() => {
                t.push(c[i + 1]);
                i += 2;
            }
            ch => {
                t.push(ch);
                i += 1;
            }
        }
    }
    out.extend(cur);
    out
}

pub fn parse_curl(cmd: &str) -> Result<Request, String> {
    let mut cmd = cmd.trim().to_string();
    // "Copy as cURL (cmd)" de Chrome escapa todo con ^
    if cmd.contains("^\"") {
        let joined = cmd.replace("^\r\n", " ").replace("^\n", " ");
        let mut chars = joined.chars();
        cmd = std::iter::from_fn(|| chars.next().map(|c| if c == '^' { chars.next().unwrap_or('^') } else { c }))
            .collect();
    }
    let t = tokenize(&cmd);
    if t.first().map(String::as_str) != Some("curl") {
        return Err("El comando debe empezar con curl".into());
    }
    let (mut method, mut url, mut form) = (String::new(), String::new(), false);
    let (mut headers, mut data) = (Vec::<String>::new(), Vec::<String>::new());
    let mut i = 1;
    while i < t.len() {
        let a = &t[i];
        let (flag, inline) = if a.starts_with("--") && a.contains('=') {
            let (f, v) = a.split_once('=').unwrap();
            (f.to_string(), Some(v.to_string()))
        } else if a.len() > 2 && !a.starts_with("--") && a.starts_with('-') && a.chars().nth(1).is_some_and(|c| "XHduAbe".contains(c)) {
            (a[..2].to_string(), Some(a[2..].to_string()))
        } else {
            (a.clone(), None)
        };
        let mut val = || match &inline {
            Some(v) => v.clone(),
            None => {
                i += 1;
                t.get(i).cloned().unwrap_or_default()
            }
        };
        match flag.as_str() {
            "-X" | "--request" => method = val().to_uppercase(),
            "-H" | "--header" => headers.push(val()),
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" => {
                data.push(val());
                form = true;
            }
            "--data-urlencode" => {
                let x = val();
                let enc = |s: &str| form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
                data.push(match x.split_once('=') {
                    Some((k, v)) => format!("{k}={}", enc(v)),
                    None => enc(&x),
                });
                form = true;
            }
            "--json" => {
                data.push(val());
                headers.push("Content-Type: application/json".into());
                headers.push("Accept: application/json".into());
            }
            "-u" | "--user" => headers.push(format!("Authorization: Basic {}", B64.encode(val()))),
            "-A" | "--user-agent" => headers.push(format!("User-Agent: {}", val())),
            "-b" | "--cookie" => headers.push(format!("Cookie: {}", val())),
            "-e" | "--referer" => headers.push(format!("Referer: {}", val())),
            "-I" | "--head" => method = "HEAD".into(),
            "--url" => url = val(),
            "-o" | "--output" | "-m" | "--max-time" | "--connect-timeout" | "-w" | "--write-out" | "-x" | "--proxy"
            | "--cacert" | "--cert" | "--key" => {
                val();
            }
            f if !f.starts_with('-') => url = f.to_string(),
            _ => {} // -s, -L, -k, --compressed, etc.
        }
        i += 1;
    }
    if form && !headers.iter().any(|h| h.to_lowercase().starts_with("content-type:")) {
        headers.push("Content-Type: application/x-www-form-urlencoded".into());
    }
    if method.is_empty() {
        method = if data.is_empty() { "GET" } else { "POST" }.into();
    }
    Ok(Request { method, url, headers: headers.join("\n"), body: data.join("&"), ..Default::default() })
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn auth_header(a: &Value) -> Option<String> {
    let kind = a["type"].as_str()?;
    let kv = |key: &str| -> String {
        match &a[kind] {
            Value::Array(items) => items.iter().find(|x| x["key"] == key).map(|x| text(&x["value"])).unwrap_or_default(),
            obj => text(&obj[key]),
        }
    };
    match kind {
        "bearer" => Some(format!("Authorization: Bearer {}", kv("token"))),
        // ponytail: Basic se codifica al importar; si usuario/clave son {{vars}} hay que reescribir el header a mano.
        "basic" => Some(format!("Authorization: Basic {}", B64.encode(format!("{}:{}", kv("username"), kv("password"))))),
        "apikey" if matches!(kv("in").as_str(), "" | "header") => Some(format!("{}: {}", kv("key"), kv("value"))),
        _ => None,
    }
}

fn script_of(events: &Value, kind: &str) -> String {
    let empty = Vec::new();
    events
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter(|e| e["listen"] == kind)
        .map(|e| match &e["script"]["exec"] {
            Value::Array(lines) => lines.iter().map(text).collect::<Vec<_>>().join("\n"),
            other => text(other),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn join(a: &str, b: &str) -> String {
    [a, b].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("\n")
}

// Auth y scripts de colección/carpeta se heredan, como en Postman.
fn walk(items: &[Value], prefix: &str, auth: &Value, pre: &str, tests: &str, out: &mut Vec<Request>) {
    for it in items {
        let p = join(pre, &script_of(&it["event"], "prerequest"));
        let t = join(tests, &script_of(&it["event"], "test"));
        let name = it["name"].as_str().unwrap_or("");
        let auth = if it["auth"].is_null() { auth } else { &it["auth"] };
        if let Some(sub) = it["item"].as_array() {
            walk(sub, &format!("{prefix}{name} / "), auth, &p, &t, out);
            continue;
        }
        let r = &it["request"];
        let url = match r {
            Value::String(s) => s.clone(),
            _ => match &r["url"] {
                Value::String(s) => s.clone(),
                u => text(&u["raw"]),
            },
        };
        let mut headers: Vec<String> = r["header"]
            .as_array()
            .map(|hs| hs.iter().filter(|h| h["disabled"] != true).map(|h| format!("{}: {}", text(&h["key"]), text(&h["value"]))).collect())
            .unwrap_or_default();
        let has = |hs: &[String], name: &str| hs.iter().any(|h| h.to_lowercase().starts_with(&format!("{}:", name.to_lowercase())));
        if let Some(ah) = auth_header(if r["auth"].is_null() { auth } else { &r["auth"] }) {
            if !has(&headers, ah.split(':').next().unwrap_or("")) {
                headers.push(ah);
            }
        }
        let b = &r["body"];
        let body = match b["mode"].as_str() {
            Some("raw") => text(&b["raw"]),
            Some(mode @ ("urlencoded" | "formdata")) => {
                if !has(&headers, "content-type") {
                    headers.push("Content-Type: application/x-www-form-urlencoded".into());
                }
                // ponytail: formdata se envía como urlencoded y sin archivos; multipart real si hace falta.
                b[mode]
                    .as_array()
                    .map(|fs| {
                        fs.iter()
                            .filter(|f| f["disabled"] != true && f["type"] != "file")
                            .map(|f| format!("{}={}", text(&f["key"]), text(&f["value"])))
                            .collect::<Vec<_>>()
                            .join("&")
                    })
                    .unwrap_or_default()
            }
            _ => String::new(),
        };
        out.push(Request {
            name: format!("{prefix}{name}"),
            method: r["method"].as_str().unwrap_or("GET").to_uppercase(),
            url,
            headers: headers.join("\n"),
            body,
            pre: p,
            tests: t,
            auth: Default::default(),
            origin: None,
        });
    }
}

pub fn import_postman(json: &str) -> Result<Collection, String> {
    let c: Value = serde_json::from_str(json).map_err(|e| format!("JSON inválido: {e}"))?;
    let items = c["item"].as_array().filter(|_| c["info"].is_object()).ok_or("No parece una colección de Postman v2.x")?;
    let mut requests = Vec::new();
    walk(items, "", &c["auth"], &script_of(&c["event"], "prerequest"), &script_of(&c["event"], "test"), &mut requests);
    let variables = c["variable"]
        .as_array()
        .map(|vs| vs.iter().filter_map(|v| Some((v["key"].as_str().filter(|k| !k.is_empty())?.to_string(), text(&v["value"])))).collect())
        .unwrap_or_default();
    Ok(Collection { name: c["info"]["name"].as_str().unwrap_or("Importada").to_string(), variables, requests, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curl_bash() {
        let r = parse_curl(
            "curl 'https://api.x/u' -H 'Content-Type: application/json' \\\n  -H \"Authorization: Bearer \\\"t\\\"\" --data-raw '{\"a\":1}' --compressed",
        )
        .unwrap();
        assert_eq!((r.method.as_str(), r.url.as_str()), ("POST", "https://api.x/u"));
        assert_eq!(r.headers, "Content-Type: application/json\nAuthorization: Bearer \"t\"");
        assert_eq!(r.body, r#"{"a":1}"#);
    }

    #[test]
    fn curl_short_flags_and_cmd() {
        let r = parse_curl("curl -XDELETE -u user:pass https://api.x/u/1").unwrap();
        assert_eq!(r.method, "DELETE");
        assert_eq!(r.headers, format!("Authorization: Basic {}", B64.encode("user:pass")));
        let r = parse_curl(r#"curl ^"https://api.x/q^" -H ^"Accept: */*^" --data-urlencode ^"q=a b^""#).unwrap();
        assert_eq!(r.url, "https://api.x/q");
        assert_eq!(r.body, "q=a+b");
        assert!(r.headers.contains("x-www-form-urlencoded"));
        assert!(parse_curl("wget x").is_err());
    }

    #[test]
    fn postman_collection() {
        let c = import_postman(
            r#"{
          "info": { "name": "Demo" },
          "auth": { "type": "bearer", "bearer": [{ "key": "token", "value": "{{token}}" }] },
          "event": [{ "listen": "test", "script": { "exec": ["// col"] } }],
          "variable": [{ "key": "base", "value": "http://h" }],
          "item": [
            { "name": "Users", "item": [{ "name": "List", "request": { "method": "get", "url": { "raw": "{{base}}/u" },
              "header": [{ "key": "A", "value": "1" }, { "key": "B", "value": "2", "disabled": true }] } }] },
            { "name": "Login", "request": { "method": "POST", "url": "{{base}}/login", "auth": { "type": "noauth" },
              "body": { "mode": "urlencoded", "urlencoded": [{ "key": "u", "value": "x" }] } },
              "event": [{ "listen": "test", "script": { "exec": ["pm.test('ok', () => {})"] } }] }
          ]
        }"#,
        )
        .unwrap();
        assert_eq!(c.variables.get("base").map(String::as_str), Some("http://h"));
        let (list, login) = (&c.requests[0], &c.requests[1]);
        assert_eq!((list.name.as_str(), list.method.as_str(), list.url.as_str()), ("Users / List", "GET", "{{base}}/u"));
        assert_eq!(list.headers, "A: 1\nAuthorization: Bearer {{token}}");
        assert_eq!(login.headers, "Content-Type: application/x-www-form-urlencoded");
        assert_eq!(login.body, "u=x");
        assert_eq!(login.tests, "// col\npm.test('ok', () => {})");
        assert!(import_postman(r#"{"foo":1}"#).is_err());
    }
}
