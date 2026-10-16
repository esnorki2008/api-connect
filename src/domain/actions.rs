//! Acciones que el agente ejecuta sobre la plataforma. El agente las escribe en un bloque ```actions con JSON
//! (un objeto o una lista); la app las aplica en orden y le devuelve el resultado.

use serde::Deserialize;
use serde_json::Value;

use crate::{
    domain::model::{Auth, Battery, Collection, Environments, MAX_VUS, Request, Stage, Vars},
    i18n::t,
    tr,
};

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
    /// {"type":"bearer","token":"{{token}}"}, basic, api_key, oauth2, none o inherit.
    pub auth: Option<Auth>,
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
    /// Autorización de la colección (la heredan sus requests con auth "inherit").
    SetCollectionAuth { collection: String, auth: Auth },
    /// Envía el request del editor; el resultado vuelve al agente.
    Send {},
    /// Crea (o reemplaza) una batería de estrés reutilizable.
    CreateBattery {
        name: String,
        #[serde(default)]
        description: String,
        stages: Vec<Stage>,
    },
    /// Corre la colección en orden con sus tests; el resumen vuelve al agente.
    RunCollection {
        collection: String,
        #[serde(default)]
        iterations: Option<u32>,
    },
    /// Prueba de estrés (siempre pide aprobación); las estadísticas vuelven al agente.
    RunStress {
        collection: String,
        /// Endpoint a estresar; por defecto toda la colección.
        #[serde(default)]
        request: Option<String>,
        /// Request que se envía una vez antes de medir (p. ej. login).
        #[serde(default)]
        setup: Option<String>,
        /// Batería por nombre, o etapas a la medida.
        #[serde(default)]
        battery: Option<String>,
        #[serde(default)]
        stages: Option<Vec<Stage>>,
    },
    /// Importa un contrato OpenAPI 3 / Swagger 2 (YAML o JSON) desde una URL o desde el texto.
    ImportOpenapi {
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        spec: Option<String>,
    },
}

/// Ejecución que pidió el agente: la UI la lanza y le devuelve el resultado.
#[derive(Debug, Clone, PartialEq)]
pub enum Launch {
    Run { col: usize, iterations: u32 },
    Stress { col: usize, target: Option<usize>, setup: Option<usize>, stages: Vec<Stage>, label: String },
    /// URL o texto del contrato; se descarga y convierte fuera del dominio.
    Import(String),
}

impl Action {
    /// Borrar, generar carga, escribir scripts o cambiar la auth siempre exige que el usuario lo apruebe: un script
    /// corre con acceso a tus variables, y una auth (p. ej. el token_url de OAuth) decide adónde van tus secretos.
    pub fn needs_approval(&self) -> bool {
        match self {
            Action::DeleteRequest { .. } | Action::DeleteCollection { .. } | Action::RunStress { .. } | Action::SetCollectionAuth { .. } => true,
            Action::EditRequest { set } | Action::UpdateRequest { set, .. } | Action::CreateRequest { request: set, .. } => set.has_scripts() || set.auth.is_some(),
            _ => false,
        }
    }

    /// Las que ejecutan algo afuera del dominio (la UI o la consola las lanzan y esperan el resultado).
    pub fn launches(&self) -> bool {
        matches!(self, Action::Send {} | Action::RunCollection { .. } | Action::RunStress { .. } | Action::ImportOpenapi { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Action::EditRequest { set } => tr!("editar el request abierto{}", "edit the open request{}", set.scripts_note()),
            Action::SaveRequest { collection } => tr!("guardar el request{}", "save the request{}", collection.as_ref().map(|c| tr!(" en \"{c}\"", " in \"{c}\"")).unwrap_or_default()),
            Action::OpenRequest { collection, name } => tr!("abrir \"{collection} / {name}\"", "open \"{collection} / {name}\""),
            Action::CreateCollection { name } => tr!("crear la colección \"{name}\"", "create the collection \"{name}\""),
            Action::CreateRequest { collection, request } => tr!("crear \"{collection} / {}\"{}", "create \"{collection} / {}\"{}", request.name.as_deref().unwrap_or("?"), request.scripts_note()),
            Action::UpdateRequest { collection, name, set } => tr!("modificar \"{collection} / {name}\"{}", "modify \"{collection} / {name}\"{}", set.scripts_note()),
            Action::SetCollectionAuth { collection, auth } => tr!("poner auth {}{} a la colección \"{collection}\"", "set {}{} auth on the collection \"{collection}\"", auth_kind(auth), token_host(auth)),
            Action::DeleteRequest { collection, name } => tr!("BORRAR el request \"{collection} / {name}\"", "DELETE the request \"{collection} / {name}\""),
            Action::DeleteCollection { name } => tr!("BORRAR la colección \"{name}\" y todos sus requests", "DELETE the collection \"{name}\" and all its requests"),
            Action::SetVariables { scope, .. } => if *scope == Scope::Environment { t("cambiar variables de entorno", "change environment variables") } else { t("cambiar variables de colección", "change collection variables") }.into(),
            Action::CreateEnvironment { name, .. } => tr!("crear el entorno \"{name}\"", "create the environment \"{name}\""),
            Action::SelectEnvironment { name } => tr!("activar el entorno \"{name}\"", "activate the environment \"{name}\""),
            Action::Send {} => t("enviar el request", "send the request").into(),
            Action::CreateBattery { name, .. } => tr!("crear la batería de estrés \"{name}\"", "create the stress battery \"{name}\""),
            Action::RunCollection { collection, .. } => tr!("correr la colección \"{collection}\" con sus tests", "run the collection \"{collection}\" with its tests"),
            Action::RunStress { collection, request, battery, stages, .. } => {
                let what = request.as_ref().map_or(tr!("toda la colección \"{collection}\"", "the whole collection \"{collection}\""), |r| format!("\"{collection} / {r}\""));
                let plan = match (battery, stages) {
                    (Some(b), _) => tr!("batería \"{b}\"", "battery \"{b}\""),
                    (None, Some(s)) => s.iter().map(Stage::label).collect::<Vec<_>>().join(" → "),
                    _ => "?".into(),
                };
                tr!("PRUEBA DE ESTRÉS contra {what} ({plan})", "STRESS TEST against {what} ({plan})")
            }
            Action::ImportOpenapi { url, .. } => match url {
                Some(u) => tr!("importar el contrato OpenAPI de {u}", "import the OpenAPI contract from {u}"),
                None => t("importar un contrato OpenAPI", "import an OpenAPI contract").into(),
            },
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
            Err(e) => out.push(Err(tr!("JSON inválido en el bloque actions: {e}", "invalid JSON in the actions block: {e}"))),
        }
    }
    out
}

fn parse_one(v: Value) -> Result<Action, String> {
    let shown = v.to_string();
    serde_json::from_value(v).map_err(|e| tr!("acción inválida {shown}: {e}", "invalid action {shown}: {e}"))
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
    fn has_scripts(&self) -> bool {
        [&self.pre, &self.tests].iter().any(|s| s.as_deref().is_some_and(|s| !s.trim().is_empty()))
    }

    fn scripts_note(&self) -> &'static str {
        if self.has_scripts() { t(" CON SCRIPTS (revísalos en el bloque de acciones: corren con acceso a tus variables)", " WITH SCRIPTS (review them in the actions block: they run with access to your variables)") } else { "" }
    }

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
        if let Some(a) = &self.auth {
            r.auth = a.clone();
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
    pub batteries: &'a mut Vec<Battery>,
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
    pub settings: bool,
    pub send: bool,
    pub launch: Option<Launch>,
}

impl Ws<'_> {
    fn col(&self, name: &str) -> Result<usize, String> {
        self.cols.iter().position(|c| c.name.eq_ignore_ascii_case(name.trim())).ok_or_else(|| {
            let names: Vec<&str> = self.cols.iter().map(|c| c.name.as_str()).collect();
            tr!("no existe la colección \"{name}\" (hay: {})", "the collection \"{name}\" does not exist (available: {})", names.join(", "))
        })
    }

    fn req(&self, ci: usize, name: &str) -> Result<usize, String> {
        let reqs = &self.cols[ci].requests;
        reqs.iter().position(|r| r.name == name).or_else(|| reqs.iter().position(|r| r.name.eq_ignore_ascii_case(name.trim()))).ok_or_else(|| {
            tr!("no existe \"{name}\" en \"{}\"", "\"{name}\" does not exist in \"{}\"", self.cols[ci].name)
        })
    }

    fn batteries_list(&self) -> String {
        self.batteries.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")
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
                Ok(t("request del editor actualizado (sin guardar)", "editor request updated (not saved)").into())
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
                    self.ed.name = if self.ed.url.is_empty() { t("Sin nombre", "Untitled").into() } else { self.ed.url.clone() };
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
                Ok(tr!("guardado en \"{}\"", "saved in \"{}\"", self.cols[ci].name))
            }
            Action::OpenRequest { collection, name } => {
                let ci = self.col(collection)?;
                let ri = self.req(ci, name)?;
                *self.ed = self.cols[ci].requests[ri].clone();
                *self.cur = Some((ci, ri));
                *self.col_sel = ci;
                Ok(tr!("abierto \"{}\"", "opened \"{}\"", self.ed.name))
            }
            Action::CreateCollection { name } => {
                if self.col(name).is_ok() {
                    return Err(tr!("ya existe la colección \"{name}\"", "the collection \"{name}\" already exists"));
                }
                self.col_or_create(name, fx);
                Ok(tr!("colección \"{name}\" creada", "collection \"{name}\" created"))
            }
            Action::CreateRequest { collection, request } => {
                let ci = self.col_or_create(collection, fx);
                let mut r = Request { method: "GET".into(), ..Default::default() };
                request.apply_to(&mut r);
                if r.name.trim().is_empty() {
                    return Err(t("create_request necesita request.name", "create_request needs request.name").into());
                }
                let name = r.name.clone();
                self.cols[ci].requests.push(r);
                fx.cols = true;
                Ok(tr!("creado \"{} / {name}\"", "created \"{} / {name}\"", self.cols[ci].name))
            }
            Action::UpdateRequest { collection, name, set } => {
                let ci = self.col(collection)?;
                let ri = self.req(ci, name)?;
                set.apply_to(&mut self.cols[ci].requests[ri]);
                if *self.cur == Some((ci, ri)) {
                    *self.ed = self.cols[ci].requests[ri].clone();
                }
                fx.cols = true;
                Ok(tr!("modificado \"{} / {}\"", "modified \"{} / {}\"", self.cols[ci].name, self.cols[ci].requests[ri].name))
            }
            Action::DeleteRequest { collection, name } => {
                let ci = self.col(collection)?;
                let ri = self.req(ci, name)?;
                self.cols[ci].requests.remove(ri);
                if matches!(*self.cur, Some((c, _)) if c == ci) {
                    *self.cur = None;
                }
                fx.cols = true;
                Ok(tr!("borrado \"{collection} / {name}\"", "deleted \"{collection} / {name}\""))
            }
            Action::DeleteCollection { name } => {
                let ci = self.col(name)?;
                self.cols.remove(ci);
                *self.cur = None;
                *self.col_sel = 0;
                fx.cols = true;
                Ok(tr!("colección \"{name}\" borrada", "collection \"{name}\" deleted"))
            }
            Action::SetVariables { scope, collection, environment, values, unset } => {
                let target: &mut Vars = match scope {
                    Scope::Collection => {
                        let ci = match collection {
                            Some(c) => self.col(c)?,
                            None => (*self.col_sel < self.cols.len()).then_some(*self.col_sel).ok_or(t("no hay colección activa", "there is no active collection"))?,
                        };
                        fx.cols = true;
                        &mut self.cols[ci].variables
                    }
                    Scope::Environment => {
                        let name = environment.clone().unwrap_or_else(|| self.envs.active.clone());
                        fx.envs = true;
                        self.envs.envs.get_mut(&name).ok_or_else(|| {
                            if name.is_empty() { t("no hay entorno activo: usa create_environment", "there is no active environment: use create_environment").to_string() } else { tr!("no existe el entorno \"{name}\"", "the environment \"{name}\" does not exist") }
                        })?
                    }
                };
                target.extend(to_vars(values));
                for k in unset {
                    target.remove(k);
                }
                Ok(tr!("variables: {} asignadas, {} quitadas", "variables: {} set, {} removed", values.len(), unset.len()))
            }
            Action::CreateEnvironment { name, values, activate } => {
                self.envs.envs.entry(name.trim().to_string()).or_default().extend(to_vars(values));
                if *activate || self.envs.active.is_empty() {
                    self.envs.active = name.trim().to_string();
                }
                fx.envs = true;
                Ok(tr!("entorno \"{name}\" listo", "environment \"{name}\" ready"))
            }
            Action::SetCollectionAuth { collection, auth } => {
                let ci = self.col(collection)?;
                self.cols[ci].auth = auth.clone();
                fx.cols = true;
                Ok(tr!("auth de \"{}\": {}", "auth of \"{}\": {}", self.cols[ci].name, auth_kind(auth)))
            }
            Action::SelectEnvironment { name } => {
                if !self.envs.envs.contains_key(name) {
                    return Err(tr!("no existe el entorno \"{name}\"", "the environment \"{name}\" does not exist"));
                }
                self.envs.active = name.clone();
                fx.envs = true;
                Ok(tr!("entorno \"{name}\" activo", "environment \"{name}\" active"))
            }
            Action::Send {} => {
                fx.send = true;
                Ok(t("enviando…", "sending…").into())
            }
            Action::CreateBattery { name, description, stages } => {
                check_stages(stages)?;
                let b = Battery { name: name.trim().to_string(), description: description.clone(), stages: stages.clone() };
                match self.batteries.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&b.name)) {
                    Some(x) => *x = b,
                    None => self.batteries.push(b),
                }
                fx.settings = true;
                Ok(tr!("batería \"{name}\" lista ({})", "battery \"{name}\" ready ({})", stages.iter().map(Stage::label).collect::<Vec<_>>().join(" → ")))
            }
            Action::RunCollection { collection, iterations } => {
                let col = self.col(collection)?;
                if self.cols[col].requests.is_empty() {
                    return Err(tr!("\"{collection}\" no tiene requests", "\"{collection}\" has no requests"));
                }
                fx.launch = Some(Launch::Run { col, iterations: iterations.unwrap_or(1).clamp(1, 1000) });
                Ok(tr!("corriendo \"{}\"…", "running \"{}\"…", self.cols[col].name))
            }
            Action::RunStress { collection, request, setup, battery, stages } => {
                let col = self.col(collection)?;
                let target = request.as_deref().map(|r| self.req(col, r)).transpose()?;
                let setup = setup.as_deref().map(|r| self.req(col, r)).transpose()?;
                let (label, stages) = match (battery, stages) {
                    (Some(b), None) => {
                        let found = self.batteries.iter().find(|x| x.name.eq_ignore_ascii_case(b.trim()));
                        let b = found.ok_or_else(|| tr!("no existe la batería \"{b}\" (hay: {})", "the battery \"{b}\" does not exist (available: {})", self.batteries_list()))?;
                        (b.name.clone(), b.stages.clone())
                    }
                    (None, Some(s)) => {
                        check_stages(s)?;
                        (t("Personalizada", "Custom").to_string(), s.clone())
                    }
                    _ => return Err(tr!("indica battery (hay: {}) o stages, no ambos", "give battery (available: {}) or stages, not both", self.batteries_list())),
                };
                fx.launch = Some(Launch::Stress { col, target, setup, stages, label: label.clone() });
                Ok(tr!("estresando \"{}\" con {label}…", "stressing \"{}\" with {label}…", self.cols[col].name))
            }
            Action::ImportOpenapi { url, spec } => {
                let source = match (url, spec) {
                    (Some(u), None) if !u.trim().is_empty() => u.trim().to_string(),
                    (None, Some(s)) if !s.trim().is_empty() => s.clone(),
                    _ => return Err(t("indica url o spec (uno de los dos)", "give url or spec (one of them)").into()),
                };
                fx.launch = Some(Launch::Import(source));
                Ok(t("importando el contrato…", "importing the contract…").into())
            }
        }
    }
}

/// Tipo de auth sin sus secretos (para mostrar y para el contexto del agente).
pub fn auth_kind(a: &Auth) -> &'static str {
    match a {
        Auth::Inherit => t("heredada", "inherited"),
        Auth::None => t("sin auth", "no auth"),
        Auth::Bearer { .. } => "Bearer",
        Auth::Basic { .. } => "Basic",
        Auth::ApiKey { .. } => "API Key",
        Auth::Oauth2(_) => "OAuth 2.0",
    }
}

/// " (token en host)" para OAuth 2.0: adónde irá tu client_secret.
fn token_host(a: &Auth) -> String {
    match a {
        Auth::Oauth2(o) => tr!(" (token en {})", " (token at {})", host_of(&o.token_url).unwrap_or_else(|| o.token_url.clone())),
        _ => String::new(),
    }
}

/// Etapas razonables: al menos una, 1..=MAX_VUS usuarios y 1..=3600 s cada una.
fn check_stages(stages: &[Stage]) -> Result<(), String> {
    if stages.is_empty() {
        return Err(t("stages está vacío", "stages is empty").into());
    }
    match stages.iter().find(|s| !(1..=MAX_VUS).contains(&s.vus) || !(1..=3600).contains(&s.secs)) {
        Some(s) => Err(tr!("etapa inválida {} VUs·{} s: usa 1..={MAX_VUS} VUs y 1..=3600 s", "invalid stage {} VUs·{} s: use 1..={MAX_VUS} VUs and 1..=3600 s", s.vus, s.secs)),
        None => Ok(()),
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
        let (mut cols, mut envs, mut ed, mut cur, mut sel, mut bats) = (Vec::new(), Environments::default(), Request::default(), None, 0, Vec::new());
        let mut ws = Ws { cols: &mut cols, batteries: &mut bats, envs: &mut envs, ed: &mut ed, cur: &mut cur, col_sel: &mut sel };
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
        assert_eq!(fx, Effects { cols: true, envs: true, send: true, ..Default::default() });

        // Estrés: batería nueva, ejecución con ella y validaciones.
        run(&mut ws, &mut fx, r#"{"action":"create_battery","name":"Carga 20","stages":[{"vus":20,"secs":10,"ramp":true},{"vus":20,"secs":60}]}"#).unwrap();
        run(&mut ws, &mut fx, r#"{"action":"run_stress","collection":"users","request":"Crear","battery":"carga 20"}"#).unwrap();
        assert!(matches!(&fx.launch, Some(Launch::Stress { col: 0, target: Some(0), setup: None, stages, label }) if stages.len() == 2 && label == "Carga 20"));
        assert!(run(&mut ws, &mut fx, r#"{"action":"run_stress","collection":"Users","battery":"Nope"}"#).unwrap_err().contains("hay: Carga 20"));
        assert!(run(&mut ws, &mut fx, r#"{"action":"create_battery","name":"X","stages":[{"vus":9999,"secs":10}]}"#).is_err());
        run(&mut ws, &mut fx, r#"{"action":"run_collection","collection":"Users","iterations":3}"#).unwrap();
        assert_eq!(fx.launch, Some(Launch::Run { col: 0, iterations: 3 }));
        assert!(fx.settings);

        let r = &cols[0].requests[0];
        assert_eq!((r.method.as_str(), r.headers.as_str(), r.body.as_str()), ("POST", "Content-Type: application/json", "{\n  \"n\": 1\n}"));
        assert_eq!(r.tests, "pm.test('ok', () => {})");
        assert_eq!(cols[0].requests.len(), 1, "save_request sobre el request abierto no duplica");
        assert_eq!(cols[0].variables.get("id").unwrap(), "7");
        assert_eq!((envs.active.as_str(), envs.envs["qa"]["port"].as_str()), ("qa", "8080"));
        assert_eq!(cur, Some((0, 0)));

        let mut ws = Ws { cols: &mut cols, batteries: &mut bats, envs: &mut envs, ed: &mut ed, cur: &mut cur, col_sel: &mut sel };
        run(&mut ws, &mut fx, r#"{"action":"set_collection_auth","collection":"Users","auth":{"type":"bearer","token":"{{token}}"}}"#).unwrap();
        run(&mut ws, &mut fx, r#"{"action":"update_request","collection":"Users","name":"Crear","set":{"auth":{"type":"api_key","key":"X-Key","value":"{{k}}"}}}"#).unwrap();
        assert_eq!(cols[0].auth, Auth::Bearer { token: "{{token}}".into() });
        assert!(matches!(&cols[0].requests[0].auth, Auth::ApiKey { key, .. } if key == "X-Key"));
    }

    #[test]
    fn destructive_and_hosts() {
        assert!(Action::DeleteCollection { name: "x".into() }.needs_approval());
        assert!(Action::RunStress { collection: "c".into(), request: None, setup: None, battery: None, stages: None }.needs_approval());
        assert!(!Action::Send {}.needs_approval());
        let with = |pre: &str| Action::EditRequest { set: Fields { pre: Some(pre.into()), ..Default::default() } };
        assert!(with("pm.sendRequest('https://x')").needs_approval() && !with("  ").needs_approval(), "scripts del agente se aprueban");
        let auth = Action::EditRequest { set: Fields { auth: Some(Auth::None), ..Default::default() } };
        assert!(auth.needs_approval(), "cambiar la auth se aprueba");
        assert_eq!(host_of("https://user:p@API.x.com:8443/a?b#c").as_deref(), Some("api.x.com:8443"));
        assert_eq!(host_of("{{base}}/x"), None);
    }
}
