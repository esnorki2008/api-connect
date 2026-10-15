//! Acciones que el agente ejecuta sobre la plataforma. El agente las escribe en un bloque ```actions con JSON
//! (un objeto o una lista); la app las aplica en orden y le devuelve el resultado.

use serde::Deserialize;
use serde_json::Value;

use crate::model::{Collection, Environments, Request, Vars};

#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Fields {
    pub name: Option<String>,
    pub method: Option<String>,
    pub url: Option<String>,
    /// Texto "Clave: Valor" por línea, u objeto {"Clave": "Valor"}.
    pub headers: Option<Value>,
    /// Texto, u objeto/lista JSON (se guarda formateado).
    pub body: Option<Value>,
    pub pre: Option<String>,
    pub tests: Option<String>,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Environment,
    Collection,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// Cambia el request abierto en el editor (sin guardar).
    EditRequest { set: Fields },
    /// Guarda el editor en su colección (o en `collection`, que se crea si no existe).
    SaveRequest { collection: Option<String> },
    OpenRequest { collection: String, name: String },
    CreateCollection { name: String },
    CreateRequest { collection: String, request: Fields },
    UpdateRequest { collection: String, name: String, set: Fields },
    DeleteRequest { collection: String, name: String },
    DeleteCollection { name: String },
    SetVariables {
        scope: Scope,
        /// Para scope=collection; por defecto la colección activa.
        collection: Option<String>,
        /// Para scope=environment; por defecto el entorno activo.
        environment: Option<String>,
        #[serde(default)]
        values: serde_json::Map<String, Value>,
        #[serde(default)]
        unset: Vec<String>,
    },
    CreateEnvironment {
        name: String,
        #[serde(default)]
        values: serde_json::Map<String, Value>,
        #[serde(default)]
        activate: bool,
    },
    SelectEnvironment { name: String },
    /// Envía el request del editor; el resultado vuelve al agente.
    Send {},
}

impl Action {
    /// Borrar exige que el usuario lo apruebe.
    pub fn destructive(&self) -> bool {
        matches!(self, Action::DeleteRequest { .. } | Action::DeleteCollection { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Action::EditRequest { .. } => "editar el request abierto".into(),
            Action::SaveRequest { collection } => format!("guardar el request{}", collection.as_ref().map(|c| format!(" en \"{c}\"")).unwrap_or_default()),
            Action::OpenRequest { collection, name } => format!("abrir \"{collection} / {name}\""),
            Action::CreateCollection { name } => format!("crear la colección \"{name}\""),
            Action::CreateRequest { collection, request } => format!("crear \"{collection} / {}\"", request.name.as_deref().unwrap_or("?")),
            Action::UpdateRequest { collection, name, .. } => format!("modificar \"{collection} / {name}\""),
            Action::DeleteRequest { collection, name } => format!("BORRAR el request \"{collection} / {name}\""),
            Action::DeleteCollection { name } => format!("BORRAR la colección \"{name}\" y todos sus requests"),
            Action::SetVariables { scope, .. } => format!("cambiar variables de {}", if *scope == Scope::Environment { "entorno" } else { "colección" }),
            Action::CreateEnvironment { name, .. } => format!("crear el entorno \"{name}\""),
            Action::SelectEnvironment { name } => format!("activar el entorno \"{name}\""),
            Action::Send {} => "enviar el request".into(),
        }
    }
}

/// Saca las acciones de los bloques ```actions de la respuesta del agente.
pub fn extract(text: &str) -> Vec<Result<Action, String>> {
    let mut out = Vec::new();
    for (i, part) in text.split("```").enumerate() {
        let Some(json) = (i % 2 == 1).then(|| part.strip_prefix("actions")).flatten() else { continue };
        match serde_json::from_str::<Value>(json.trim()) {
            Ok(Value::Array(items)) => out.extend(items.into_iter().map(parse_one)),
            Ok(v) => out.push(parse_one(v)),
            Err(e) => out.push(Err(format!("JSON inválido en el bloque actions: {e}"))),
        }
    }
    out
}

fn parse_one(v: Value) -> Result<Action, String> {
    let shown = v.to_string();
    serde_json::from_value(v).map_err(|e| format!("acción inválida {shown}: {e}"))
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn to_vars(m: &serde_json::Map<String, Value>) -> Vars {
    m.iter().map(|(k, v)| (k.clone(), text(v))).collect()
}

impl Fields {
    fn apply_to(&self, r: &mut Request) {
        let set = |dst: &mut String, src: &Option<String>| {
            if let Some(s) = src {
                *dst = s.clone();
            }
        };
        set(&mut r.name, &self.name);
        set(&mut r.url, &self.url);
        set(&mut r.pre, &self.pre);
        set(&mut r.tests, &self.tests);
        if let Some(m) = &self.method {
            r.method = m.to_uppercase();
        }
        if let Some(h) = &self.headers {
            r.headers = match h {
                Value::Object(o) => o.iter().map(|(k, v)| format!("{k}: {}", text(v))).collect::<Vec<_>>().join("\n"),
                other => text(other),
            };
        }
        if let Some(b) = &self.body {
            r.body = match b {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => serde_json::to_string_pretty(other).unwrap_or_default(),
            };
        }
    }
}

/// Estado que las acciones pueden tocar (prestado de la App).
pub struct Ws<'a> {
    pub cols: &'a mut Vec<Collection>,
    pub envs: &'a mut Environments,
    pub ed: &'a mut Request,
    pub cur: &'a mut Option<(usize, usize)>,
    pub col_sel: &'a mut usize,
}

/// Qué hay que persistir o disparar después de aplicar.
#[derive(Default, Debug, PartialEq)]
pub struct Effects {
    pub cols: bool,
    pub envs: bool,
    pub send: bool,
}

impl Ws<'_> {
    fn col(&self, name: &str) -> Result<usize, String> {
        self.cols.iter().position(|c| c.name.eq_ignore_ascii_case(name.trim())).ok_or_else(|| {
            let names: Vec<&str> = self.cols.iter().map(|c| c.name.as_str()).collect();
            format!("no existe la colección \"{name}\" (hay: {})", names.join(", "))
        })
    }

    fn req(&self, ci: usize, name: &str) -> Result<usize, String> {
        let reqs = &self.cols[ci].requests;
        reqs.iter().position(|r| r.name == name).or_else(|| reqs.iter().position(|r| r.name.eq_ignore_ascii_case(name.trim()))).ok_or_else(|| {
            format!("no existe \"{name}\" en \"{}\"", self.cols[ci].name)
        })
    }

    fn col_or_create(&mut self, name: &str, fx: &mut Effects) -> usize {
        self.col(name).unwrap_or_else(|_| {
            self.cols.push(Collection { name: name.trim().to_string(), ..Default::default() });
            fx.cols = true;
            self.cols.len() - 1
        })
    }

    pub fn apply(&mut self, a: &Action, fx: &mut Effects) -> Result<String, String> {
        match a {
            Action::EditRequest { set } => {
                set.apply_to(self.ed);
                if self.ed.method.is_empty() {
                    self.ed.method = "GET".into();
                }
                Ok("request del editor actualizado (sin guardar)".into())
            }
            Action::SaveRequest { collection } => {
                let ci = match collection {
                    Some(name) => self.col_or_create(name, fx),
                    None => match *self.cur {
                        Some((c, _)) => c,
                        None => self.col_or_create("General", fx),
                    },
                };
                if self.ed.name.trim().is_empty() {
                    self.ed.name = if self.ed.url.is_empty() { "Sin nombre".into() } else { self.ed.url.clone() };
                }
                match *self.cur {
                    Some((c, i)) if c == ci && i < self.cols[c].requests.len() => self.cols[c].requests[i] = self.ed.clone(),
                    _ => {
                        self.cols[ci].requests.push(self.ed.clone());
                        *self.cur = Some((ci, self.cols[ci].requests.len() - 1));
                    }
                }
                *self.col_sel = ci;
                fx.cols = true;
                Ok(format!("guardado en \"{}\"", self.cols[ci].name))
            }
            Action::OpenRequest { collection, name } => {
                let ci = self.col(collection)?;
                let ri = self.req(ci, name)?;
                *self.ed = self.cols[ci].requests[ri].clone();
                *self.cur = Some((ci, ri));
                *self.col_sel = ci;
                Ok(format!("abierto \"{}\"", self.ed.name))
            }
            Action::CreateCollection { name } => {
                if self.col(name).is_ok() {
                    return Err(format!("ya existe la colección \"{name}\""));
                }
                self.col_or_create(name, fx);
                Ok(format!("colección \"{name}\" creada"))
            }
            Action::CreateRequest { collection, request } => {
                let ci = self.col_or_create(collection, fx);
                let mut r = Request { method: "GET".into(), ..Default::default() };
                request.apply_to(&mut r);
                if r.name.trim().is_empty() {
                    return Err("create_request necesita request.name".into());
                }
                let name = r.name.clone();
                self.cols[ci].requests.push(r);
                fx.cols = true;
                Ok(format!("creado \"{} / {name}\"", self.cols[ci].name))
            }
            Action::UpdateRequest { collection, name, set } => {
                let ci = self.col(collection)?;
                let ri = self.req(ci, name)?;
                set.apply_to(&mut self.cols[ci].requests[ri]);
                if *self.cur == Some((ci, ri)) {
                    *self.ed = self.cols[ci].requests[ri].clone();
                }
                fx.cols = true;
                Ok(format!("modificado \"{} / {}\"", self.cols[ci].name, self.cols[ci].requests[ri].name))
            }
            Action::DeleteRequest { collection, name } => {
                let ci = self.col(collection)?;
                let ri = self.req(ci, name)?;
                self.cols[ci].requests.remove(ri);
                if matches!(*self.cur, Some((c, _)) if c == ci) {
                    *self.cur = None;
                }
                fx.cols = true;
                Ok(format!("borrado \"{collection} / {name}\""))
            }
            Action::DeleteCollection { name } => {
                let ci = self.col(name)?;
                self.cols.remove(ci);
                *self.cur = None;
                *self.col_sel = 0;
                fx.cols = true;
                Ok(format!("colección \"{name}\" borrada"))
            }
            Action::SetVariables { scope, collection, environment, values, unset } => {
                let target: &mut Vars = match scope {
                    Scope::Collection => {
                        let ci = match collection {
                            Some(c) => self.col(c)?,
                            None => (*self.col_sel < self.cols.len()).then_some(*self.col_sel).ok_or("no hay colección activa")?,
                        };
                        fx.cols = true;
                        &mut self.cols[ci].variables
                    }
                    Scope::Environment => {
                        let name = environment.clone().unwrap_or_else(|| self.envs.active.clone());
                        fx.envs = true;
                        self.envs.envs.get_mut(&name).ok_or_else(|| {
                            if name.is_empty() { "no hay entorno activo: usa create_environment".to_string() } else { format!("no existe el entorno \"{name}\"") }
                        })?
                    }
                };
                target.extend(to_vars(values));
                for k in unset {
                    target.remove(k);
                }
                Ok(format!("variables: {} asignadas, {} quitadas", values.len(), unset.len()))
            }
            Action::CreateEnvironment { name, values, activate } => {
                self.envs.envs.entry(name.trim().to_string()).or_default().extend(to_vars(values));
                if *activate || self.envs.active.is_empty() {
                    self.envs.active = name.trim().to_string();
                }
                fx.envs = true;
                Ok(format!("entorno \"{name}\" listo"))
            }
            Action::SelectEnvironment { name } => {
                if !self.envs.envs.contains_key(name) {
                    return Err(format!("no existe el entorno \"{name}\""));
                }
                self.envs.active = name.clone();
                fx.envs = true;
                Ok(format!("entorno \"{name}\" activo"))
            }
            Action::Send {} => {
                fx.send = true;
                Ok("enviando…".into())
            }
        }
    }
}

/// Host de una URL ya resuelta ("https://user@api.x.com:8443/a" -> "api.x.com:8443").
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.trim().split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?.to_lowercase();
    (!host.is_empty()).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_actions_from_reply() {
        let reply = "Hago esto:\n```actions\n[{\"action\":\"create_collection\",\"name\":\"A\"},{\"action\":\"send\"},{\"action\":\"volar\"}]\n```\n```js\nignorado()\n```\n```actions\n{\"action\":\"select_environment\",\"name\":\"qa\"}\n```";
        let acts = extract(reply);
        assert_eq!(acts.len(), 4);
        assert_eq!(acts[0], Ok(Action::CreateCollection { name: "A".into() }));
        assert_eq!(acts[1], Ok(Action::Send {}));
        assert!(acts[2].as_ref().unwrap_err().contains("volar"));
        assert_eq!(acts[3], Ok(Action::SelectEnvironment { name: "qa".into() }));
        assert!(extract("```actions\n{nope\n```")[0].is_err());
    }

    #[test]
    fn applies_actions_to_workspace() {
        let (mut cols, mut envs, mut ed, mut cur, mut sel) = (Vec::new(), Environments::default(), Request::default(), None, 0);
        let mut ws = Ws { cols: &mut cols, envs: &mut envs, ed: &mut ed, cur: &mut cur, col_sel: &mut sel };
        let mut fx = Effects::default();
        let run = |ws: &mut Ws, fx: &mut Effects, json: &str| ws.apply(&serde_json::from_str(json).unwrap(), fx);

        run(&mut ws, &mut fx, r#"{"action":"create_request","collection":"Users","request":{"name":"Crear","method":"post","url":"{{base}}/u","headers":{"Content-Type":"application/json"},"body":{"n":1}}}"#).unwrap();
        run(&mut ws, &mut fx, r#"{"action":"create_environment","name":"qa","values":{"base":"http://qa","port":8080}}"#).unwrap();
        run(&mut ws, &mut fx, r#"{"action":"set_variables","scope":"collection","collection":"users","values":{"id":7}}"#).unwrap();
        run(&mut ws, &mut fx, r#"{"action":"open_request","collection":"Users","name":"crear"}"#).unwrap();
        run(&mut ws, &mut fx, r#"{"action":"edit_request","set":{"tests":"pm.test('ok', () => {})"}}"#).unwrap();
        run(&mut ws, &mut fx, r#"{"action":"save_request"}"#).unwrap();
        assert!(run(&mut ws, &mut fx, r#"{"action":"update_request","collection":"Nope","name":"x","set":{}}"#).unwrap_err().contains("hay: Users"));
        assert!(run(&mut ws, &mut fx, r#"{"action":"set_variables","scope":"environment","environment":"prod","values":{}}"#).is_err());
        run(&mut ws, &mut fx, r#"{"action":"send"}"#).unwrap();
        assert_eq!(fx, Effects { cols: true, envs: true, send: true });

        let r = &cols[0].requests[0];
        assert_eq!((r.method.as_str(), r.headers.as_str(), r.body.as_str()), ("POST", "Content-Type: application/json", "{\n  \"n\": 1\n}"));
        assert_eq!(r.tests, "pm.test('ok', () => {})");
        assert_eq!(cols[0].requests.len(), 1, "save_request sobre el request abierto no duplica");
        assert_eq!(cols[0].variables.get("id").unwrap(), "7");
        assert_eq!((envs.active.as_str(), envs.envs["qa"]["port"].as_str()), ("qa", "8080"));
        assert_eq!(cur, Some((0, 0)));
    }

    #[test]
    fn destructive_and_hosts() {
        assert!(Action::DeleteCollection { name: "x".into() }.destructive());
        assert!(!Action::Send {}.destructive());
        assert_eq!(host_of("https://user:p@API.x.com:8443/a?b#c").as_deref(), Some("api.x.com:8443"));
        assert_eq!(host_of("{{base}}/x"), None);
    }
}
