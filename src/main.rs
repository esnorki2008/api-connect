#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use api_connect::{actions, agent, cookies, import, model, overlay, repo, runner, script, theme, widgets};
use widgets::{Lang, Row};

use std::{
    collections::HashSet,
    path::Path,
    time::Instant,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Relaxed},
        mpsc::{Receiver, TryRecvError, channel},
    },
};

use actions::{Action, Effects, Ws};

use eframe::egui::{self, Align, Button, Color32, Id, Key, Layout, Modifiers, RichText, ScrollArea, TextEdit, TextStyle};
use serde::de::DeserializeOwned;

use model::{
    AGENTS, AgentCfg, COLLECTIONS, Collection, ENVIRONMENTS, Environments, RepoCfg, Request, Response, SETTINGS, Settings,
    Source, Vars, resolve,
};
use runner::SendDone;
use script::{ReqParts, Scopes, TestResult};

const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
const HISTORY_MAX: usize = 100;
const VIEW_LIMIT: usize = 2_000_000; // ponytail: el visor de egui se vuelve lento con cuerpos enormes; se recorta
const REPO_CACHE: &str = "repos"; // clones dispersos de los repos, junto a los JSON de la app
const AGENT_ROUNDS: u8 = 5; // rondas automáticas (acción -> resultado -> agente) por pregunta

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("api-connect").with_inner_size([1300.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native(
        "api-connect",
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            let app = App::new();
            apply_theme(&cc.egui_ctx, app.settings.light_theme);
            Ok(Box::new(app))
        }),
    )
}

/// Segoe UI y Consolas del sistema leen mejor que la letra delgada por defecto; las de egui quedan de respaldo
/// para símbolos (✔ ✖ ⚡ ▶).
fn setup_fonts(ctx: &egui::Context) {
    use egui::FontFamily;
    let mut fonts = egui::FontDefinitions::default();
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    for (name, file, family) in [("segoe", "segoeui.ttf", FontFamily::Proportional), ("consolas", "consola.ttf", FontFamily::Monospace)] {
        if let Ok(bytes) = std::fs::read(Path::new(&dir).join("Fonts").join(file)) {
            fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
            fonts.families.entry(family).or_default().insert(0, name.into());
        }
    }
    ctx.set_fonts(fonts);
}

/// Tema Atom One Dark / One Light: fondos, texto, acentos de la paleta y letra más grande.
fn apply_theme(ctx: &egui::Context, light: bool) {
    use egui::{FontId, Stroke, TextStyle};
    theme::set_light(light);
    ctx.set_theme(if light { egui::ThemePreference::Light } else { egui::ThemePreference::Dark });
    let mut style = (*ctx.style()).clone();
    style.visuals = if light { egui::Visuals::light() } else { egui::Visuals::dark() };
    style.text_styles = [
        (TextStyle::Small, FontId::proportional(12.5)),
        (TextStyle::Body, FontId::proportional(14.5)),
        (TextStyle::Button, FontId::proportional(14.5)),
        (TextStyle::Heading, FontId::proportional(19.0)),
        (TextStyle::Monospace, FontId::monospace(13.5)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    style.spacing.interact_size.y = 24.0;

    let (panel, line, text, bright) = (theme::panel(), theme::line(), theme::text(), theme::bright());
    let v = &mut style.visuals;
    (v.panel_fill, v.window_fill, v.extreme_bg_color, v.faint_bg_color, v.code_bg_color) = (panel, theme::surface(), theme::field(), theme::faint(), theme::field());
    v.window_stroke = Stroke::new(1.0_f32, line);
    v.override_text_color = None;
    (v.hyperlink_color, v.warn_fg_color, v.error_fg_color) = (theme::accent(), theme::warn(), theme::err());
    v.selection.bg_fill = theme::selection();
    v.selection.stroke = Stroke::new(1.0_f32, bright);
    v.text_cursor.stroke = Stroke::new(2.0_f32, theme::accent());
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = panel;
    w.noninteractive.weak_bg_fill = panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, line);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, text);
    for (state, bg, border, fg) in [
        (&mut w.inactive, theme::faint(), line, text),
        (&mut w.hovered, theme::hover(), theme::accent(), bright),
        (&mut w.active, theme::active(), theme::accent(), bright),
        (&mut w.open, theme::hover(), line, bright),
    ] {
        (state.bg_fill, state.weak_bg_fill) = (bg, bg);
        state.bg_stroke = Stroke::new(1.0_f32, border);
        state.fg_stroke = Stroke::new(1.5_f32, fg);
    }
    ctx.set_style(style);
}

/// Colores por método HTTP, como en la mayoría de clientes de APIs.
fn method_color(m: &str) -> Color32 {
    match m.to_uppercase().as_str() {
        "GET" => theme::ok(),
        "POST" => theme::warn(),
        "PUT" => theme::accent(),
        "PATCH" => theme::purple(),
        "DELETE" => theme::err(),
        _ => theme::cyan(),
    }
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Tab {
    #[default]
    Params,
    Auth,
    Headers,
    Body,
    Pre,
    Tests,
    Contract,
}

#[derive(Clone, Copy, PartialEq, Default)]
enum RespTab {
    #[default]
    Body,
    Headers,
    Cookies,
    Tests,
    Console,
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Side {
    #[default]
    Cols,
    Envs,
    History,
}

#[derive(Clone)]
enum Confirm {
    Env(String),
    Col(usize),
    Req(usize, usize),
    CloseTab(usize),
}

#[derive(Clone, Copy)]
enum AuthTarget {
    /// Pestaña de request (índice en `tabs`).
    Tab(usize),
    Collection(usize),
}

/// Qué pidió el editor de auth.
#[derive(Default)]
struct AuthUi {
    get_token: bool,
    edit_collection: bool,
}

fn auth_label(a: &model::Auth) -> &'static str {
    match a {
        model::Auth::Inherit => "Heredar de la colección",
        model::Auth::None => "Sin auth",
        model::Auth::Bearer { .. } => "Bearer token",
        model::Auth::Basic { .. } => "Basic auth",
        model::Auth::ApiKey { .. } => "API Key",
        model::Auth::Oauth2(_) => "OAuth 2.0",
    }
}

/// Campo con etiqueta de ancho fijo. Los secretos se ocultan.
fn auth_field(ui: &mut egui::Ui, label: &str, value: &mut String, secret: bool, hint: &str) -> bool {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(130.0, 22.0), Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_width(130.0);
            ui.label(RichText::new(label).color(theme::muted()));
        });
        let mut lay = widgets::line_layouter(Lang::Plain, 13.5);
        let mut edit = TextEdit::singleline(value).hint_text(hint).font(TextStyle::Monospace).desired_width(ui.available_width().min(560.0));
        edit = if secret { edit.password(true) } else { edit.layouter(&mut lay) };
        ui.add(edit).changed()
    })
    .inner
}

/// Editor de autorización (request o colección). `inherited` = lo que heredaría un request.
fn auth_editor(ui: &mut egui::Ui, auth: &mut model::Auth, in_collection: bool, inherited: Option<(&str, &model::Auth)>, fetching: bool) -> (bool, AuthUi) {
    use model::Auth;
    let mut changed = false;
    let mut out = AuthUi::default();
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(130.0, 22.0), Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_width(130.0);
            ui.label(RichText::new("Tipo").color(theme::muted()));
        });
        let mut options = vec![Auth::None, Auth::Bearer { token: String::new() }, Auth::Basic { user: String::new(), pass: String::new() }];
        options.push(Auth::ApiKey { key: "X-API-Key".into(), value: String::new(), in_query: false });
        options.push(Auth::Oauth2(model::OAuth2 { grant: "client_credentials".into(), callback_url: api_connect::auth::DEFAULT_CALLBACK.into(), ..Default::default() }));
        if !in_collection {
            options.insert(0, Auth::Inherit);
        }
        egui::ComboBox::from_id_salt("auth_type").width(240.0).selected_text(auth_label(auth)).show_ui(ui, |ui| {
            for o in options {
                let same = std::mem::discriminant(&o) == std::mem::discriminant(auth);
                if ui.selectable_label(same, auth_label(&o)).clicked() && !same {
                    *auth = o;
                    changed = true;
                }
            }
        });
    });
    ui.add_space(6.0);
    match auth {
        Auth::Inherit => {
            let (col, a) = inherited.unwrap_or(("(sin colección)", &Auth::None));
            ui.label(RichText::new(format!("Usa la autorización de la colección «{col}»: {}.", auth_label(a))).color(theme::muted()));
            if ui.button("Editar la autorización de la colección").clicked() {
                out.edit_collection = true;
            }
        }
        Auth::None => {
            ui.label(RichText::new(if in_collection { "Los requests que hereden no llevarán autorización." } else { "Este request se envía sin autorización." }).color(theme::muted()));
        }
        Auth::Bearer { token } => {
            changed |= auth_field(ui, "Token", token, false, "{{token}}");
            ui.label(RichText::new("Se envía como Authorization: Bearer <token>.").small().color(theme::muted()));
        }
        Auth::Basic { user, pass } => {
            changed |= auth_field(ui, "Usuario", user, false, "{{usuario}}");
            changed |= auth_field(ui, "Contraseña", pass, true, "");
        }
        Auth::ApiKey { key, value, in_query } => {
            changed |= auth_field(ui, "Clave", key, false, "X-API-Key");
            changed |= auth_field(ui, "Valor", value, false, "{{apiKey}}");
            ui.horizontal(|ui| {
                ui.add_space(138.0);
                changed |= ui.radio_value(in_query, false, "En un header").changed();
                changed |= ui.radio_value(in_query, true, "En la query de la URL").changed();
            });
        }
        Auth::Oauth2(o) => {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(egui::vec2(130.0, 22.0), Layout::left_to_right(Align::Center), |ui| {
                    ui.set_min_width(130.0);
                    ui.label(RichText::new("Grant type").color(theme::muted()));
                });
                let shown = api_connect::auth::GRANTS.iter().find(|g| g.0 == o.grant).map_or("Client credentials", |g| g.1);
                egui::ComboBox::from_id_salt("grant").width(240.0).selected_text(shown).show_ui(ui, |ui| {
                    for (k, label) in api_connect::auth::GRANTS {
                        changed |= ui.selectable_value(&mut o.grant, k.to_string(), label).changed();
                    }
                });
            });
            let code = o.grant == "authorization_code";
            if code {
                changed |= auth_field(ui, "Auth URL", &mut o.auth_url, false, "https://login.ejemplo.com/oauth2/authorize");
            }
            changed |= auth_field(ui, "Token URL", &mut o.token_url, false, "https://login.ejemplo.com/oauth2/token");
            if code {
                changed |= auth_field(ui, "Callback URL", &mut o.callback_url, false, api_connect::auth::DEFAULT_CALLBACK);
            }
            changed |= auth_field(ui, "Client ID", &mut o.client_id, false, "{{clientId}}");
            changed |= auth_field(ui, "Client secret", &mut o.client_secret, true, "vacío si es un cliente público");
            changed |= auth_field(ui, "Scope", &mut o.scope, false, "api.read api.write");
            if o.grant == "password" {
                changed |= auth_field(ui, "Usuario", &mut o.username, false, "{{usuario}}");
                changed |= auth_field(ui, "Contraseña", &mut o.password, true, "");
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(138.0);
                let now = api_connect::auth::now();
                if o.access_token.is_empty() {
                    widgets::pill(ui, "sin token", theme::muted());
                } else if o.expires_at == 0 {
                    widgets::pill(ui, "token sin vencimiento conocido", theme::ok());
                } else if o.expires_at > now {
                    widgets::pill(ui, format!("vigente {} min", (o.expires_at - now) / 60), theme::ok());
                } else {
                    widgets::pill(ui, "vencido", theme::warn());
                }
                let label = if fetching { "Obteniendo…" } else { "Obtener token" };
                if ui.add_enabled(!fetching, Button::new(RichText::new(label).strong().color(theme::on_accent())).fill(theme::accent())).clicked() {
                    out.get_token = true;
                }
                if !o.access_token.is_empty() && ui.button("Borrar token").clicked() {
                    (o.access_token, o.refresh_token, o.expires_at) = (String::new(), String::new(), 0);
                    changed = true;
                }
            });
            let note = if code {
                "Abre tu navegador para iniciar sesión (PKCE). Registra la Callback URL en el proveedor. Con refresh token se renueva solo al enviar."
            } else {
                "Al enviar se pide o renueva el token solo cuando hace falta (también en el Runner y la consola)."
            };
            ui.label(RichText::new(note).small().color(theme::muted()));
        }
    }
    (changed, out)
}

/// Pestaña de request que no está activa (la activa vive en los campos de App: ed, cur, resp…).
#[derive(Default)]
struct TabState {
    cur: Option<(usize, usize)>,
    /// Para volver a ubicar el request si se borró algo antes que él.
    col_name: String,
    col_sel: usize,
    ed: Request,
    resp: Option<Result<Response, String>>,
    resp_view: String,
    tests: Vec<TestResult>,
    logs: Vec<String>,
    tab: Tab,
}

struct Hist {
    req: Request,
    status: Option<u16>,
    ms: u64,
    at: Instant,
}

/// Cambios sin guardar: distinto del guardado, o un request nuevo con algo escrito.
fn is_dirty(cols: &[Collection], cur: Option<(usize, usize)>, ed: &Request) -> bool {
    match cur {
        Some((c, i)) => cols.get(c).and_then(|c| c.requests.get(i)) != Some(ed),
        None => !ed.url.trim().is_empty() || !ed.body.trim().is_empty(),
    }
}

fn vars_rows(v: Option<&Vars>) -> Vec<Row> {
    v.map(|v| v.iter().map(|(k, v)| Row { on: true, key: k.clone(), value: v.clone() }).collect()).unwrap_or_default()
}

fn rows_vars(rows: &[Row]) -> Vars {
    rows.iter().filter(|r| !r.key.trim().is_empty()).map(|r| (r.key.trim().to_string(), r.value.clone())).collect()
}

struct ChatMsg {
    /// "user" | "assistant" | "tool" (resultado de acciones; al agente le llega como mensaje de usuario)
    role: &'static str,
    shown: String,
    sent: String,
    error: bool,
}

/// Estado previo a una pregunta, para deshacer lo que hizo el agente.
struct Snapshot {
    cols: Vec<Collection>,
    envs: Environments,
    ed: Request,
    cur: Option<(usize, usize)>,
    col_sel: usize,
}

/// Diferencias con el contrato y avisos de validación del request abierto (se recalculan solo si cambia).
struct ContractView {
    parts: ReqParts,
    key: String,
    changes: Vec<overlay::Change>,
    issues: Vec<String>,
}

/// Envío en curso, con el entorno y la colección a los que deben volver las variables que cambien los scripts.
struct Pending {
    rx: Receiver<SendDone>,
    req: Request,
    env: String,
    col: Option<(usize, String)>,
}

enum RunMsg {
    Step(runner::Step),
    Progress(String),
    RunDone(Scopes),
    StressDone(Result<Vec<runner::StageReport>, String>),
}

/// Ventana del runner de colecciones y de las baterías de estrés.
#[derive(Default)]
struct RunPanel {
    open: bool,
    stress: bool,
    col: usize,
    iterations: u32,
    bail: bool,
    /// Endpoint a estresar; None = toda la colección.
    target: Option<usize>,
    /// Índice en settings.batteries; si es igual a su largo, es la personalizada.
    battery: usize,
    vus: u32,
    secs: u64,
    /// Rampa previa de la batería personalizada (0 = sin rampa).
    ramp_secs: u64,
    /// Request de preparación (login) que se envía una vez antes de medir.
    setup: Option<usize>,
    /// Filas del archivo de datos del runner (una por iteración).
    data: Vec<Vars>,
    data_name: String,
    rx: Option<Receiver<RunMsg>>,
    stop: Arc<AtomicBool>,
    /// Entorno y colección a los que vuelven las variables que cambien los scripts del runner.
    dest: (String, Option<(usize, String)>),
    steps: Vec<runner::Step>,
    progress: String,
    reports: Vec<runner::StageReport>,
    err: String,
}

struct App {
    client: reqwest::blocking::Client,
    cols: Vec<Collection>,
    envs: Environments,
    agents: Vec<AgentCfg>,
    settings: Settings,
    blocked: Vec<&'static str>, // archivos que no se pudieron leer: no se sobrescriben
    error: String,

    cur: Option<(usize, usize)>, // request abierto: (colección, índice)
    ed: Request,
    col_sel: usize,
    tab: Tab,
    side: Side,
    filter: String,
    /// Entorno que se edita en la sección Entornos (no siempre el activo).
    env_edit: String,
    env_rows: Vec<Row>,
    /// Ventana de variables de colección abierta y su tabla.
    vars_win: Option<usize>,
    /// Ventana de autorización de una colección.
    auth_win: Option<usize>,
    show_cookies: bool,
    /// Formulario "agregar cookie": dominio, nombre, valor.
    new_cookie: (String, String, String),
    /// "Obtener token" de OAuth 2.0 en curso y a quién va el token.
    oauth_pending: Option<(Receiver<Result<api_connect::auth::Token, String>>, AuthTarget)>,
    col_rows: Vec<Row>,
    /// Headers como texto en vez de tabla.
    bulk: bool,
    tabs: Vec<TabState>,
    active_tab: usize,
    history: Vec<Hist>,
    resp_tab: RespTab,
    raw: bool,
    new_env: String,
    new_col: String,

    sending: Option<Pending>,
    resp: Option<Result<Response, String>>,
    resp_view: String,
    tests: Vec<TestResult>,
    logs: Vec<String>,

    show_import: bool,
    import_text: String,
    import_keep_scripts: bool,
    import_err: String,
    confirm: Option<Confirm>,

    show_agent: bool,
    agent_sel: usize,
    chat: Vec<ChatMsg>,
    question: String,
    with_context: bool,
    asking: Option<Receiver<Result<String, String>>>,
    show_agents_cfg: bool,
    agents_json: String,
    agents_err: String,
    timeout_edit: u64,

    show_repos: bool,
    run: RunPanel,
    repo_url: String,
    repo_branch: String,
    repo_scan: Option<repo::Scan>,
    /// Conexión en curso y, si vino de "Sincronizar" en una colección, cuál.
    repo_pending: Option<(Receiver<Result<repo::Scan, String>>, Option<usize>)>,
    repo_msg: String,
    notice: String,

    agent_rounds: u8,
    /// El agente pidió un envío y espera su resultado para seguir.
    agent_waits_send: bool,
    /// Acciones del agente que esperan aprobación (borrados o envío a un host nuevo).
    pending_actions: Vec<Action>,
    undo: Option<Snapshot>,
    /// Hosts a los que TÚ enviaste o que vienen de un contrato importado: el agente puede enviar ahí sin preguntar.
    trusted_hosts: HashSet<String>,
    contract_view: Option<ContractView>,
}

fn load_or<T: DeserializeOwned>(path: &'static str, default: impl FnOnce() -> T, errors: &mut Vec<String>, blocked: &mut Vec<&'static str>) -> T {
    match model::load(path) {
        Ok(v) => v.unwrap_or_else(default),
        Err(e) => {
            errors.push(format!("No se pudo leer {e}. No se guardarán cambios en ese archivo hasta corregirlo."));
            blocked.push(path);
            default()
        }
    }
}

impl App {
    fn new() -> Self {
        let (mut errors, mut blocked) = (Vec::new(), Vec::new());
        let cols = load_or(COLLECTIONS, Vec::new, &mut errors, &mut blocked);
        let envs = load_or(ENVIRONMENTS, Environments::default, &mut errors, &mut blocked);
        let agents = load_or(AGENTS, model::default_agents, &mut errors, &mut blocked);
        let settings = load_or(SETTINGS, Settings::default, &mut errors, &mut blocked);
        cookies::replace(load_or(cookies::FILE, Vec::new, &mut errors, &mut blocked));
        cookies::take_dirty();
        cookies::enable(true);
        let mut app = App {
            client: model::client(),
            cols,
            envs,
            agents,
            settings,
            blocked,
            error: errors.join("\n"),
            cur: None,
            ed: Request { method: "GET".into(), ..Default::default() },
            col_sel: 0,
            tab: Tab::Params,
            side: Side::Cols,
            filter: String::new(),
            env_edit: String::new(),
            env_rows: Vec::new(),
            vars_win: None,
            auth_win: None,
            show_cookies: false,
            new_cookie: Default::default(),
            oauth_pending: None,
            col_rows: Vec::new(),
            bulk: false,
            tabs: vec![TabState::default()],
            active_tab: 0,
            history: Vec::new(),
            resp_tab: RespTab::Body,
            raw: false,
            new_env: String::new(),
            new_col: String::new(),
            sending: None,
            resp: None,
            resp_view: String::new(),
            tests: Vec::new(),
            logs: Vec::new(),
            show_import: false,
            import_text: String::new(),
            import_keep_scripts: false,
            import_err: String::new(),
            confirm: None,
            show_agent: false,
            agent_sel: 0,
            chat: Vec::new(),
            question: String::new(),
            with_context: true,
            asking: None,
            show_agents_cfg: false,
            agents_json: String::new(),
            agents_err: String::new(),
            timeout_edit: 0,
            show_repos: false,
            run: RunPanel { iterations: 1, vus: 10, secs: 30, ..Default::default() },
            repo_url: String::new(),
            repo_branch: String::new(),
            repo_scan: None,
            repo_pending: None,
            repo_msg: String::new(),
            notice: String::new(),
            agent_rounds: 0,
            agent_waits_send: false,
            pending_actions: Vec::new(),
            undo: None,
            trusted_hosts: ["localhost", "127.0.0.1"].into_iter().map(String::from).collect(),
            contract_view: None,
        };
        app.sync_buffers();
        app.restore_tabs();
        app
    }

    /// Reabre las pestañas guardadas de la sesión anterior (las que ya no existen se ignoran).
    fn restore_tabs(&mut self) {
        let saved = std::mem::take(&mut self.settings.open_tabs);
        let mut first = true;
        for (cn, rn) in &saved {
            let Some(c) = self.cols.iter().position(|c| c.name == *cn) else { continue };
            let Some(r) = self.cols[c].requests.iter().position(|r| r.name == *rn) else { continue };
            if !first {
                self.new_tab();
            }
            self.open(c, r);
            first = false;
        }
        self.switch_tab(self.settings.active_tab.min(self.tabs.len() - 1));
        self.settings.open_tabs = saved;
    }

    fn save_session(&mut self) {
        let name = |cols: &[Collection], cur: Option<(usize, usize)>| {
            let (c, r) = cur?;
            let col = cols.get(c)?;
            Some((col.name.clone(), col.requests.get(r)?.name.clone()))
        };
        let tabs: Vec<(String, String)> = (0..self.tabs.len())
            .filter_map(|i| name(&self.cols, if i == self.active_tab { self.cur } else { self.tabs[i].cur }))
            .collect();
        let active = name(&self.cols, self.cur).and_then(|a| tabs.iter().position(|t| *t == a)).unwrap_or(0);
        if tabs != self.settings.open_tabs || active != self.settings.active_tab {
            (self.settings.open_tabs, self.settings.active_tab) = (tabs, active);
            self.persist(SETTINGS);
        }
    }

    fn persist(&mut self, file: &'static str) {
        if self.blocked.contains(&file) {
            self.error = format!("{file} no se pudo leer al iniciar; no se sobrescribe. Corrígelo y reinicia la app.");
            return;
        }
        let r = match file {
            COLLECTIONS => model::save(file, &self.cols),
            ENVIRONMENTS => model::save(file, &self.envs),
            SETTINGS => model::save(file, &self.settings),
            cookies::FILE => model::save(file, &cookies::snapshot()),
            _ => model::save(file, &self.agents),
        };
        if let Err(e) = r {
            self.error = e;
        }
    }

    fn active_col(&self) -> Option<usize> {
        (self.col_sel < self.cols.len()).then_some(self.col_sel)
    }

    /// Rellena los editores de variables desde el modelo (al cambiar de entorno/colección o tras un envío).
    fn sync_buffers(&mut self) {
        if !self.envs.envs.contains_key(&self.env_edit) {
            self.env_edit = self.envs.active.clone();
        }
        self.env_rows = vars_rows(self.envs.envs.get(&self.env_edit));
        self.col_rows = vars_rows(self.vars_win.and_then(|i| self.cols.get(i)).map(|c| &c.variables));
    }

    // ---- Pestañas de requests ----

    /// Envío o agente en curso: sus resultados van a la pestaña activa, así que no se cambia de pestaña.
    fn busy(&self) -> bool {
        self.sending.is_some() || self.asking.is_some() || self.agent_waits_send
    }

    fn stash(&mut self) -> TabState {
        TabState {
            cur: self.cur,
            col_name: self.cur.and_then(|(c, _)| self.cols.get(c)).map(|c| c.name.clone()).unwrap_or_default(),
            col_sel: self.col_sel,
            ed: std::mem::take(&mut self.ed),
            resp: self.resp.take(),
            resp_view: std::mem::take(&mut self.resp_view),
            tests: std::mem::take(&mut self.tests),
            logs: std::mem::take(&mut self.logs),
            tab: self.tab,
        }
    }

    fn unstash(&mut self, t: TabState) {
        // Si se borró algo antes que el request, su índice cambió: se vuelve a ubicar por nombres.
        self.cur = t.cur.and_then(|(c, i)| {
            let same = self.cols.get(c).is_some_and(|col| col.name == t.col_name && col.requests.get(i).is_some_and(|r| r.name == t.ed.name));
            if same {
                return Some((c, i));
            }
            let c = self.cols.iter().position(|col| col.name == t.col_name)?;
            Some((c, self.cols[c].requests.iter().position(|r| r.name == t.ed.name)?))
        });
        self.col_sel = self.cur.map_or(t.col_sel, |(c, _)| c);
        (self.ed, self.resp, self.resp_view, self.tests, self.logs, self.tab) = (t.ed, t.resp, t.resp_view, t.tests, t.logs, t.tab);
        if self.ed.method.is_empty() {
            self.ed.method = "GET".into();
        }
        self.contract_view = None;
    }

    fn switch_tab(&mut self, i: usize) {
        if i == self.active_tab || i >= self.tabs.len() || self.busy() {
            return;
        }
        let cur = self.stash();
        self.tabs[self.active_tab] = cur;
        let t = std::mem::take(&mut self.tabs[i]);
        self.unstash(t);
        self.active_tab = i;
        self.sync_buffers();
    }

    fn new_tab(&mut self) {
        if self.busy() {
            return;
        }
        self.tabs.push(TabState { col_sel: self.col_sel, ..Default::default() });
        self.switch_tab(self.tabs.len() - 1);
    }

    fn close_tab(&mut self, i: usize) {
        if self.busy() || i >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            self.new_request();
            return;
        }
        if i == self.active_tab {
            self.switch_tab(if i + 1 < self.tabs.len() { i + 1 } else { i - 1 });
        }
        self.tabs.remove(i);
        if self.active_tab > i {
            self.active_tab -= 1;
        }
    }

    fn request_close_tab(&mut self, i: usize) {
        let dirty = if i == self.active_tab { is_dirty(&self.cols, self.cur, &self.ed) } else { self.tabs.get(i).is_some_and(|t| is_dirty(&self.cols, t.cur, &t.ed)) };
        if dirty {
            self.confirm = Some(Confirm::CloseTab(i));
        } else {
            self.close_tab(i);
        }
    }

    /// Ajusta el request abierto tras borrar un request (`r`) o una colección entera (`r` = None).
    fn after_delete(&mut self, c: usize, r: Option<usize>) {
        self.cur = match (self.cur, r) {
            (Some((cc, _)), None) if cc == c => None,
            (Some((cc, i)), None) if cc > c => Some((cc - 1, i)),
            (Some((cc, i)), Some(r)) if cc == c && i == r => None,
            (Some((cc, i)), Some(r)) if cc == c && i > r => Some((cc, i - 1)),
            (cur, _) => cur,
        };
        if r.is_none() && self.col_sel >= c && self.col_sel > 0 {
            self.col_sel -= 1;
        }
        self.persist(COLLECTIONS);
        self.sync_buffers();
    }

    /// Mueve un request (arrastrar y soltar) y corrige a qué apuntan las pestañas abiertas.
    fn move_request(&mut self, from: (usize, usize), to_col: usize, to_idx: usize) {
        let Some(remap) = model::move_request(&mut self.cols, from, to_col, to_idx) else { return };
        self.cur = self.cur.map(&remap);
        if let Some((c, _)) = self.cur {
            self.col_sel = c;
        }
        for t in &mut self.tabs {
            t.cur = t.cur.map(&remap);
            if let Some((c, _)) = t.cur {
                t.col_name = self.cols[c].name.clone();
            }
        }
        self.persist(COLLECTIONS);
        self.sync_buffers();
    }

    /// Abre en la pestaña que ya lo tenga, en la actual si no tiene cambios, o en una nueva.
    fn open(&mut self, c: usize, r: usize) {
        if !self.busy() {
            if let Some(i) = (0..self.tabs.len()).find(|&i| i != self.active_tab && self.tabs[i].cur == Some((c, r))) {
                self.switch_tab(i);
                return;
            }
            if self.cur != Some((c, r)) && is_dirty(&self.cols, self.cur, &self.ed) {
                self.new_tab();
            }
        }
        if self.cur != Some((c, r)) {
            (self.resp, self.tests, self.logs) = (None, Vec::new(), Vec::new());
            self.resp_view.clear();
        }
        self.cur = Some((c, r));
        self.ed = self.cols[c].requests[r].clone();
        if self.ed.method.is_empty() {
            self.ed.method = "GET".into();
        }
        self.col_sel = c;
        self.sync_buffers();
    }

    fn new_request(&mut self) {
        self.cur = None;
        self.ed = Request { method: "GET".into(), ..Default::default() };
        (self.resp, self.tests, self.logs) = (None, Vec::new(), Vec::new());
        self.resp_view.clear();
    }

    fn save_request(&mut self) {
        if self.cols.is_empty() {
            self.cols.push(Collection { name: "General".into(), ..Default::default() });
            self.col_sel = 0;
        }
        let ci = self.active_col().unwrap_or(0);
        if self.ed.name.trim().is_empty() {
            self.ed.name = if self.ed.url.is_empty() { "Sin nombre".into() } else { self.ed.url.clone() };
        }
        match self.cur {
            Some((c, i)) if c == ci && i < self.cols[c].requests.len() => self.cols[c].requests[i] = self.ed.clone(),
            _ => {
                self.cols[ci].requests.push(self.ed.clone());
                self.cur = Some((ci, self.cols[ci].requests.len() - 1));
            }
        }
        self.persist(COLLECTIONS);
        self.sync_buffers();
    }

    // ---- Envío ----

    /// Variables visibles para el request abierto (colección < entorno), sin las locales del script.
    fn visible_vars(&self) -> Vars {
        let mut v = self.active_col().map(|i| self.cols[i].variables.clone()).unwrap_or_default();
        v.extend(self.envs.envs.get(&self.envs.active).cloned().unwrap_or_default());
        v
    }

    fn current_host(&self) -> Option<String> {
        actions::host_of(&resolve(&self.ed.url, &self.visible_vars()))
    }

    fn host_trusted(&self) -> bool {
        self.current_host().is_some_and(|h| self.trusted_hosts.contains(&h) || self.trusted_hosts.contains(h.split(':').next().unwrap_or("")))
    }

    /// Lo que tú envías marca el host como de confianza para el agente.
    fn send_by_user(&mut self, ctx: &egui::Context) {
        if let Some(h) = self.current_host() {
            self.trusted_hosts.insert(h);
        }
        self.send(ctx);
    }

    fn send(&mut self, ctx: &egui::Context) {
        if self.sending.is_some() {
            return;
        }
        let col = self.active_col();
        // Sin entorno activo, pm.environment.set no tiene dónde persistir: queda solo para este envío.
        let env = if self.envs.envs.contains_key(&self.envs.active) { self.envs.active.clone() } else { String::new() };
        let scopes = Scopes {
            env: self.envs.envs.get(&env).cloned().unwrap_or_default(),
            collection: col.map(|i| self.cols[i].variables.clone()).unwrap_or_default(),
            ..Default::default()
        };
        let (tx, rx) = channel();
        let ed = model::with_effective_auth(&self.ed, col.map(|i| &self.cols[i]));
        let (client, ctx) = (self.client.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(runner::run_send(&client, &ed, scopes));
            ctx.request_repaint();
        });
        self.sending = Some(Pending { rx, req: self.ed.clone(), env, col: col.map(|i| (i, self.cols[i].name.clone())) });
    }

    fn finish_send(&mut self, ctx: &egui::Context, p: Pending, d: SendDone) {
        self.tests = d.tests;
        self.logs = d.logs;
        self.resp_view = match &d.result {
            Ok(r) => {
                let pretty = serde_json::from_str::<serde_json::Value>(&r.body)
                    .and_then(|v| serde_json::to_string_pretty(&v))
                    .unwrap_or_else(|_| r.body.clone());
                if pretty.len() > VIEW_LIMIT {
                    let cut = (0..=VIEW_LIMIT).rev().find(|&i| pretty.is_char_boundary(i)).unwrap_or(0);
                    format!("{}\n\n…(respuesta recortada a {VIEW_LIMIT} bytes para mostrarla)", &pretty[..cut])
                } else {
                    pretty
                }
            }
            Err(e) => e.clone(),
        };
        let (status, ms) = d.result.as_ref().map_or((None, 0), |r| (Some(r.status), r.ms));
        self.history.insert(0, Hist { req: p.req, status, ms, at: Instant::now() });
        self.history.truncate(HISTORY_MAX);
        self.resp = Some(d.result);
        self.write_back(&p.env, p.col, d.scopes);

        if std::mem::take(&mut self.agent_waits_send) {
            let summary = match &self.resp {
                Some(Ok(r)) => format!("{} {} · {} ms\n{}", r.status, r.status_text, r.ms, cut(&r.body, 3000)),
                Some(Err(e)) => format!("falló: {e}"),
                None => String::new(),
            };
            let tests: Vec<String> = self
                .tests
                .iter()
                .map(|t| format!("{} {}{}", if t.pass { "✔" } else { "✖" }, t.name, t.error.as_ref().map(|e| format!(" — {e}")).unwrap_or_default()))
                .collect();
            let text = format!(
                "Resultado del envío: {summary}\nTests: {}",
                if tests.is_empty() { "(ninguno)".into() } else { tests.join("\n") }
            );
            self.chat.push(ChatMsg { role: "tool", shown: text.clone(), sent: text, error: false });
            self.continue_agent(ctx);
        }
    }

    /// Guarda las variables que cambiaron los scripts en su entorno y colección (si siguen existiendo).
    fn write_back(&mut self, env: &str, col: Option<(usize, String)>, scopes: Scopes) {
        if let Some(e) = self.envs.envs.get_mut(env) {
            if *e != scopes.env {
                *e = scopes.env;
                self.persist(ENVIRONMENTS);
            }
        }
        if let Some((i, name)) = col {
            if let Some(c) = self.cols.get_mut(i).filter(|c| c.name == name) {
                if c.variables != scopes.collection {
                    c.variables = scopes.collection;
                    self.persist(COLLECTIONS);
                }
            }
        }
        self.sync_buffers();
    }

    // ---- Runner y estrés ----

    fn open_runner(&mut self, ci: usize, stress: bool) {
        if self.run.rx.is_none() {
            if self.run.col != ci {
                (self.run.target, self.run.setup) = (None, None);
            }
            self.run.col = ci;
        }
        self.run.open = true;
        self.run.stress = stress;
    }

    fn start_run(&mut self, ctx: &egui::Context, stress: bool) {
        let Some(col) = self.cols.get(self.run.col).cloned() else { return };
        let env = if self.envs.envs.contains_key(&self.envs.active) { self.envs.active.clone() } else { String::new() };
        let scopes = Scopes { env: self.envs.envs.get(&env).cloned().unwrap_or_default(), collection: col.variables.clone(), ..Default::default() };
        let r = &mut self.run;
        (r.steps, r.reports, r.err, r.progress) = (Vec::new(), Vec::new(), String::new(), String::new());
        r.stop = Arc::new(AtomicBool::new(false));
        r.dest = (env, Some((r.col, col.name.clone())));
        let (tx, rx) = channel();
        r.rx = Some(rx);
        let (client, stop, ctx) = (self.client.clone(), r.stop.clone(), ctx.clone());
        if !stress {
            let (n, bail, data) = (r.iterations, r.bail, r.data.clone());
            std::thread::spawn(move || {
                let mut scopes = scopes;
                runner::run_collection(&client, &col, &mut scopes, n, &data, bail, &stop, &mut |s| {
                    let _ = tx.send(RunMsg::Step(s.clone()));
                    ctx.request_repaint();
                });
                let _ = tx.send(RunMsg::RunDone(scopes));
                ctx.request_repaint();
            });
            return;
        }
        let eff = |q: &Request| model::with_effective_auth(q, Some(&col));
        let setup: Vec<Request> = r.setup.and_then(|i| col.requests.get(i)).map(eff).into_iter().collect();
        let targets: Vec<Request> = match r.target {
            Some(i) => col.requests.get(i).map(eff).into_iter().collect(),
            // La preparación (login) no se martilla junto con el resto.
            None => col.requests.iter().enumerate().filter(|(i, _)| Some(*i) != r.setup).map(|(_, q)| eff(q)).collect(),
        };
        let stages = match self.settings.batteries.get(r.battery) {
            Some(b) => b.stages.clone(),
            None => {
                let ramp = (r.ramp_secs > 0).then_some(runner::Stage { vus: r.vus, secs: r.ramp_secs, ramp: true });
                ramp.into_iter().chain([runner::Stage { vus: r.vus, secs: r.secs, ramp: false }]).collect()
            }
        };
        std::thread::spawn(move || {
            let res = runner::stress(&client, &setup, &targets, scopes, &stages, &stop, &mut |p| {
                let text = format!(
                    "Etapa {}/{} · {} VUs · {}/{} s · {} requests · {} fallidos · {:.0} req/s",
                    p.stage + 1,
                    p.stages,
                    p.vus,
                    p.elapsed.as_secs(),
                    p.secs,
                    p.count,
                    p.failed,
                    p.count as f64 / p.elapsed.as_secs_f64().max(0.001)
                );
                let _ = tx.send(RunMsg::Progress(text));
                ctx.request_repaint();
            });
            let _ = tx.send(RunMsg::StressDone(res));
            ctx.request_repaint();
        });
    }

    fn poll_run(&mut self) {
        let Some(rx) = self.run.rx.take() else { return };
        loop {
            match rx.try_recv() {
                Ok(RunMsg::Step(s)) => self.run.steps.push(s),
                Ok(RunMsg::Progress(p)) => self.run.progress = p,
                Ok(RunMsg::RunDone(scopes)) => {
                    let (env, col) = self.run.dest.clone();
                    self.write_back(&env, col, scopes);
                    return;
                }
                Ok(RunMsg::StressDone(r)) => {
                    match r {
                        Ok(reports) => self.run.reports = reports,
                        Err(e) => self.run.err = e,
                    }
                    return;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.run.err = "la ejecución terminó inesperadamente".into();
                    return;
                }
            }
        }
        self.run.rx = Some(rx);
    }

    /// "Obtener token": corre fuera de la UI (authorization code espera al navegador hasta 3 minutos).
    fn fetch_token(&mut self, ctx: &egui::Context, target: AuthTarget) {
        let cfg = match target {
            AuthTarget::Tab(_) => &self.ed.auth,
            AuthTarget::Collection(ci) => match self.cols.get(ci) {
                Some(c) => &c.auth,
                None => return,
            },
        };
        let model::Auth::Oauth2(o) = cfg.clone() else { return };
        let mut vars = match target {
            AuthTarget::Collection(ci) => self.cols[ci].variables.clone(),
            AuthTarget::Tab(_) => self.active_col().map(|i| self.cols[i].variables.clone()).unwrap_or_default(),
        };
        vars.extend(self.envs.envs.get(&self.envs.active).cloned().unwrap_or_default());
        let (tx, rx) = channel();
        let (client, ctx) = (self.client.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(api_connect::auth::fetch_token(&client, &o, &vars));
            ctx.request_repaint();
        });
        self.oauth_pending = Some((rx, target));
    }

    fn finish_token(&mut self, res: Result<api_connect::auth::Token, String>, target: AuthTarget) {
        let t = match res {
            Ok(t) => t,
            Err(e) => {
                self.error = e;
                return;
            }
        };
        let auth = match target {
            AuthTarget::Tab(i) if i == self.active_tab => Some(&mut self.ed.auth),
            AuthTarget::Tab(i) => self.tabs.get_mut(i).map(|t| &mut t.ed.auth),
            AuthTarget::Collection(ci) => self.cols.get_mut(ci).map(|c| &mut c.auth),
        };
        if let Some(model::Auth::Oauth2(o)) = auth {
            api_connect::auth::store(o, &t);
            self.notice = "Token de OAuth 2.0 obtenido.".into();
        }
        match target {
            AuthTarget::Collection(_) => self.persist(COLLECTIONS),
            // El token queda en el request abierto; si ya estaba guardado, se guarda también.
            AuthTarget::Tab(i) if i == self.active_tab => {
                if let Some((c, r)) = self.cur {
                    let mut saved = self.cols[c].requests[r].clone();
                    saved.auth = self.ed.auth.clone();
                    if saved == self.ed {
                        self.cols[c].requests[r] = saved;
                        self.persist(COLLECTIONS);
                    }
                }
            }
            AuthTarget::Tab(_) => {}
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some((rx, target)) = self.oauth_pending.take() {
            match rx.try_recv() {
                Ok(r) => self.finish_token(r, target),
                Err(TryRecvError::Empty) => self.oauth_pending = Some((rx, target)),
                Err(TryRecvError::Disconnected) => self.error = "la obtención del token falló inesperadamente".into(),
            }
        }
        self.poll_run();
        if let Some(p) = self.sending.take() {
            match p.rx.try_recv() {
                Ok(done) => self.finish_send(ctx, p, done),
                Err(TryRecvError::Empty) => self.sending = Some(p),
                Err(TryRecvError::Disconnected) => self.resp = Some(Err("el envío falló inesperadamente".into())),
            }
        }
        if let Some((rx, col)) = self.repo_pending.take() {
            match rx.try_recv() {
                Ok(r) => self.finish_repo(r, col),
                Err(TryRecvError::Empty) => self.repo_pending = Some((rx, col)),
                Err(TryRecvError::Disconnected) => self.repo_msg = "la conexión con el repo falló inesperadamente".into(),
            }
        }
        if let Some(rx) = self.asking.take() {
            let reply = match rx.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Empty) => {
                    self.asking = Some(rx);
                    return;
                }
                Err(TryRecvError::Disconnected) => Err("el agente falló inesperadamente".into()),
            };
            let (text, error) = match reply {
                Ok(t) if t.trim().is_empty() => ("(sin respuesta)".to_string(), false),
                Ok(t) => (t, false),
                Err(e) => (e, true),
            };
            self.chat.push(ChatMsg { role: "assistant", shown: text.clone(), sent: text.clone(), error });
            if !error {
                self.run_actions(ctx, actions::extract(&text), false);
            }
        }
    }

    // ---- Agente ----

    /// Solo nombres de variables: los valores (tokens, claves) no salen de la máquina.
    fn agent_context(&self) -> String {
        let or = |s: &str, empty: &str| if s.trim().is_empty() { empty.to_string() } else { s.to_string() };
        let keys = |v: Option<&Vars>| v.map(|v| v.keys().cloned().collect::<Vec<_>>().join(", ")).filter(|s| !s.is_empty()).unwrap_or("(ninguna)".into());
        let last = match &self.resp {
            Some(Ok(r)) => format!("Última respuesta: {} {}\n{}", r.status, r.status_text, cut(&r.body, 3000)),
            Some(Err(e)) => format!("Último envío falló: {e}"),
            None => "Aún no hay respuesta.".into(),
        };
        let mut workspace: Vec<String> = Vec::new();
        for c in &self.cols {
            let src = c.source.as_ref().map(|s| format!(" [contrato: {}]", s.file)).unwrap_or_default();
            workspace.push(format!("- {}{src} ({} requests)", c.name, c.requests.len()));
            for r in c.requests.iter().take(40) {
                workspace.push(format!("    {} {}", r.method, r.name));
            }
        }
        let open = match self.cur {
            Some((c, i)) => format!("\"{} / {}\"", self.cols[c].name, self.cols[c].requests[i].name),
            None => "(nuevo, sin guardar)".into(),
        };
        let mut parts = vec![
            format!("Request abierto {open}: {} {}", self.ed.method, self.ed.url),
            format!("Headers:\n{}", or(&self.ed.headers, "(ninguno)")),
            format!("Body:\n{}", or(&cut(&self.ed.body, 2000), "(vacío)")),
            format!("Pre-request:\n{}", or(&self.ed.pre, "(vacío)")),
            format!("Tests:\n{}", or(&self.ed.tests, "(vacío)")),
        ];
        if let Some(o) = &self.ed.origin {
            let changes: Vec<String> = overlay::diff(&o.base, &self.ed.parts())
                .iter()
                .map(|c| format!("- {}: contrato {} → tuyo {}", c.path, c.contract.as_deref().unwrap_or("(no existe)"), c.yours.as_deref().unwrap_or("(quitado)")))
                .collect();
            let issues = overlay::validate(&self.ed.body, &o.schema);
            parts.push(format!(
                "Este request viene del CONTRATO ({}), que es de solo lectura; lo que editas es la versión del usuario.\nCambios del usuario vs contrato:\n{}\nAvisos de validación:\n{}\nSchema del body según el contrato:\n{}",
                o.key,
                if changes.is_empty() { "(ninguno)".into() } else { changes.join("\n") },
                if issues.is_empty() { "(ninguno)".into() } else { issues.join("\n") },
                if o.schema.is_null() { "(sin schema)".into() } else { cut(&o.schema.to_string(), 3000) },
            ));
        }
        parts.extend([
            format!("Entornos: {} (activo: {})", self.envs.envs.keys().cloned().collect::<Vec<_>>().join(", "), or(&self.envs.active, "ninguno")),
            format!("Variables de entorno disponibles: {}", keys(self.envs.envs.get(&self.envs.active))),
            format!("Colección activa: {}", self.active_col().map_or("(ninguna)".into(), |i| self.cols[i].name.clone())),
            format!("Variables de colección disponibles: {}", keys(self.active_col().map(|i| &self.cols[i].variables))),
            format!("Colecciones:\n{}", if workspace.is_empty() { "(ninguna)".into() } else { cut(&workspace.join("\n"), 4000) }),
            last,
        ]);
        parts.join("\n\n")
    }

    /// Pregunta del usuario: arranca un ciclo nuevo del agente (rondas y punto de deshacer).
    fn ask(&mut self, ctx: &egui::Context) {
        let q = self.question.trim().to_string();
        if q.is_empty() || self.asking.is_some() || self.agents.get(self.agent_sel).is_none() {
            return;
        }
        let sent = if self.with_context { format!("{q}\n\n---\nContexto:\n{}", self.agent_context()) } else { q.clone() };
        self.chat.push(ChatMsg { role: "user", shown: q, sent, error: false });
        self.question.clear();
        self.pending_actions.clear();
        self.agent_rounds = AGENT_ROUNDS;
        self.undo = Some(Snapshot { cols: self.cols.clone(), envs: self.envs.clone(), ed: self.ed.clone(), cur: self.cur, col_sel: self.col_sel });
        self.ask_agent(ctx);
    }

    /// Siguiente ronda automática: el último mensaje "tool" ya tiene el resultado; se añade el contexto actualizado.
    fn continue_agent(&mut self, ctx: &egui::Context) {
        if self.agent_rounds == 0 {
            self.chat.push(ChatMsg { role: "tool", shown: "Límite de rondas automáticas alcanzado; pregúntale de nuevo para seguir.".into(), sent: String::new(), error: true });
            return;
        }
        self.agent_rounds -= 1;
        let light = self.agent_context_light();
        if let Some(last) = self.chat.last_mut().filter(|m| m.role == "tool") {
            last.sent = format!("{}\n\n---\nContexto actualizado:\n{light}", last.sent);
        }
        self.ask_agent(ctx);
    }

    /// Contexto corto para las rondas automáticas (el completo ya se envió con la pregunta).
    fn agent_context_light(&self) -> String {
        format!("Request abierto: {} {}\nBody:\n{}\nTests:\n{}", self.ed.method, self.ed.url, cut(&self.ed.body, 1500), cut(&self.ed.tests, 1500))
    }

    fn ask_agent(&mut self, ctx: &egui::Context) {
        let Some(agent) = self.agents.get(self.agent_sel).cloned() else { return };
        let messages: Vec<(String, String)> = self
            .chat
            .iter()
            .filter(|m| !m.error && !m.sent.is_empty())
            .map(|m| (if m.role == "assistant" { "assistant" } else { "user" }.to_string(), m.sent.clone()))
            .collect();
        let (tx, rx) = channel();
        let (client, ctx) = (self.client.clone(), ctx.clone());
        let timeout = std::time::Duration::from_secs(self.settings.claude_timeout_secs);
        std::thread::spawn(move || {
            let _ = tx.send(agent::ask(&client, &agent, &messages, timeout));
            ctx.request_repaint();
        });
        self.asking = Some(rx);
    }

    /// Aplica en orden las acciones del agente. Se detiene en la primera que necesite tu aprobación
    /// (borrar, o enviar a un host que no usaste tú); `approved` = ya la diste.
    fn run_actions(&mut self, ctx: &egui::Context, list: Vec<Result<Action, String>>, approved: bool) {
        if list.is_empty() {
            return;
        }
        let (mut log, mut fx, mut failed) = (Vec::new(), Effects::default(), false);
        let mut items = list.into_iter();
        while let Some(item) = items.next() {
            let a = match item {
                Ok(a) => a,
                Err(e) => {
                    log.push(format!("✖ {e}"));
                    failed = true;
                    continue;
                }
            };
            let needs_ok = a.destructive() || (a == Action::Send {} && !self.host_trusted());
            if needs_ok && !approved {
                self.pending_actions = std::iter::once(a).chain(items.by_ref().filter_map(Result::ok)).collect();
                break;
            }
            let mut ws = Ws { cols: &mut self.cols, envs: &mut self.envs, ed: &mut self.ed, cur: &mut self.cur, col_sel: &mut self.col_sel };
            match ws.apply(&a, &mut fx) {
                Ok(msg) => log.push(format!("✔ {msg}")),
                Err(e) => {
                    log.push(format!("✖ {}: {e}", a.describe()));
                    failed = true;
                }
            }
        }
        if fx.cols {
            self.persist(COLLECTIONS);
        }
        if fx.envs {
            self.persist(ENVIRONMENTS);
        }
        self.sync_buffers();
        if !log.is_empty() {
            let text = format!("Acciones aplicadas:\n{}", log.join("\n"));
            self.chat.push(ChatMsg { role: "tool", shown: text.clone(), sent: text, error: false });
        }
        if !self.pending_actions.is_empty() {
            let host = self.current_host().unwrap_or_else(|| "(URL sin resolver)".into());
            let list: Vec<String> = self
                .pending_actions
                .iter()
                .map(|a| if *a == (Action::Send {}) { format!("enviar a {host}") } else { a.describe() })
                .collect();
            self.chat.push(ChatMsg { role: "tool", shown: format!("⚠ El agente necesita tu aprobación para:\n- {}", list.join("\n- ")), sent: String::new(), error: false });
            return;
        }
        if fx.send {
            self.agent_waits_send = true;
            self.send(ctx);
        } else if failed {
            self.continue_agent(ctx); // que el agente vea el error y lo corrija
        }
    }

    fn approve_pending(&mut self, ctx: &egui::Context) {
        let list = std::mem::take(&mut self.pending_actions);
        if list.contains(&Action::Send {}) {
            if let Some(h) = self.current_host() {
                self.trusted_hosts.insert(h);
            }
        }
        self.run_actions(ctx, list.into_iter().map(Ok).collect(), true);
    }

    fn reject_pending(&mut self) {
        let list: Vec<String> = std::mem::take(&mut self.pending_actions).iter().map(Action::describe).collect();
        let text = format!("El usuario rechazó: {}", list.join(", "));
        self.chat.push(ChatMsg { role: "tool", shown: text.clone(), sent: text, error: false });
    }

    fn undo_agent(&mut self) {
        let Some(s) = self.undo.take() else { return };
        (self.cols, self.envs, self.ed, self.cur, self.col_sel) = (s.cols, s.envs, s.ed, s.cur, s.col_sel);
        self.pending_actions.clear();
        self.persist(COLLECTIONS);
        self.persist(ENVIRONMENTS);
        self.sync_buffers();
        let text = "El usuario deshizo todos los cambios del agente de esta pregunta.".to_string();
        self.chat.push(ChatMsg { role: "tool", shown: text.clone(), sent: text, error: false });
    }

    // ---- Repos (Azure DevOps) ----

    fn start_repo(&mut self, ctx: &egui::Context, url: String, branch: String, sync_col: Option<usize>) {
        if self.repo_pending.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(repo::fetch_and_scan(Path::new(REPO_CACHE), &url, &branch));
            ctx.request_repaint();
        });
        self.repo_msg = "Conectando con el repo… (si git pide credenciales, se abre la ventana de Git Credential Manager)".into();
        self.repo_pending = Some((rx, sync_col));
    }

    fn finish_repo(&mut self, res: Result<repo::Scan, String>, sync_col: Option<usize>) {
        let scan = match res {
            Ok(s) => s,
            Err(e) => {
                if sync_col.is_some() {
                    self.error = e.clone();
                }
                self.repo_msg = e;
                return;
            }
        };
        let cfg = RepoCfg { url: scan.url.clone(), branch: scan.branch.clone() };
        self.settings.repos.retain(|r| *r != cfg);
        self.settings.repos.insert(0, cfg);
        self.settings.repos.truncate(10);
        self.persist(SETTINGS);
        self.repo_msg = format!("Commit {} · {} archivo(s) en /{}", scan.commit, scan.files.len(), repo::FOLDER);

        if let Some(ci) = sync_col {
            let file = self.cols.get(ci).and_then(|c| c.source.as_ref()).map(|s| s.file.clone()).unwrap_or_default();
            match scan.files.iter().find(|(p, _)| *p == file) {
                Some((_, Ok(remote))) => self.notice = self.sync_into(ci, &scan, &file, remote.clone()),
                Some((_, Err(e))) => self.error = format!("{file}: {e}"),
                None => self.error = format!("{file} ya no está en /{} del repo", repo::FOLDER),
            }
        }
        self.repo_scan = Some(scan);
    }

    /// Los servidores declarados en un contrato que tú importaste son de confianza para el agente.
    fn trust_collection_hosts(&mut self, ci: usize) {
        let hosts: Vec<String> = self.cols[ci].variables.values().filter_map(|v| actions::host_of(v)).collect();
        self.trusted_hosts.extend(hosts);
    }

    fn sync_into(&mut self, ci: usize, scan: &repo::Scan, file: &str, remote: Collection) -> String {
        // Si el request abierto es de esta colección y no tiene cambios sin guardar, se recarga tras sincronizar.
        let clean = matches!(self.cur, Some((c, i)) if c == ci && self.cols[ci].requests.get(i) == Some(&self.ed));
        let report = repo::merge(&mut self.cols[ci], remote);
        self.cols[ci].source = Some(source_of(scan, file));
        self.trust_collection_hosts(ci);
        if let (true, Some((_, i))) = (clean, self.cur) {
            self.ed = self.cols[ci].requests[i].clone();
        }
        self.persist(COLLECTIONS);
        self.sync_buffers();
        format!("{} sincronizada ({}): {}", self.cols[ci].name, scan.commit, report.summary())
    }

    // ---- Import ----

    fn do_import(&mut self) {
        // Los JSON exportados en Windows a veces traen BOM, que serde_json rechaza.
        let text = self.import_text.trim_start_matches('\u{feff}').trim().to_string();
        let res = if text.starts_with("curl") {
            import::parse_curl(&text).map(|r| {
                self.ed = r;
                self.cur = None;
            })
        } else {
            import::import_postman(&text).map(|mut c| {
                // Los scripts corren con acceso a tus entornos (tokens): solo se mantienen si lo pides.
                if !self.import_keep_scripts {
                    for r in &mut c.requests {
                        r.pre.clear();
                        r.tests.clear();
                    }
                }
                self.cols.push(c);
                self.col_sel = self.cols.len() - 1;
                self.cur = None;
                self.persist(COLLECTIONS);
            })
        };
        match res {
            Ok(()) => {
                self.import_text.clear();
                self.import_err.clear();
                self.show_import = false;
                self.sync_buffers();
            }
            Err(e) => self.import_err = e,
        }
    }

    // ---- UI ----

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("⚡ api-connect").strong().size(17.0).color(theme::accent()));
            ui.label(RichText::new("local · sin cuenta · sin nube").small().color(theme::muted()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let agent = RichText::new("✨ Agente IA").color(if self.show_agent { theme::accent() } else { ui.visuals().text_color() });
                if ui.add(Button::new(agent).selected(self.show_agent)).on_hover_text("Asistente IA que puede editar la plataforma").clicked() {
                    self.show_agent = !self.show_agent;
                }
                let n = cookies::snapshot().len();
                if ui.button(format!("Cookies ({n})")).on_hover_text("Cookies guardadas: se envían solas a su dominio").clicked() {
                    self.show_cookies = !self.show_cookies;
                }
                let label = if self.settings.light_theme { "Tema oscuro" } else { "Tema claro" };
                if ui.button(label).on_hover_text("Cambiar entre One Dark y One Light (se guarda)").clicked() {
                    self.settings.light_theme = !self.settings.light_theme;
                    apply_theme(ui.ctx(), self.settings.light_theme);
                    self.persist(SETTINGS);
                }
                if ui.add_enabled(!self.cols.is_empty(), Button::new("▶ Runner")).on_hover_text("Runner y estrés de la colección actual").clicked() {
                    self.open_runner(self.active_col().unwrap_or(0), false);
                }
                ui.separator();
                let before = self.envs.active.clone();
                let shown = if self.envs.active.is_empty() { "Sin entorno".to_string() } else { self.envs.active.clone() };
                egui::ComboBox::from_id_salt("env").width(170.0).selected_text(RichText::new(shown).strong()).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.envs.active, String::new(), "Sin entorno");
                    for name in self.envs.envs.keys() {
                        ui.selectable_value(&mut self.envs.active, name.clone(), name);
                    }
                });
                ui.label(RichText::new("Entorno").small().color(theme::muted()));
                if self.envs.active != before {
                    self.env_edit = self.envs.active.clone();
                    self.persist(ENVIRONMENTS);
                    self.sync_buffers();
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let n: usize = self.cols.iter().map(|c| c.requests.len()).sum();
            let small = |t: String| RichText::new(t).small().color(theme::muted());
            ui.label(small(format!("{} colecciones · {n} requests", self.cols.len())));
            ui.separator();
            ui.label(small(format!("entorno: {}", if self.envs.active.is_empty() { "ninguno" } else { &self.envs.active })));
            if self.sending.is_some() {
                ui.separator();
                ui.spinner();
                ui.label(small("enviando…".into()));
            }
            if self.asking.is_some() {
                ui.separator();
                ui.spinner();
                ui.label(small("el agente está pensando…".into()));
            }
            if self.run.rx.is_some() {
                ui.separator();
                ui.spinner();
                ui.label(small("runner en curso".into()));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(small("Ctrl+Enter enviar · Ctrl+S guardar · Ctrl+T nueva pestaña · Ctrl+W cerrar".into()));
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for (s, label) in [(Side::Cols, "Colecciones"), (Side::Envs, "Entornos"), (Side::History, "Historial")] {
                let text = RichText::new(label).strong().color(if self.side == s { theme::accent() } else { theme::muted() });
                if ui.selectable_label(self.side == s, text).clicked() {
                    self.side = s;
                }
            }
        });
        ui.add_space(2.0);
        ui.separator();
        match self.side {
            Side::Cols => self.collections_view(ui),
            Side::Envs => self.envs_view(ui),
            Side::History => self.history_view(ui),
        }
    }

    fn collections_view(&mut self, ui: &mut egui::Ui) {
        enum Act {
            Open(usize, usize),
            OpenNew(usize, usize),
            Dup(usize, usize),
            DelReq(usize, usize),
            DelCol(usize),
            Sync(usize),
            Run(usize, bool),
            Vars(usize),
            Auth(usize),
            NewIn(usize),
            NewTab,
            Renamed(usize),
            /// (desde, colección destino, posición destino antes de quitarlo)
            Move((usize, usize), usize, usize),
        }
        let mut act = None;
        // De derecha a izquierda: el buscador toma lo que sobra y no empuja el ancho del panel.
        row_rtl(ui, |ui| {
            ui.menu_button(RichText::new(" + ").strong().color(theme::accent()), |ui| {
                ui.set_min_width(230.0);
                ui.label(RichText::new("Nueva colección").small().color(theme::muted()));
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut self.new_col).hint_text("Nombre").desired_width(150.0));
                    let name = self.new_col.trim().to_string();
                    if ui.add_enabled(!name.is_empty(), Button::new("Crear")).clicked() {
                        self.cols.push(Collection { name, ..Default::default() });
                        self.col_sel = self.cols.len() - 1;
                        self.new_col.clear();
                        self.persist(COLLECTIONS);
                        ui.close_menu();
                    }
                });
                ui.separator();
                if ui.button("Nuevo request   Ctrl+T").clicked() {
                    act = Some(Act::NewTab);
                    ui.close_menu();
                }
                if ui.button("Importar cURL / Postman…").clicked() {
                    self.show_import = true;
                    ui.close_menu();
                }
                if ui.button("Azure DevOps (OpenAPI)…").clicked() {
                    self.show_repos = true;
                    ui.close_menu();
                }
            })
            .response
            .on_hover_text("Nueva colección, request o importar");
            ui.add(TextEdit::singleline(&mut self.filter).hint_text("🔍  Buscar requests").desired_width(ui.available_width()));
        });
        ui.add_space(4.0);

        if self.cols.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Todavía no hay colecciones").strong());
                ui.label(RichText::new("Crea una con +, importa de Postman o cURL, o conecta un repo de Azure DevOps.").small().color(theme::muted()));
                ui.add_space(8.0);
                if ui.button("Importar cURL / Postman").clicked() {
                    self.show_import = true;
                }
                if ui.button("Azure DevOps…").clicked() {
                    self.show_repos = true;
                }
            });
            return;
        }

        let busy = self.repo_pending.is_some();
        let filter = self.filter.trim().to_lowercase();
        let cur = self.cur;
        ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            for (ci, c) in self.cols.iter_mut().enumerate() {
                let matches: Vec<usize> = (0..c.requests.len())
                    .filter(|&ri| {
                        let r = &c.requests[ri];
                        filter.is_empty() || c.name.to_lowercase().contains(&filter) || r.name.to_lowercase().contains(&filter) || r.url.to_lowercase().contains(&filter)
                    })
                    .collect();
                if !filter.is_empty() && matches.is_empty() {
                    continue;
                }
                let mut title = egui::text::LayoutJob::default();
                title.append(&c.name, 0.0, egui::TextFormat::simple(egui::FontId::proportional(14.5), theme::bright()));
                title.append(&format!("  {}", c.requests.len()), 0.0, egui::TextFormat::simple(egui::FontId::proportional(12.0), theme::muted()));
                let col_menu = |ui: &mut egui::Ui, act: &mut Option<Act>, c: &mut Collection| {
                    if ui.button("▶ Ejecutar colección").clicked() {
                        *act = Some(Act::Run(ci, false));
                        ui.close_menu();
                    }
                    if ui.button("⚡ Prueba de estrés").clicked() {
                        *act = Some(Act::Run(ci, true));
                        ui.close_menu();
                    }
                    if ui.button("Autorización…").clicked() {
                        *act = Some(Act::Auth(ci));
                        ui.close_menu();
                    }
                    if ui.button("Variables de colección").clicked() {
                        *act = Some(Act::Vars(ci));
                        ui.close_menu();
                    }
                    if ui.button("Nuevo request aquí").clicked() {
                        *act = Some(Act::NewIn(ci));
                        ui.close_menu();
                    }
                    if c.source.is_some() && ui.add_enabled(!busy, Button::new("Sincronizar con el repo")).clicked() {
                        *act = Some(Act::Sync(ci));
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.label(RichText::new("Renombrar").small().color(theme::muted()));
                    if ui.text_edit_singleline(&mut c.name).changed() {
                        *act = Some(Act::Renamed(ci));
                    }
                    ui.separator();
                    if ui.button(RichText::new("Borrar colección").color(theme::err())).clicked() {
                        *act = Some(Act::DelCol(ci));
                        ui.close_menu();
                    }
                };
                let resp = egui::CollapsingHeader::new(title)
                    .id_salt(("col", ci))
                    .default_open(true)
                    .open((!filter.is_empty()).then_some(true))
                    .show(ui, |ui| {
                        if let Some(s) = &c.source {
                            let short = s.repo.rsplit('/').next().unwrap_or(&s.repo);
                            ui.label(RichText::new(format!("repo {short} · {} @{}", s.file, s.commit)).small().color(theme::muted()))
                                .on_hover_text(format!("{}\nrama: {}", s.repo, if s.branch.is_empty() { "(por defecto)" } else { &s.branch }));
                        }
                        if c.requests.is_empty() {
                            ui.label(RichText::new("Sin requests").small().color(theme::muted()));
                        }
                        for &ri in &matches {
                            let r = &c.requests[ri];
                            // En colecciones de un repo: "+" = agregado localmente, "●" = modificado localmente.
                            let (mark, tip) = match (&c.source, &r.origin) {
                                (Some(_), None) => ("+", "Agregado localmente: no está en el repo"),
                                (Some(_), Some(_)) if r.modified() => ("●", "Modificado localmente: la sincronización no lo pisa"),
                                _ => (" ", ""),
                            };
                            let mut job = egui::text::LayoutJob::default();
                            let mono = |c: Color32| egui::TextFormat::simple(egui::FontId::monospace(11.5), c);
                            job.append(mark, 0.0, mono(theme::warn()));
                            job.append(&format!("{:<6}", short_method(&r.method)), 4.0, mono(method_color(&r.method)));
                            job.append(&r.name, 6.0, egui::TextFormat::simple(egui::FontId::proportional(14.0), theme::text()));
                            let selected = cur == Some((ci, ri));
                            let draw = |ui: &mut egui::Ui| ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| ui.selectable_label(selected, job)).inner;
                            // Con el buscador activo no se arrastra: las posiciones visibles no son las reales.
                            let (row, area) = if filter.is_empty() {
                                let ir = ui.dnd_drag_source(Id::new(("drag_req", ci, ri)), (ci, ri), draw);
                                (ir.inner, ir.response)
                            } else {
                                let r = draw(ui);
                                (r.clone(), r)
                            };
                            let below = ui.ctx().pointer_interact_pos().is_some_and(|p| p.y > area.rect.center().y);
                            if area.dnd_hover_payload::<(usize, usize)>().is_some_and(|p| *p != (ci, ri)) {
                                let y = if below { area.rect.bottom() } else { area.rect.top() };
                                ui.painter().hline(area.rect.x_range(), y, egui::Stroke::new(2.0_f32, theme::accent()));
                            }
                            if let Some(p) = area.dnd_release_payload::<(usize, usize)>() {
                                act = Some(Act::Move(*p, ci, if below { ri + 1 } else { ri }));
                            }
                            let row = if tip.is_empty() { row } else { row.on_hover_text(tip) };
                            if row.clicked() {
                                act = Some(Act::Open(ci, ri));
                            }
                            if row.middle_clicked() {
                                act = Some(Act::OpenNew(ci, ri));
                            }
                            row.context_menu(|ui| {
                                if ui.button("Abrir").clicked() {
                                    act = Some(Act::Open(ci, ri));
                                    ui.close_menu();
                                }
                                if ui.button("Abrir en pestaña nueva").clicked() {
                                    act = Some(Act::OpenNew(ci, ri));
                                    ui.close_menu();
                                }
                                if ui.button("Duplicar").clicked() {
                                    act = Some(Act::Dup(ci, ri));
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui.button(RichText::new("Borrar").color(theme::err())).clicked() {
                                    act = Some(Act::DelReq(ci, ri));
                                    ui.close_menu();
                                }
                            });
                        }
                        ui.horizontal(|ui| {
                            if ui.small_button("▶ Ejecutar").on_hover_text("Runner: todos los requests en orden, con sus tests").clicked() {
                                act = Some(Act::Run(ci, false));
                            }
                            if ui.small_button("⚡ Estrés").on_hover_text("Baterías de carga / estrés con estadísticas por endpoint").clicked() {
                                act = Some(Act::Run(ci, true));
                            }
                            ui.menu_button("···", |ui| col_menu(ui, &mut act, c)).response.on_hover_text("Más acciones");
                        });
                    });
                resp.header_response.context_menu(|ui| col_menu(ui, &mut act, c));
                // Soltar sobre el título de una colección: al final de esa colección.
                if resp.header_response.dnd_hover_payload::<(usize, usize)>().is_some() {
                    ui.painter().rect_stroke(resp.header_response.rect, 4.0, egui::Stroke::new(1.5_f32, theme::accent()), egui::StrokeKind::Inside);
                }
                if let Some(p) = resp.header_response.dnd_release_payload::<(usize, usize)>() {
                    act = Some(Act::Move(*p, ci, usize::MAX));
                }
            }
        });

        match act {
            Some(Act::Open(c, r)) => self.open(c, r),
            Some(Act::OpenNew(c, r)) => {
                if !self.busy() && self.cur != Some((c, r)) {
                    self.new_tab();
                }
                self.open(c, r);
            }
            Some(Act::Dup(c, r)) => {
                let mut copy = self.cols[c].requests[r].clone();
                copy.name = format!("{} (copia)", copy.name);
                copy.origin = None;
                self.cols[c].requests.insert(r + 1, copy);
                self.persist(COLLECTIONS);
                if let Some((cc, i)) = self.cur.filter(|&(cc, i)| cc == c && i > r) {
                    self.cur = Some((cc, i + 1));
                }
                self.open(c, r + 1);
            }
            Some(Act::DelReq(c, r)) => self.confirm = Some(Confirm::Req(c, r)),
            Some(Act::DelCol(c)) => self.confirm = Some(Confirm::Col(c)),
            Some(Act::Run(c, stress)) => self.open_runner(c, stress),
            Some(Act::Auth(c)) => self.auth_win = Some(c),
            Some(Act::Vars(c)) => {
                self.vars_win = Some(c);
                self.sync_buffers();
            }
            Some(Act::NewIn(c)) => {
                self.new_tab();
                self.col_sel = c;
            }
            Some(Act::NewTab) => self.new_tab(),
            Some(Act::Move(from, c, i)) => self.move_request(from, c, i),
            Some(Act::Renamed(ci)) => {
                // Las pestañas inactivas ubican su request por nombre de colección.
                let name = self.cols[ci].name.clone();
                for t in self.tabs.iter_mut().filter(|t| t.cur.is_some_and(|(c, _)| c == ci)) {
                    t.col_name = name.clone();
                }
                self.persist(COLLECTIONS);
            }
            Some(Act::Sync(c)) => {
                if let Some(s) = self.cols[c].source.clone() {
                    self.start_repo(&ui.ctx().clone(), s.repo, s.branch, Some(c));
                }
            }
            None => {}
        }
    }

    fn envs_view(&mut self, ui: &mut egui::Ui) {
        row_rtl(ui, |ui| {
            let name = self.new_env.trim().to_string();
            let create = ui.add_enabled(!name.is_empty() && !self.envs.envs.contains_key(&name), Button::new("Crear"));
            ui.add(TextEdit::singleline(&mut self.new_env).hint_text("Nuevo entorno (dev, qa, prod…)").desired_width(ui.available_width()));
            if create.clicked() {
                self.envs.envs.entry(name.clone()).or_default();
                self.env_edit = name;
                self.new_env.clear();
                self.persist(ENVIRONMENTS);
                self.sync_buffers();
            }
        });
        ui.add_space(4.0);
        if self.envs.envs.is_empty() {
            ui.label(RichText::new("Sin entornos. Crea uno para guardar baseUrl, tokens y demás variables.").small().color(theme::muted()));
            return;
        }
        let (mut edit, mut activate, mut delete) = (None, None, None);
        for name in self.envs.envs.keys() {
            let active = *name == self.envs.active;
            ui.horizontal(|ui| {
                let mark = RichText::new(if active { "●" } else { "○" }).color(if active { theme::ok() } else { theme::muted() });
                if ui.add(egui::Label::new(mark).sense(egui::Sense::click())).on_hover_text("Usar este entorno").clicked() {
                    activate = Some(name.clone());
                }
                let text = if active { RichText::new(name).strong() } else { RichText::new(name) };
                let r = ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| ui.selectable_label(self.env_edit == *name, text)).inner;
                if r.clicked() {
                    edit = Some(name.clone());
                }
                r.context_menu(|ui| {
                    if ui.button("Usar este entorno").clicked() {
                        activate = Some(name.clone());
                        ui.close_menu();
                    }
                    if ui.button(RichText::new("Borrar").color(theme::err())).clicked() {
                        delete = Some(name.clone());
                        ui.close_menu();
                    }
                });
            });
        }
        if let Some(n) = activate {
            self.envs.active = n.clone();
            self.env_edit = n;
            self.persist(ENVIRONMENTS);
            self.sync_buffers();
        }
        if let Some(n) = edit {
            self.env_edit = n;
            self.sync_buffers();
        }
        if let Some(n) = delete {
            self.confirm = Some(Confirm::Env(n));
        }
        if !self.envs.envs.contains_key(&self.env_edit) {
            return;
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.env_edit).strong());
            if self.env_edit == self.envs.active {
                widgets::pill(ui, "activo", theme::ok());
            }
        });
        ui.label(RichText::new("Úsalas como {{nombre}}. Precedencia: script > datos > entorno > colección.").small().color(theme::muted()));
        ScrollArea::vertical().id_salt("env_vars").auto_shrink([false; 2]).show(ui, |ui| {
            if widgets::kv_table(ui, "env_table", &mut self.env_rows, false, ("baseUrl", "https://api.ejemplo.com")) {
                if let Some(v) = self.envs.envs.get_mut(&self.env_edit) {
                    *v = rows_vars(&self.env_rows);
                }
                self.persist(ENVIRONMENTS);
            }
        });
    }

    fn history_view(&mut self, ui: &mut egui::Ui) {
        if self.history.is_empty() {
            ui.label(RichText::new("Aquí aparecen los requests que envíes en esta sesión.").small().color(theme::muted()));
            return;
        }
        let mut open = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} envíos", self.history.len())).small().color(theme::muted()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Limpiar").clicked() {
                    self.history.clear();
                }
            });
        });
        ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            for (i, h) in self.history.iter().enumerate() {
                let r = egui::Frame::new().inner_margin(egui::Margin::symmetric(4, 3)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(short_method(&h.req.method)).monospace().small().strong().color(method_color(&h.req.method)));
                        match h.status {
                            Some(s) => widgets::pill(ui, s.to_string(), status_color(s)),
                            None => widgets::pill(ui, "error", theme::err()),
                        };
                        ui.label(RichText::new(format!("{} ms · {}", h.ms, ago(h.at))).small().color(theme::muted()));
                    });
                    let name = if h.req.name.is_empty() { &h.req.url } else { &h.req.name };
                    ui.add(egui::Label::new(RichText::new(cut_line(name, 60))).truncate());
                    ui.add(egui::Label::new(RichText::new(cut_line(&h.req.url, 80)).small().monospace().color(theme::muted())).truncate());
                });
                let r = ui.interact(r.response.rect, Id::new(("hist", i)), egui::Sense::click()).on_hover_text("Abrir en una pestaña");
                if r.hovered() {
                    ui.painter().rect_stroke(r.rect, 4.0, egui::Stroke::new(1.0_f32, theme::line()), egui::StrokeKind::Inside);
                }
                if r.clicked() {
                    open = Some(i);
                }
            }
        });
        if let Some(i) = open {
            let req = self.history[i].req.clone();
            if is_dirty(&self.cols, self.cur, &self.ed) || self.cur.is_some() {
                self.new_tab();
            }
            if !self.busy() {
                self.cur = None;
                self.ed = req;
                self.ed.origin = None;
            }
        }
    }

    fn tabs_bar(&mut self, ui: &mut egui::Ui) {
        let (mut switch, mut close, mut new) = (None, None, false);
        let busy = self.busy();
        ScrollArea::horizontal().id_salt("req_tabs").auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for i in 0..self.tabs.len() {
                    let active = i == self.active_tab;
                    let (ed, cur) = if active { (&self.ed, self.cur) } else { (&self.tabs[i].ed, self.tabs[i].cur) };
                    let dirty = is_dirty(&self.cols, cur, ed);
                    let name = if !ed.name.trim().is_empty() {
                        cut_line(&ed.name, 26)
                    } else if !ed.url.trim().is_empty() {
                        cut_line(&ed.url, 26)
                    } else {
                        "Nuevo request".into()
                    };
                    let frame = egui::Frame::new()
                        .fill(if active { theme::surface() } else { Color32::TRANSPARENT })
                        .stroke(egui::Stroke::new(1.0_f32, if active { theme::line() } else { Color32::TRANSPARENT }))
                        .corner_radius(egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 })
                        .inner_margin(egui::Margin::symmetric(10, 5));
                    let out = frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(short_method(&ed.method)).monospace().small().strong().color(method_color(&ed.method)));
                            ui.add_space(2.0);
                            let label = RichText::new(name).color(if active { theme::bright() } else { theme::muted() });
                            let r = ui.add(egui::Label::new(label).selectable(false).sense(egui::Sense::click()));
                            if r.clicked() {
                                switch = Some(i);
                            }
                            if r.middle_clicked() {
                                close = Some(i);
                            }
                            if dirty {
                                ui.label(RichText::new("●").small().color(theme::warn())).on_hover_text("Cambios sin guardar");
                            }
                            if ui.add(Button::new(RichText::new("×").color(theme::muted())).frame(false)).on_hover_text("Cerrar (Ctrl+W)").clicked() {
                                close = Some(i);
                            }
                        });
                    });
                    if active {
                        let r = out.response.rect;
                        ui.painter().hline(r.x_range(), r.top() + 0.5, egui::Stroke::new(2.0_f32, theme::accent()));
                    } else if busy {
                        out.response.on_hover_text("Espera a que termine el envío o el agente para cambiar de pestaña");
                    }
                }
                if ui.add(Button::new(RichText::new(" + ").strong().color(theme::muted())).frame(false)).on_hover_text("Nueva pestaña (Ctrl+T)").clicked() {
                    new = true;
                }
            });
        });
        if let Some(i) = switch {
            self.switch_tab(i);
        }
        if let Some(i) = close {
            self.request_close_tab(i);
        }
        if new {
            self.new_tab();
        }
    }

    fn main_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.tabs_bar(ui);
        ui.add_space(6.0);

        // Ruta + nombre + guardar
        let dirty = is_dirty(&self.cols, self.cur, &self.ed);
        ui.horizontal(|ui| {
            match self.cur {
                Some((c, _)) => {
                    ui.label(RichText::new(format!("{}  /", self.cols.get(c).map_or("", |c| c.name.as_str()))).color(theme::muted()));
                }
                None => {
                    ui.label(RichText::new("Guardar en").small().color(theme::muted()));
                    let before = self.col_sel;
                    let shown = self.cols.get(self.col_sel).map_or("(nueva: General)".to_string(), |c| c.name.clone());
                    egui::ComboBox::from_id_salt("col").selected_text(shown).show_ui(ui, |ui| {
                        for (i, c) in self.cols.iter().enumerate() {
                            ui.selectable_value(&mut self.col_sel, i, &c.name);
                        }
                    });
                    if before != self.col_sel {
                        self.sync_buffers();
                    }
                    ui.label(RichText::new("/").color(theme::muted()));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let save = if dirty { Button::new(RichText::new("Guardar").strong().color(theme::on_accent())).fill(theme::accent()) } else { Button::new("Guardar") };
                if ui.add(save).on_hover_text("Ctrl+S").clicked() {
                    self.save_request();
                }
                if self.ed.modified()
                    && ui.button("Descartar cambios locales").on_hover_text("Vuelve a la versión del repo de la última sincronización").clicked()
                {
                    let base = self.ed.origin.as_ref().map(|o| o.base.clone()).unwrap_or_default();
                    self.ed.set_parts(base.clone());
                    if let Some((c, i)) = self.cur {
                        self.cols[c].requests[i].set_parts(base);
                        self.persist(COLLECTIONS);
                    }
                }
                if dirty {
                    ui.label(RichText::new("● sin guardar").small().color(theme::warn()));
                }
                ui.add(
                    TextEdit::singleline(&mut self.ed.name)
                        .hint_text("Request sin nombre")
                        .font(egui::FontId::proportional(16.0))
                        .frame(false)
                        .desired_width(ui.available_width()),
                );
            });
        });
        ui.add_space(4.0);

        // Barra de URL
        egui::Frame::new().fill(theme::deep()).stroke(egui::Stroke::new(1.0_f32, theme::line())).corner_radius(6.0).inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| {
            ui.horizontal(|ui| {
                let m = self.ed.method.clone();
                egui::ComboBox::from_id_salt("method").width(92.0).selected_text(RichText::new(&m).strong().monospace().color(method_color(&m))).show_ui(ui, |ui| {
                    for x in METHODS {
                        ui.selectable_value(&mut self.ed.method, x.to_string(), RichText::new(x).monospace().strong().color(method_color(x)));
                    }
                });
                ui.separator();
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let label = if self.sending.is_some() { "Enviando…" } else { "Enviar" };
                    let send = Button::new(RichText::new(label).strong().size(15.0).color(theme::on_accent())).fill(theme::accent()).min_size(egui::vec2(96.0, 30.0));
                    if ui.add_enabled(self.sending.is_none(), send).on_hover_text("Ctrl+Enter").clicked() {
                        self.send_by_user(&ctx);
                    }
                    let mut lay = widgets::line_layouter(Lang::Plain, 14.0);
                    ui.add(
                        TextEdit::singleline(&mut self.ed.url)
                            .hint_text("{{baseUrl}}/recurso?x=1   —   pega aquí una URL")
                            .font(TextStyle::Monospace)
                            .frame(false)
                            .desired_width(ui.available_width())
                            .layouter(&mut lay),
                    );
                });
            });
        });
        ui.add_space(4.0);

        // Pestañas del request
        self.refresh_contract_view();
        let (n_changes, n_issues) = self.contract_view.as_ref().map_or((0, 0), |v| (v.changes.len(), v.issues.len()));
        let n_params = widgets::url_to_rows(&self.ed.url).1.len();
        let n_headers = widgets::headers_to_rows(&self.ed.headers).iter().filter(|r| r.on).count();
        ui.horizontal(|ui| {
            let count = |n: usize| if n == 0 { String::new() } else { format!(" ({n})") };
            let dot = |s: &str| if s.trim().is_empty() { "" } else { " ●" };
            let tabs = [
                (Tab::Params, format!("Params{}", count(n_params))),
                (Tab::Auth, if matches!(self.ed.auth, model::Auth::Inherit | model::Auth::None) { "Auth".to_string() } else { "Auth ●".to_string() }),
                (Tab::Headers, format!("Headers{}", count(n_headers))),
                (Tab::Body, format!("Body{}", dot(&self.ed.body))),
                (Tab::Pre, format!("Pre-request{}", dot(&self.ed.pre))),
                (Tab::Tests, format!("Tests{}", dot(&self.ed.tests))),
            ];
            for (t, label) in tabs {
                let text = RichText::new(label).color(if self.tab == t { theme::bright() } else { theme::muted() });
                let r = ui.selectable_label(self.tab == t, text);
                if r.clicked() {
                    self.tab = t;
                }
            }
            if self.ed.origin.is_some() {
                let label = match n_changes {
                    0 => "Contrato".to_string(),
                    n => format!("Contrato · {n} cambio{}", if n == 1 { "" } else { "s" }),
                };
                ui.selectable_value(&mut self.tab, Tab::Contract, label).on_hover_text("Tu versión comparada con el contrato del repo (solo lectura)");
                if n_issues > 0 && ui.link(RichText::new(format!("⚠ {n_issues} aviso{}", if n_issues == 1 { "" } else { "s" })).color(theme::warn())).clicked() {
                    self.tab = Tab::Contract;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| match self.tab {
                Tab::Headers => {
                    ui.checkbox(&mut self.bulk, "Editar como texto");
                }
                Tab::Body => {
                    if ui.small_button("{ } Embellecer").on_hover_text("Formatea el JSON").clicked() {
                        match serde_json::from_str::<serde_json::Value>(&self.ed.body).and_then(|v| serde_json::to_string_pretty(&v)) {
                            Ok(p) => self.ed.body = p,
                            Err(e) => self.error = format!("El body no es JSON válido ({e}). Si usas {{{{vars}}}} sin comillas, ponlas entre comillas para formatear."),
                        }
                    }
                }
                _ => {}
            });
        });
        let sep = ui.cursor().top();
        ui.painter().hline(ui.max_rect().x_range(), sep, egui::Stroke::new(1.0_f32, theme::line()));
        ui.add_space(6.0);

        let h = ui.available_height();
        egui::TopBottomPanel::bottom("response")
            .resizable(true)
            .default_height(h * 0.52)
            .min_height(120.0)
            .max_height(h - 100.0)
            .frame(egui::Frame::new().inner_margin(egui::Margin { left: 0, right: 0, top: 8, bottom: 0 }))
            .show_inside(ui, |ui| self.response_panel(ui));
        egui::CentralPanel::default().frame(egui::Frame::new()).show_inside(ui, |ui| {
            if self.tab == Tab::Contract {
                self.contract_tab(ui);
            } else {
                self.request_editor(ui);
            }
        });
    }

    /// Recalcula diferencias y validación solo cuando cambia tu versión o el request abierto.
    fn refresh_contract_view(&mut self) {
        let Some(o) = &self.ed.origin else {
            self.contract_view = None;
            if self.tab == Tab::Contract {
                self.tab = Tab::Params;
            }
            return;
        };
        let parts = self.ed.parts();
        if self.contract_view.as_ref().is_some_and(|v| v.parts == parts && v.key == o.key) {
            return;
        }
        let changes = overlay::diff(&o.base, &parts);
        let issues = overlay::validate(&parts.body, &o.schema);
        self.contract_view = Some(ContractView { parts, key: o.key.clone(), changes, issues });
    }

    fn contract_tab(&mut self, ui: &mut egui::Ui) {
        let (Some(o), Some(view)) = (self.ed.origin.clone(), self.contract_view.as_ref()) else { return };
        let mut restore: Option<String> = None;
        ScrollArea::vertical().id_salt("contract").max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
            ui.label(RichText::new(format!("Contrato: {}  ·  lo que editas es tu versión; el contrato no se modifica", o.key)).small().color(theme::muted()));
            if view.changes.is_empty() {
                ui.label("Tu versión es igual al contrato.");
            } else {
                egui::Grid::new("changes").striped(true).num_columns(4).show(ui, |ui| {
                    ui.strong("Campo");
                    ui.strong("Contrato");
                    ui.strong("Tu versión");
                    ui.label("");
                    ui.end_row();
                    for c in &view.changes {
                        ui.monospace(&c.path);
                        ui.label(RichText::new(cut(c.contract.as_deref().unwrap_or("(no existe)"), 80)).color(theme::muted()));
                        let yours = c.yours.as_deref().unwrap_or("(quitado)");
                        // Campo tuyo que el contrato no tiene: se conserva y se marca.
                        let out = c.contract.is_none() && c.path.starts_with("body.");
                        ui.label(if out { RichText::new(format!("{} ⚠ fuera de contrato", cut(yours, 80))).color(theme::warn()) } else { RichText::new(cut(yours, 80)) });
                        if ui.small_button("Restaurar").on_hover_text("Deja este campo como en el contrato").clicked() {
                            restore = Some(c.path.clone());
                        }
                        ui.end_row();
                    }
                });
            }
            if !view.issues.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new("Validación contra el schema (no bloquea el envío):").strong());
                for i in &view.issues {
                    ui.label(RichText::new(format!("⚠ {i}")).color(theme::warn()).monospace());
                }
            }
            egui::CollapsingHeader::new("Ver contrato (solo lectura)").show(ui, |ui| {
                let b = &o.base;
                let text = format!("{} {}\n\n{}\n\n{}", b.method, b.url, b.headers, b.body);
                ui.add(TextEdit::multiline(&mut text.as_str()).code_editor().desired_width(f32::INFINITY));
            });
        });
        if let Some(path) = restore {
            self.ed.set_parts(overlay::restore(&o.base, &self.ed.parts(), &path));
        }
    }

    fn request_editor(&mut self, ui: &mut egui::Ui) {
        let mut get_token = None;
        ScrollArea::vertical().id_salt(("editor", self.tab as u8)).auto_shrink([false; 2]).show(ui, |ui| match self.tab {
            Tab::Params => {
                let (base, mut rows) = widgets::url_to_rows(&self.ed.url);
                ui.label(RichText::new("Parámetros de query: se sincronizan con la URL. Los {{vars}} se resuelven al enviar.").small().color(theme::muted()));
                if widgets::kv_table(ui, "params", &mut rows, false, ("clave", "valor")) {
                    self.ed.url = widgets::rows_to_url(&base, &rows);
                }
            }
            Tab::Headers if self.bulk => {
                ui.label(RichText::new("Un header por línea: Clave: valor. Empieza con // para desactivarlo.").small().color(theme::muted()));
                widgets::code_editor(ui, &mut self.ed.headers, Lang::Plain, "Content-Type: application/json\nAuthorization: Bearer {{token}}", 10);
            }
            Tab::Headers => {
                let mut rows = widgets::headers_to_rows(&self.ed.headers);
                if widgets::kv_table(ui, "headers", &mut rows, true, ("Content-Type", "application/json")) {
                    self.ed.headers = widgets::rows_to_headers(&rows);
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Rápidos:").small().color(theme::muted()));
                    for (k, v) in [("Content-Type", "application/json"), ("Accept", "application/json"), ("Authorization", "Bearer {{token}}")] {
                        if ui.small_button(format!("{k}: {v}")).clicked() {
                            let mut rows = widgets::headers_to_rows(&self.ed.headers);
                            match rows.iter_mut().find(|r| r.key.eq_ignore_ascii_case(k)) {
                                Some(r) => (r.value, r.on) = (v.into(), true),
                                None => rows.push(Row { on: true, key: k.into(), value: v.into() }),
                            }
                            self.ed.headers = widgets::rows_to_headers(&rows);
                        }
                    }
                });
            }
            Tab::Body => {
                let lang = Lang::guess(&self.ed.body);
                widgets::code_editor(ui, &mut self.ed.body, lang, "{\n  \"clave\": \"{{valor}}\"\n}", 14);
            }
            Tab::Pre | Tab::Tests => {
                let pre = self.tab == Tab::Pre;
                let snippets: &[(&str, &str)] = if pre {
                    &[
                        ("Variable local", "pm.variables.set('ts', Date.now());"),
                        ("Header dinámico", "pm.request.headers.upsert({ key: 'X-Request-Id', value: pm.variables.replaceIn('{{$guid}}') });"),
                        ("Login previo", "const res = await pm.sendRequest({ url: pm.variables.replaceIn('{{baseUrl}}/login'), method: 'POST', header: { 'Content-Type': 'application/json' }, body: { mode: 'raw', raw: JSON.stringify({ user: 'demo' }) } });\npm.environment.set('token', res.json().token);"),
                    ]
                } else {
                    &[
                        ("Status 200", "pm.test('status 200', () => pm.response.to.have.status(200));"),
                        ("Tiempo < 500 ms", "pm.test('responde en menos de 500 ms', () => pm.expect(pm.response.responseTime).to.be.below(500));"),
                        ("Tiene campo", "pm.test('trae id', () => pm.expect(pm.response.json()).to.have.property('id'));"),
                        ("Guardar token", "pm.environment.set('token', pm.response.json().token);"),
                        ("Siguiente request", "pm.setNextRequest('nombre del request'); // null termina la iteración"),
                    ]
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Fragmentos:").small().color(theme::muted()));
                    for (label, code) in snippets {
                        if ui.small_button(*label).on_hover_text(*code).clicked() {
                            let target = if pre { &mut self.ed.pre } else { &mut self.ed.tests };
                            *target = if target.trim().is_empty() { code.to_string() } else { format!("{}\n{code}", target.trim_end()) };
                        }
                    }
                });
                let (text, hint) = if pre {
                    (&mut self.ed.pre, "// Corre antes de enviar. pm.variables (solo este envío), pm.collectionVariables, pm.environment")
                } else {
                    (&mut self.ed.tests, "// Corre con la respuesta. pm.test(...), pm.expect(...), pm.response.json()")
                };
                widgets::code_editor(ui, text, Lang::Js, hint, 14);
            }
            Tab::Auth => {
                let col = self.active_col();
                let inherited = col.map(|i| (self.cols[i].name.as_str(), &self.cols[i].auth));
                let fetching = self.oauth_pending.is_some();
                let (_, act) = auth_editor(ui, &mut self.ed.auth, false, inherited, fetching);
                if act.edit_collection {
                    self.auth_win = col;
                }
                if act.get_token {
                    get_token = Some(AuthTarget::Tab(self.active_tab));
                }
            }
            Tab::Contract => {}
        });
        if let Some(t) = get_token {
            self.fetch_token(ui.ctx(), t);
        }
    }

    fn response_panel(&mut self, ui: &mut egui::Ui) {
        // El panel guarda el alto de su contenido: sin esto, el estado vacío lo encoge para siempre.
        ui.set_min_height(ui.available_height());
        let pass = self.tests.iter().filter(|t| t.pass).count();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Respuesta").strong());
            if self.sending.is_some() {
                ui.spinner();
            }
            match &self.resp {
                Some(Ok(r)) => {
                    widgets::pill(ui, format!("{} {}", r.status, r.status_text), status_color(r.status));
                    widgets::pill(ui, format!("{} ms", r.ms), theme::muted());
                    widgets::pill(ui, human_size(r.size), theme::muted());
                }
                Some(Err(_)) => {
                    widgets::pill(ui, "Error de conexión", theme::err());
                }
                None => {}
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let n_headers = match &self.resp {
                    Some(Ok(r)) => r.headers.len(),
                    _ => 0,
                };
                let n_cookies = match &self.resp {
                    Some(Ok(r)) => r.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie")).count(),
                    _ => 0,
                };
                let tests_label = if self.tests.is_empty() { "Tests".to_string() } else { format!("Tests {pass}/{}", self.tests.len()) };
                let tests_color = if self.tests.is_empty() { theme::muted() } else if pass == self.tests.len() { theme::ok() } else { theme::err() };
                let tabs = [
                    (RespTab::Console, RichText::new(format!("Consola{}", if self.logs.is_empty() { String::new() } else { format!(" ({})", self.logs.len()) }))),
                    (RespTab::Tests, RichText::new(tests_label).color(tests_color)),
                    (RespTab::Cookies, RichText::new(format!("Cookies{}", if n_cookies == 0 { String::new() } else { format!(" ({n_cookies})") }))),
                    (RespTab::Headers, RichText::new(format!("Headers{}", if n_headers == 0 { String::new() } else { format!(" ({n_headers})") }))),
                    (RespTab::Body, RichText::new("Body")),
                ];
                for (t, label) in tabs {
                    if ui.selectable_label(self.resp_tab == t, label).clicked() {
                        self.resp_tab = t;
                    }
                }
            });
        });
        ui.add_space(4.0);

        let empty = |ui: &mut egui::Ui, title: &str, sub: &str| {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(title).size(16.0).color(theme::muted()));
                ui.label(RichText::new(sub).small().color(theme::muted()));
            });
        };
        match self.resp_tab {
            RespTab::Body => match &self.resp {
                None if self.sending.is_some() => empty(ui, "Enviando…", "La respuesta aparecerá aquí"),
                None => empty(ui, "Envía el request para ver la respuesta", "Ctrl+Enter · las respuestas no salen de tu máquina"),
                Some(Err(e)) => {
                    egui::Frame::new().fill(theme::err().gamma_multiply(0.12)).stroke(egui::Stroke::new(1.0_f32, theme::err().gamma_multiply(0.5))).corner_radius(6.0).inner_margin(10.0).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new("No se obtuvo respuesta").strong().color(theme::err()));
                        ui.add(egui::Label::new(RichText::new(e).monospace()).wrap());
                        ui.label(RichText::new("Revisa la URL, que el servidor esté arriba y que las {{variables}} existan en el entorno.").small().color(theme::muted()));
                    });
                }
                Some(Ok(r)) => {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.raw, false, "Pretty");
                        ui.selectable_value(&mut self.raw, true, "Raw");
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("Copiar").clicked() {
                                ui.ctx().copy_text(r.body.clone());
                            }
                            if let Some((_, ct)) = r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                                ui.label(RichText::new(ct).small().color(theme::muted()));
                            }
                        });
                    });
                    let text: &str = if self.raw { &r.body[..floor_boundary(&r.body, VIEW_LIMIT)] } else { &self.resp_view };
                    let lang = if self.raw { Lang::Plain } else { Lang::guess(text) };
                    ScrollArea::both().id_salt("resp_body").auto_shrink([false; 2]).show(ui, |ui| widgets::code_viewer(ui, text, lang));
                }
            },
            RespTab::Headers => match &self.resp {
                Some(Ok(r)) => {
                    ScrollArea::vertical().id_salt("resp_headers").auto_shrink([false; 2]).show(ui, |ui| {
                        egui::Grid::new("resp_h").striped(true).num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                            for (k, v) in &r.headers {
                                ui.label(RichText::new(k).monospace().color(theme::syn_key()));
                                ui.add(egui::Label::new(RichText::new(v).monospace()).wrap());
                                ui.end_row();
                            }
                        });
                    });
                }
                _ => empty(ui, "Sin headers", "Envía el request primero"),
            },
            RespTab::Cookies => {
                let set: Vec<&String> = match &self.resp {
                    Some(Ok(r)) => r.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie")).map(|(_, v)| v).collect(),
                    _ => Vec::new(),
                };
                if set.is_empty() {
                    empty(ui, "Esta respuesta no fijó cookies", "Las que lleguen se guardan solas y se envían a su dominio");
                } else {
                    ScrollArea::vertical().id_salt("resp_cookies").auto_shrink([false; 2]).show(ui, |ui| {
                        for sc in set {
                            let (pair, attrs) = sc.split_once(';').unwrap_or((sc.as_str(), ""));
                            let (n, v) = pair.split_once('=').unwrap_or((pair, ""));
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(n.trim()).monospace().strong().color(theme::syn_key()));
                                ui.label(RichText::new(v.trim()).monospace());
                                ui.label(RichText::new(attrs.trim()).small().color(theme::muted()));
                            });
                        }
                        ui.add_space(6.0);
                        if ui.button("Ver todas las cookies guardadas").clicked() {
                            self.show_cookies = true;
                        }
                    });
                }
            }
            RespTab::Tests if self.tests.is_empty() => empty(ui, "Este request no tiene tests", "Agrégalos en la pestaña Tests: hay fragmentos listos para usar"),
            RespTab::Tests => {
                ScrollArea::vertical().id_salt("resp_tests").auto_shrink([false; 2]).show(ui, |ui| {
                    for t in &self.tests {
                        egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                widgets::pill(ui, if t.pass { "PASA" } else { "FALLA" }, if t.pass { theme::ok() } else { theme::err() });
                                ui.label(&t.name);
                            });
                            if let Some(e) = &t.error {
                                ui.label(RichText::new(e).small().monospace().color(theme::err()));
                            }
                        });
                    }
                });
            }
            RespTab::Console if self.logs.is_empty() => empty(ui, "Consola vacía", "console.log(...) en tus scripts aparece aquí"),
            RespTab::Console => {
                ScrollArea::vertical().id_salt("resp_logs").auto_shrink([false; 2]).show(ui, |ui| {
                    for l in &self.logs {
                        ui.add(egui::Label::new(RichText::new(format!("› {l}")).monospace()).wrap());
                    }
                });
            }
        }
    }

    fn agent_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            let shown = self.agents.get(self.agent_sel).map_or("(sin agentes)".to_string(), |a| format!("{} · {}", a.name, a.model));
            egui::ComboBox::from_id_salt("agent").width(210.0).selected_text(shown).show_ui(ui, |ui| {
                for (i, a) in self.agents.iter().enumerate() {
                    ui.selectable_value(&mut self.agent_sel, i, format!("{} ({} · {})", a.name, a.kind, a.model));
                }
            });
            if ui.button("Configurar").clicked() {
                self.agents_json = serde_json::to_string_pretty(&self.agents).unwrap_or_default();
                self.agents_err.clear();
                self.timeout_edit = self.settings.claude_timeout_secs;
                self.show_agents_cfg = true;
            }
            if ui.button("Limpiar").clicked() {
                self.chat.clear();
                self.pending_actions.clear();
            }
        });
        if self.undo.is_some() && self.chat.iter().any(|m| m.role == "tool") {
            ui.horizontal(|ui| {
                if ui.button("↶ Deshacer cambios del agente").on_hover_text("Vuelve colecciones, entornos y editor a como estaban antes de tu última pregunta").clicked() {
                    self.undo_agent();
                }
            });
        }
        ui.separator();

        let mut insert: Option<(Tab, String)> = None;
        let (mut approve, mut reject) = (false, false);
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            let busy = self.asking.is_some() || self.agent_waits_send;
            let label = if self.agent_waits_send { "Esperando la respuesta HTTP…" } else if busy { "Pensando…" } else { "Preguntar  (Ctrl+Enter)" };
            if ui.add_enabled(!busy, Button::new(RichText::new(label).strong().color(theme::on_accent())).fill(theme::accent())).clicked() {
                self.ask(&ctx);
            }
            ui.add(
                TextEdit::multiline(&mut self.question)
                    .id(Id::new("question"))
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .hint_text("Ej: escribe tests que validen que la respuesta trae un id numérico y guarda el token en el entorno"),
            );
            ui.checkbox(&mut self.with_context, "Incluir contexto: request, contrato, colecciones y última respuesta (sin valores de variables)");
            if !self.pending_actions.is_empty() {
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Aprobar").color(theme::ok())).clicked() {
                        approve = true;
                    }
                    if ui.button(RichText::new("Rechazar").color(theme::err())).clicked() {
                        reject = true;
                    }
                    ui.label(RichText::new("El agente espera tu aprobación").color(theme::warn()));
                });
            }
            ui.separator();
            ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false; 2]).show(ui, |ui| {
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    if self.chat.is_empty() {
                        ui.add_space(24.0);
                        ui.label(RichText::new("Asistente IA").size(18.0).strong());
                        ui.label(
                            RichText::new("Puede escribir scripts, crear y editar requests, colecciones y variables, y enviar requests para probar. Pide aprobación antes de borrar o de enviar a hosts nuevos.")
                                .color(theme::muted()),
                        );
                        ui.add_space(10.0);
                        ui.label(RichText::new("PRUEBA CON").small().color(theme::muted()));
                        for s in [
                            "Escribe tests para este request: status 200 y que la respuesta tenga los campos esperados",
                            "Crea un request de login que guarde el token en el entorno",
                            "Revisa la última respuesta y explícame si algo se ve mal",
                            "Crea una colección CRUD para /productos con sus tests",
                        ] {
                            if ui.add(Button::new(RichText::new(s).color(theme::accent())).wrap()).clicked() {
                                self.question = s.into();
                            }
                        }
                    }
                    for m in &self.chat {
                        let fill = match m.role {
                            "user" => ui.visuals().faint_bg_color,
                            "tool" => ui.visuals().widgets.noninteractive.bg_fill,
                            _ => ui.visuals().extreme_bg_color,
                        };
                        egui::Frame::new().fill(fill).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if m.role == "tool" {
                                ui.label(RichText::new(&m.shown).small().monospace().color(if m.error { theme::err() } else { theme::muted() }));
                                return;
                            }
                            for (lang, part) in agent::split_code(&m.shown) {
                                let Some(lang) = lang else {
                                    ui.label(if m.error { RichText::new(part).color(theme::err()) } else { RichText::new(part) });
                                    continue;
                                };
                                if lang == "actions" {
                                    egui::CollapsingHeader::new(RichText::new("⚙ acciones para la plataforma").color(theme::accent())).id_salt(&part).show(ui, |ui| {
                                        ui.add(egui::Label::new(RichText::new(&part).monospace().small()).wrap());
                                    });
                                    continue;
                                }
                                egui::Frame::new().fill(ui.visuals().code_bg_color).inner_margin(6.0).show(ui, |ui| {
                                    ui.add(egui::Label::new(RichText::new(&part).monospace()).wrap());
                                });
                                ui.horizontal(|ui| {
                                    for (tab, label) in [(Tab::Pre, "→ Pre-request"), (Tab::Tests, "→ Tests"), (Tab::Body, "→ Body")] {
                                        if ui.small_button(label).clicked() {
                                            insert = Some((tab, part.clone()));
                                        }
                                    }
                                    if ui.small_button("Copiar").clicked() {
                                        ui.ctx().copy_text(part.clone());
                                    }
                                });
                            }
                        });
                        ui.add_space(6.0);
                    }
                });
            });
        });

        if let Some((tab, code)) = insert {
            let target = match tab {
                Tab::Pre => &mut self.ed.pre,
                Tab::Tests => &mut self.ed.tests,
                _ => &mut self.ed.body,
            };
            *target = if target.trim().is_empty() { code } else { format!("{}\n{code}", target.trim_end()) };
            self.tab = tab;
        }
        if approve {
            self.approve_pending(&ctx);
        }
        if reject {
            self.reject_pending();
        }
    }

    fn runner_window(&mut self, ctx: &egui::Context) {
        let mut open = self.run.open;
        let title = format!("Runner · {}", self.cols.get(self.run.col).map_or("", |c| c.name.as_str()));
        let mut start = None;
        egui::Window::new(title).id(Id::new("runner")).open(&mut open).default_width(860.0).default_height(560.0).show(ctx, |ui| {
            let running = self.run.rx.is_some();
            let Some(col) = self.cols.get(self.run.col) else {
                ui.label("La colección ya no existe.");
                return;
            };
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!running, |ui| {
                    ui.selectable_value(&mut self.run.stress, false, "▶ Runner");
                    ui.selectable_value(&mut self.run.stress, true, "⚡ Estrés");
                });
                let env = if self.envs.active.is_empty() { "(ninguno)" } else { self.envs.active.as_str() };
                ui.label(RichText::new(format!("entorno: {env}")).color(theme::muted()));
            });
            ui.separator();
            if !self.run.stress {
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!running, |ui| {
                        ui.label("Iteraciones");
                        ui.add(egui::DragValue::new(&mut self.run.iterations).range(1..=100_000));
                        ui.checkbox(&mut self.run.bail, "Detener al primer fallo");
                        if ui.button("Datos CSV/JSON…").on_hover_text("Una fila por iteración: {{columna}} y pm.iterationData.get('columna')").clicked() {
                            let picked = rfd::FileDialog::new()
                                .set_title("Archivo de datos para el runner")
                                .add_filter("CSV / JSON", &["csv", "json", "txt"])
                                .pick_file();
                            if let Some(path) = picked {
                                match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| runner::parse_data(&t)) {
                                    Ok(rows) => {
                                        self.run.iterations = rows.len().max(1) as u32;
                                        self.run.data_name = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
                                        self.run.data = rows;
                                        self.run.err.clear();
                                    }
                                    Err(e) => self.run.err = format!("{}: {e}", path.display()),
                                }
                            }
                        }
                        if !self.run.data.is_empty() {
                            ui.label(RichText::new(format!("{} · {} filas", self.run.data_name, self.run.data.len())).color(theme::muted()));
                            if ui.small_button("×").on_hover_text("Quitar datos").clicked() {
                                (self.run.data, self.run.data_name) = (Vec::new(), String::new());
                            }
                        }
                    });
                    run_buttons(ui, running, &self.run.stop, &mut start, false);
                });
                if !self.run.err.is_empty() {
                    ui.colored_label(theme::err(), &self.run.err);
                }
                let total = self.run.steps.len();
                if total == 0 && !running {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(format!("Ejecuta los {} requests de \"{}\" en orden, con sus tests", col.requests.len(), col.name)).size(16.0).color(theme::muted()));
                        ui.label(
                            RichText::new("Las variables que guarden los scripts pasan al siguiente request. Con un archivo de datos, cada fila es una iteración.")
                                .small()
                                .color(theme::muted()),
                        );
                    });
                }
                if total > 0 {
                    let failed = self.run.steps.iter().filter(|s| !s.passed()).count();
                    let tests: usize = self.run.steps.iter().map(|s| s.tests.len()).sum();
                    let tf = self.run.steps.iter().flat_map(|s| &s.tests).filter(|t| !t.pass).count();
                    let done = self.run.steps.last().map_or(0, |s| s.iteration);
                    ui.colored_label(
                        if failed > 0 { theme::err() } else { theme::ok() },
                        format!("Iteración {done}/{} · {total} requests · {failed} fallidos · {tests} tests · {tf} tests fallidos", self.run.iterations),
                    );
                }
                ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                    for s in &self.run.steps {
                        ui.horizontal(|ui| {
                            ui.colored_label(if s.passed() { theme::ok() } else { theme::err() }, if s.passed() { "✔" } else { "✖" });
                            let status = if s.status == 0 { "---".to_string() } else { s.status.to_string() };
                            ui.monospace(format!("#{} {:<7} {status} {:>6} ms  {}", s.iteration, s.method, s.ms, s.name));
                        });
                        if let Some(e) = &s.error {
                            ui.colored_label(theme::err(), format!("      {e}"));
                        }
                        for t in &s.tests {
                            let text = format!("      {} {}{}", if t.pass { "✔" } else { "✖" }, t.name, t.error.as_ref().map(|e| format!(" — {e}")).unwrap_or_default());
                            ui.colored_label(if t.pass { theme::muted() } else { theme::err() }, text);
                        }
                    }
                });
                return;
            }

            let nb = self.settings.batteries.len();
            ui.add_enabled_ui(!running, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Endpoint");
                    let shown = match self.run.target.and_then(|i| col.requests.get(i)) {
                        Some(r) => format!("{} {}", r.method, r.name),
                        None => format!("Toda la colección ({})", col.requests.len()),
                    };
                    egui::ComboBox::from_id_salt("stress_target").width(300.0).selected_text(shown).show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.run.target, None, format!("Toda la colección ({})", col.requests.len()));
                        for (i, r) in col.requests.iter().enumerate() {
                            ui.selectable_value(&mut self.run.target, Some(i), format!("{} {}", r.method, r.name));
                        }
                    });
                    ui.label("Batería");
                    let shown = self.settings.batteries.get(self.run.battery).map_or("Personalizada".to_string(), |b| b.name.clone());
                    egui::ComboBox::from_id_salt("battery").selected_text(shown).show_ui(ui, |ui| {
                        for (i, b) in self.settings.batteries.iter().enumerate() {
                            ui.selectable_value(&mut self.run.battery, i, &b.name).on_hover_text(&b.description);
                        }
                        ui.selectable_value(&mut self.run.battery, nb, "Personalizada");
                    });
                });
                ui.horizontal(|ui| {
                    ui.label("Preparación");
                    let shown = self.run.setup.and_then(|i| col.requests.get(i)).map_or("(ninguna)".to_string(), |r| format!("{} {}", r.method, r.name));
                    egui::ComboBox::from_id_salt("stress_setup").width(300.0).selected_text(shown).show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.run.setup, None, "(ninguna)");
                        for (i, r) in col.requests.iter().enumerate() {
                            ui.selectable_value(&mut self.run.setup, Some(i), format!("{} {}", r.method, r.name));
                        }
                    });
                    ui.label(RichText::new("se envía una vez antes de medir (p. ej. el login que guarda el token)").small().color(theme::muted()));
                });
                match self.settings.batteries.get(self.run.battery) {
                    Some(b) => {
                        let plan: Vec<String> = b.stages.iter().map(|s| s.label()).collect();
                        let total: u64 = b.stages.iter().map(|s| s.secs).sum();
                        ui.label(RichText::new(format!("{} — {} (≈{total} s)", b.description, plan.join(" → "))).small().color(theme::muted()));
                    }
                    None => {
                        ui.horizontal(|ui| {
                            ui.label("Usuarios virtuales");
                            ui.add(egui::DragValue::new(&mut self.run.vus).range(1..=runner::MAX_VUS));
                            ui.label("Duración");
                            ui.add(egui::DragValue::new(&mut self.run.secs).range(1..=3600).suffix(" s"));
                            ui.label("Rampa previa");
                            ui.add(egui::DragValue::new(&mut self.run.ramp_secs).range(0..=600).suffix(" s"))
                                .on_hover_text("Sube de 1 a los usuarios virtuales en este tiempo antes de sostener la carga (0 = de golpe)");
                        });
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("Úsalo solo contra servicios propios o con autorización. Las baterías se editan en settings.json.").small().color(theme::warn()));
                run_buttons(ui, running, &self.run.stop, &mut start, true);
            });
            if running && !self.run.progress.is_empty() {
                ui.label(&self.run.progress);
            }
            if !self.run.err.is_empty() {
                ui.colored_label(theme::err(), &self.run.err);
            }
            if !self.run.reports.is_empty() {
                ui.horizontal(|ui| {
                    if ui.small_button("Copiar JSON").clicked() {
                        ui.ctx().copy_text(serde_json::to_string_pretty(&self.run.reports).unwrap_or_default());
                    }
                    ui.label(RichText::new("Latencias en ms · éxito = 2xx/3xx · en estrés no se corren los tests").small().color(theme::muted()));
                });
            }
            ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                timeline_charts(ui, &self.run.reports);
                for (i, r) in self.run.reports.iter().enumerate() {
                    ui.add_space(6.0);
                    let ramp = if r.ramp { " (rampa)" } else { "" };
                    ui.label(RichText::new(format!("Etapa {} · {} VUs{ramp} · {} s", i + 1, r.vus, r.secs)).strong());
                    egui::Grid::new(("stage", i)).striped(true).num_columns(11).show(ui, |ui| {
                        for h in ["Endpoint", "n", "err %", "req/s", "p50", "p90", "p95", "p99", "max", "media", "códigos"] {
                            ui.label(RichText::new(h).small().color(theme::muted()));
                        }
                        ui.end_row();
                        for s in r.endpoints.iter().chain([&r.total]) {
                            let name = RichText::new(&s.name);
                            ui.label(if s.name == "Total" { name.strong() } else { name });
                            ui.monospace(s.count.to_string());
                            ui.colored_label(if s.error_pct > 0.0 { theme::err() } else { theme::ok() }, format!("{:.2}", s.error_pct));
                            ui.monospace(format!("{:.1}", s.rps));
                            for v in [s.p50, s.p90, s.p95, s.p99, s.max, s.mean] {
                                ui.monospace(v.to_string());
                            }
                            let codes: Vec<String> = s.statuses.iter().map(|(k, v)| format!("{k}×{v}")).collect();
                            let tip = if s.errors.is_empty() { "sin errores de red".to_string() } else { s.errors.join("\n") };
                            ui.label(codes.join(" ")).on_hover_text(tip);
                            ui.end_row();
                        }
                    });
                }
            });
        });
        self.run.open = open;
        if let Some(stress) = start {
            self.start_run(ctx, stress);
        }
    }

    fn windows(&mut self, ctx: &egui::Context) {
        if self.run.open {
            self.runner_window(ctx);
        }
        if self.show_repos {
            enum RepoAct {
                Connect,
                Fill(RepoCfg),
                Import(usize),
                Sync(usize, usize),
            }
            let mut act = None;
            let mut open = true;
            egui::Window::new("Azure DevOps · /collections").open(&mut open).collapsible(false).default_width(680.0).show(ctx, |ui| {
                ui.label(
                    RichText::new("Usa tus credenciales de git (Git Credential Manager); la app no guarda tokens. Solo se descarga la carpeta /collections del repo.")
                        .small()
                        .color(theme::muted()),
                );
                ui.horizontal(|ui| {
                    ui.label("Repo");
                    ui.add(TextEdit::singleline(&mut self.repo_url).hint_text("https://dev.azure.com/org/proyecto/_git/repo").desired_width(ui.available_width()));
                });
                ui.horizontal(|ui| {
                    ui.label("Rama");
                    ui.add(TextEdit::singleline(&mut self.repo_branch).hint_text("(por defecto)").desired_width(180.0));
                    let busy = self.repo_pending.is_some();
                    let label = if busy { "Conectando…" } else { "Conectar / actualizar" };
                    if ui.add_enabled(!busy && !self.repo_url.trim().is_empty(), Button::new(label)).clicked() {
                        act = Some(RepoAct::Connect);
                    }
                });
                if !self.settings.repos.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Recientes:").small());
                        for r in &self.settings.repos {
                            let short = r.url.trim_end_matches('/').rsplit('/').next().unwrap_or(&r.url);
                            let label = if r.branch.is_empty() { short.to_string() } else { format!("{short} ({})", r.branch) };
                            if ui.small_button(label).on_hover_text(&r.url).clicked() {
                                act = Some(RepoAct::Fill(r.clone()));
                            }
                        }
                    });
                }
                if !self.repo_msg.is_empty() {
                    ui.label(&self.repo_msg);
                }
                if let Some(scan) = &self.repo_scan {
                    ui.separator();
                    ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for (fi, (path, res)) in scan.files.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.monospace(path);
                                match res {
                                    Ok(c) => {
                                        ui.label(RichText::new(format!("{} · {} endpoints", c.name, c.requests.len())).color(theme::muted()));
                                        let existing = self
                                            .cols
                                            .iter()
                                            .position(|l| l.source.as_ref().is_some_and(|s| s.repo == scan.url && s.file == *path));
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| match existing {
                                            Some(ci) => {
                                                if ui.button("Sincronizar").on_hover_text("Trae cambios del repo sin pisar tus cambios locales").clicked() {
                                                    act = Some(RepoAct::Sync(ci, fi));
                                                }
                                            }
                                            None => {
                                                if ui.button("Importar").clicked() {
                                                    act = Some(RepoAct::Import(fi));
                                                }
                                            }
                                        });
                                    }
                                    Err(e) => {
                                        ui.colored_label(theme::err(), e);
                                    }
                                }
                            });
                        }
                    });
                }
            });
            self.show_repos &= open;

            match act {
                Some(RepoAct::Connect) => self.start_repo(ctx, self.repo_url.clone(), self.repo_branch.clone(), None),
                Some(RepoAct::Fill(r)) => (self.repo_url, self.repo_branch) = (r.url, r.branch),
                Some(RepoAct::Import(fi)) | Some(RepoAct::Sync(_, fi)) if self.repo_scan.is_some() => {
                    let scan = self.repo_scan.take().expect("scan");
                    if let (path, Ok(remote)) = &scan.files[fi] {
                        match act {
                            Some(RepoAct::Sync(ci, _)) => self.repo_msg = self.sync_into(ci, &scan, path, remote.clone()),
                            _ => {
                                let mut c = remote.clone();
                                c.source = Some(source_of(&scan, path));
                                self.cols.push(c);
                                self.col_sel = self.cols.len() - 1;
                                self.trust_collection_hosts(self.col_sel);
                                self.persist(COLLECTIONS);
                                self.sync_buffers();
                                self.repo_msg = format!("{path} importado como colección \"{}\"", remote.name);
                            }
                        }
                    }
                    self.repo_scan = Some(scan);
                }
                _ => {}
            }
        }

        if self.show_import {
            let mut open = true;
            egui::Window::new("Importar cURL / Postman").open(&mut open).collapsible(false).default_width(700.0).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Pega un comando cURL o el JSON de una colección de Postman v2.x, o");
                    if ui.button("Abrir archivo…").clicked() {
                        // Diálogo nativo del sistema; bloquea la ventana mientras está abierto, como cualquier diálogo modal.
                        let picked = rfd::FileDialog::new()
                            .set_title("Importar colección de Postman o cURL")
                            .add_filter("Colección Postman / cURL", &["json", "txt", "sh", "curl"])
                            .add_filter("Todos los archivos", &["*"])
                            .pick_file();
                        if let Some(path) = picked {
                            match std::fs::read_to_string(&path) {
                                Ok(t) => {
                                    self.import_text = t;
                                    self.import_err.clear();
                                }
                                Err(e) => self.import_err = format!("{}: {e}", path.display()),
                            }
                        }
                    }
                });
                ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    ui.add(TextEdit::multiline(&mut self.import_text).code_editor().desired_rows(16).desired_width(f32::INFINITY));
                });
                ui.checkbox(&mut self.import_keep_scripts, "Mantener los scripts de la colección (se ejecutan al enviar y pueden leer tus variables)");
                if !self.import_err.is_empty() {
                    ui.colored_label(theme::err(), &self.import_err);
                }
                if ui.button("Importar").clicked() {
                    self.do_import();
                }
            });
            self.show_import &= open;
        }

        if self.show_agents_cfg {
            let mut open = true;
            egui::Window::new("Agentes").open(&mut open).collapsible(false).default_width(560.0).show(ctx, |ui| {
                ui.label(RichText::new("claude: ejecuta `claude -p` sin herramientas ni MCP. Campos: model, effort (low…max).").small());
                ui.label(RichText::new("ollama: llama a {url}/api/chat. Campos: model, url (por defecto http://127.0.0.1:11434).").small());
                ui.add(TextEdit::multiline(&mut self.agents_json).code_editor().desired_rows(14).desired_width(f32::INFINITY));
                ui.horizontal(|ui| {
                    ui.label("Timeout de Claude:");
                    ui.add(egui::DragValue::new(&mut self.timeout_edit).range(10..=3600).suffix(" s"));
                    ui.label(RichText::new("se guarda en settings.json").small().color(theme::muted()));
                });
                if !self.agents_err.is_empty() {
                    ui.colored_label(theme::err(), &self.agents_err);
                }
                if ui.button("Guardar").clicked() {
                    match serde_json::from_str::<Vec<AgentCfg>>(&self.agents_json) {
                        Ok(list) if list.iter().all(|a| !a.name.is_empty() && !a.model.is_empty() && ["claude", "ollama"].contains(&a.kind.as_str())) => {
                            self.agents = list;
                            self.agent_sel = self.agent_sel.min(self.agents.len().saturating_sub(1));
                            self.persist(AGENTS);
                            if self.settings.claude_timeout_secs != self.timeout_edit {
                                self.settings.claude_timeout_secs = self.timeout_edit;
                                self.persist(SETTINGS);
                            }
                            self.show_agents_cfg = false;
                        }
                        Ok(_) => self.agents_err = "Cada agente necesita name, kind (claude | ollama) y model".into(),
                        Err(e) => self.agents_err = format!("JSON inválido: {e}"),
                    }
                }
            });
            self.show_agents_cfg &= open;
        }

        if self.show_cookies {
            let mut open = true;
            egui::Window::new("Cookies").id(Id::new("cookies")).open(&mut open).default_width(760.0).default_height(460.0).show(ctx, |ui| {
                let mut jar = cookies::snapshot();
                let mut changed = false;
                ui.label(
                    RichText::new("Se guardan de las respuestas (Set-Cookie) y se envían solas a su dominio y path. Un header Cookie escrito a mano tiene prioridad.")
                        .small()
                        .color(theme::muted()),
                );
                ui.horizontal(|ui| {
                    let (d, n, v) = &mut self.new_cookie;
                    ui.add(TextEdit::singleline(d).hint_text("dominio (api.ejemplo.com)").desired_width(200.0));
                    ui.add(TextEdit::singleline(n).hint_text("nombre").desired_width(140.0));
                    ui.add(TextEdit::singleline(v).hint_text("valor").desired_width(200.0));
                    let ok = !d.trim().is_empty() && !n.trim().is_empty();
                    if ui.add_enabled(ok, Button::new("Agregar")).clicked() {
                        let c = cookies::Cookie {
                            domain: d.trim().trim_start_matches('.').to_lowercase(),
                            path: "/".into(),
                            name: n.trim().into(),
                            value: v.trim().into(),
                            ..Default::default()
                        };
                        jar.retain(|x| !(x.domain == c.domain && x.path == c.path && x.name == c.name));
                        jar.push(c);
                        self.new_cookie = Default::default();
                        changed = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !jar.is_empty() && ui.button(RichText::new("Borrar todas").color(theme::err())).clicked() {
                            jar.clear();
                            changed = true;
                        }
                    });
                });
                ui.separator();
                if jar.is_empty() {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new("No hay cookies guardadas").color(theme::muted())));
                }
                let mut domains: Vec<String> = jar.iter().map(|c| c.domain.clone()).collect();
                domains.sort();
                domains.dedup();
                let mut remove: Option<usize> = None;
                let mut remove_domain: Option<String> = None;
                ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                    for d in &domains {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(d).strong());
                            ui.label(RichText::new(format!("{}", jar.iter().filter(|c| c.domain == *d).count())).small().color(theme::muted()));
                            if ui.small_button("Borrar dominio").clicked() {
                                remove_domain = Some(d.clone());
                            }
                        });
                        for (i, c) in jar.iter_mut().enumerate().filter(|(_, c)| c.domain == *d) {
                            ui.horizontal(|ui| {
                                ui.add_space(14.0);
                                ui.allocate_ui_with_layout(egui::vec2(150.0, 22.0), Layout::left_to_right(Align::Center), |ui| {
                                    ui.set_min_width(150.0);
                                    ui.add(egui::Label::new(RichText::new(&c.name).monospace().color(theme::syn_key())).truncate());
                                });
                                changed |= ui.add(TextEdit::singleline(&mut c.value).font(TextStyle::Monospace).desired_width(260.0)).changed();
                                ui.label(RichText::new(&c.path).small().monospace().color(theme::muted()));
                                let exp = c.expires.map_or("sesión".to_string(), |e| model::iso8601(e, 0)[..16].replace('T', " "));
                                ui.label(RichText::new(exp).small().color(theme::muted())).on_hover_text("Vencimiento (UTC)");
                                if c.secure {
                                    widgets::pill(ui, "Secure", theme::ok());
                                }
                                if c.http_only {
                                    widgets::pill(ui, "HttpOnly", theme::cyan());
                                }
                                if ui.small_button("×").on_hover_text("Borrar").clicked() {
                                    remove = Some(i);
                                }
                            });
                        }
                        ui.add_space(4.0);
                    }
                });
                if let Some(i) = remove {
                    jar.remove(i);
                    changed = true;
                }
                if let Some(d) = remove_domain {
                    jar.retain(|c| c.domain != d);
                    changed = true;
                }
                if changed {
                    cookies::replace(jar);
                }
            });
            self.show_cookies &= open;
        }

        if let Some(ci) = self.auth_win {
            let mut open = ci < self.cols.len();
            let title = format!("Autorización · {}", self.cols.get(ci).map_or("", |c| c.name.as_str()));
            let fetching = self.oauth_pending.is_some();
            let mut get = false;
            egui::Window::new(title).id(Id::new("col_auth")).open(&mut open).collapsible(false).default_width(640.0).show(ctx, |ui| {
                ui.label(RichText::new("La heredan los requests de esta colección cuyo tipo sea \"Heredar de la colección\".").small().color(theme::muted()));
                ui.add_space(6.0);
                if let Some(c) = self.cols.get_mut(ci) {
                    let (changed, act) = auth_editor(ui, &mut c.auth, true, None, fetching);
                    if changed {
                        self.persist(COLLECTIONS);
                    }
                    get = act.get_token;
                }
            });
            if get {
                self.fetch_token(ctx, AuthTarget::Collection(ci));
            }
            if !open {
                self.auth_win = None;
            }
        }

        if let Some(ci) = self.vars_win {
            let mut open = ci < self.cols.len();
            let title = format!("Variables · {}", self.cols.get(ci).map_or("", |c| c.name.as_str()));
            egui::Window::new(title).id(Id::new("col_vars")).open(&mut open).collapsible(false).default_width(560.0).show(ctx, |ui| {
                ui.label(RichText::new("Disponibles como {{nombre}} en los requests de esta colección. El entorno activo las pisa si repite el nombre.").small().color(theme::muted()));
                ui.add_space(4.0);
                ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    if widgets::kv_table(ui, "col_vars_table", &mut self.col_rows, false, ("baseUrl", "https://api.ejemplo.com")) {
                        if let Some(c) = self.cols.get_mut(ci) {
                            c.variables = rows_vars(&self.col_rows);
                        }
                        self.persist(COLLECTIONS);
                    }
                });
            });
            if !open {
                self.vars_win = None;
            }
        }

        if let Some(c) = self.confirm.clone() {
            let (title, msg, ok) = match &c {
                Confirm::Env(n) => ("Borrar entorno", format!("¿Borrar el entorno \"{n}\" y sus variables?"), "Borrar"),
                Confirm::Col(i) => ("Borrar colección", format!("¿Borrar la colección \"{}\" y todos sus requests?", self.cols.get(*i).map_or("", |c| c.name.as_str())), "Borrar"),
                Confirm::Req(c, r) => (
                    "Borrar request",
                    format!("¿Borrar \"{}\"?", self.cols.get(*c).and_then(|c| c.requests.get(*r)).map_or("", |r| r.name.as_str())),
                    "Borrar",
                ),
                Confirm::CloseTab(_) => ("Cambios sin guardar", "Esta pestaña tiene cambios sin guardar. ¿Cerrarla igual?".to_string(), "Cerrar sin guardar"),
            };
            egui::Window::new(title).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.label(msg);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(Button::new(RichText::new(ok).strong().color(theme::on_accent())).fill(theme::err())).clicked() {
                        self.confirm = None;
                        match c {
                            Confirm::Env(n) => {
                                self.envs.envs.remove(&n);
                                if self.envs.active == n {
                                    self.envs.active.clear();
                                }
                                self.persist(ENVIRONMENTS);
                                self.sync_buffers();
                            }
                            Confirm::Col(i) if i < self.cols.len() => {
                                self.cols.remove(i);
                                self.vars_win = None;
                                self.after_delete(i, None);
                            }
                            Confirm::Req(c, r) if self.cols.get(c).is_some_and(|x| r < x.requests.len()) => {
                                self.cols[c].requests.remove(r);
                                self.after_delete(c, Some(r));
                            }
                            Confirm::CloseTab(i) => self.close_tab(i),
                            _ => {}
                        }
                    }
                    if ui.button("Cancelar").clicked() {
                        self.confirm = None;
                    }
                });
            });
        }
    }
}

type Series = (&'static str, Color32, fn(&runner::Bucket) -> f64, bool);

/// Latencia y throughput segundo a segundo de todas las etapas, con líneas verticales entre etapas.
fn timeline_charts(ui: &mut egui::Ui, reports: &[runner::StageReport]) {
    let pts: Vec<&runner::Bucket> = reports.iter().flat_map(|r| &r.timeline).collect();
    if pts.len() < 2 {
        return;
    }
    let bounds: Vec<usize> = reports.iter().scan(0, |acc, r| Some(std::mem::replace(acc, *acc + r.timeline.len()))).skip(1).collect();
    chart(ui, "Latencia (ms)", &pts, &bounds, &[("p95", theme::accent(), |b| b.p95 as f64, false), ("p50", theme::muted(), |b| b.p50 as f64, false)]);
    chart(
        ui,
        "Throughput",
        &pts,
        &bounds,
        &[("req/s", theme::ok(), |b| b.count as f64, false), ("errores/s", theme::err(), |b| b.errors as f64, false), ("VUs (escala propia)", theme::warn(), |b| b.vus as f64, true)],
    );
}

/// Gráfico de líneas mínimo con el painter de egui; `own` = la serie usa su propia escala.
fn chart(ui: &mut egui::Ui, title: &str, pts: &[&runner::Bucket], bounds: &[usize], series: &[Series]) {
    let max_of = |f: fn(&runner::Bucket) -> f64| pts.iter().map(|b| f(b)).fold(0.0, f64::max);
    let shared = series.iter().filter(|s| !s.3).map(|s| max_of(s.2)).fold(1.0, f64::max);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).small().strong());
        for (name, color, f, _) in series {
            ui.label(RichText::new(format!("— {name} máx {:.0}", max_of(*f))).small().color(*color));
        }
    });
    let (resp, painter) = ui.allocate_painter(egui::vec2(ui.available_width(), 110.0), egui::Sense::hover());
    let rect = resp.rect;
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let n = pts.len();
    let x = |i: usize| rect.left() + rect.width() * i as f32 / (n - 1) as f32;
    for &b in bounds {
        painter.vline(x(b.min(n - 1)), rect.y_range(), egui::Stroke::new(1.0_f32, theme::muted().gamma_multiply(0.4)));
    }
    for (_, color, f, own) in series {
        let max = if *own { max_of(*f).max(1.0) } else { shared };
        let line = pts.iter().enumerate().map(|(i, b)| egui::pos2(x(i), rect.bottom() - rect.height() * 0.95 * (f(b) / max) as f32)).collect();
        painter.add(egui::Shape::line(line, egui::Stroke::new(1.5_f32, *color)));
    }
    if let Some(p) = resp.hover_pos() {
        let i = ((((p.x - rect.left()) / rect.width()) * (n - 1) as f32).round().max(0.0) as usize).min(n - 1);
        painter.vline(x(i), rect.y_range(), egui::Stroke::new(1.0_f32, theme::muted()));
        let b = pts[i];
        resp.on_hover_text_at_pointer(format!("{i} s · {} VUs · {} req/s · {} errores · p50 {} ms · p95 {} ms", b.vus, b.count, b.errors, b.p50, b.p95));
    }
}

/// Iniciar / Detener del runner y del estrés.
fn run_buttons(ui: &mut egui::Ui, running: bool, stop: &AtomicBool, start: &mut Option<bool>, stress: bool) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if running {
            if ui.button("Detener").clicked() {
                stop.store(true, Relaxed);
            }
            ui.spinner();
        } else {
            let label = if stress { "Iniciar batería" } else { "Ejecutar" };
            if ui.add(Button::new(RichText::new(label).strong().color(theme::on_accent())).fill(theme::accent())).clicked() {
                *start = Some(stress);
            }
        }
    });
}

/// Fila de derecha a izquierda del alto de un control (with_layout sola ocuparía todo el alto disponible).
fn row_rtl(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y + 4.0);
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), add);
}

fn status_color(s: u16) -> Color32 {
    match s {
        ..=299 => theme::ok(),
        300..=399 => theme::warn(),
        _ => theme::err(),
    }
}

/// DELETE y OPTIONS no caben en la columna de métodos.
fn short_method(m: &str) -> String {
    match m.to_uppercase().as_str() {
        "DELETE" => "DEL".into(),
        "OPTIONS" => "OPT".into(),
        other => other.into(),
    }
}

fn human_size(n: usize) -> String {
    match n {
        ..1024 => format!("{n} B"),
        ..1_048_576 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

fn ago(t: Instant) -> String {
    match t.elapsed().as_secs() {
        s @ ..60 => format!("hace {s} s"),
        s @ ..3600 => format!("hace {} min", s / 60),
        s => format!("hace {} h", s / 3600),
    }
}

/// Primera línea, recortada a `n` caracteres.
fn cut_line(s: &str, n: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() <= n { line.into() } else { line.chars().take(n - 1).chain(['…']).collect() }
}

fn floor_boundary(s: &str, max: usize) -> usize {
    (0..=max.min(s.len())).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0)
}

/// Recorta a `n` caracteres respetando UTF-8.
fn cut(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}\n…(recortado)", &s[..i]),
        None => s.to_string(),
    }
}

fn source_of(scan: &repo::Scan, file: &str) -> Source {
    Source { repo: scan.url.clone(), branch: scan.branch.clone(), file: file.into(), commit: scan.commit.clone() }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        if cookies::take_dirty() {
            self.persist(cookies::FILE);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::Enter)) {
            if ctx.memory(|m| m.has_focus(Id::new("question"))) {
                self.ask(ctx);
            } else {
                self.send_by_user(ctx);
            }
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::S)) {
            self.save_request();
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::T)) {
            self.new_tab();
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::W)) {
            self.request_close_tab(self.active_tab);
        }

        let bar = |fill: Color32| egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(12, 6));
        egui::TopBottomPanel::top("top").frame(bar(theme::deep())).show(ctx, |ui| self.top_bar(ui));
        if !self.error.is_empty() || !self.notice.is_empty() {
            egui::TopBottomPanel::top("banner").frame(bar(theme::surface())).show(ctx, |ui| {
                for (text, color) in [(&mut self.error, theme::err()), (&mut self.notice, theme::ok())] {
                    if text.is_empty() {
                        continue;
                    }
                    egui::Frame::new().fill(color.gamma_multiply(0.12)).stroke(egui::Stroke::new(1.0_f32, color.gamma_multiply(0.5))).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(if color == theme::err() { "✖" } else { "✔" }).color(color).strong());
                            ui.add(egui::Label::new(RichText::new(text.as_str()).color(color)).wrap());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.add(Button::new("×").frame(false)).clicked() {
                                    text.clear();
                                }
                            });
                        });
                    });
                }
            });
        }
        egui::TopBottomPanel::bottom("status").frame(bar(theme::deep())).show(ctx, |ui| self.status_bar(ui));
        egui::SidePanel::left("side")
            .default_width(300.0)
            .width_range(220.0..=520.0)
            .frame(egui::Frame::new().fill(theme::panel()).inner_margin(egui::Margin::symmetric(10, 4)))
            .show(ctx, |ui| self.sidebar(ui));
        if self.show_agent {
            egui::SidePanel::right("agent")
                .default_width(420.0)
                .frame(egui::Frame::new().fill(theme::panel()).inner_margin(10.0))
                .show(ctx, |ui| self.agent_panel(ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::surface()).inner_margin(egui::Margin { left: 14, right: 14, top: 8, bottom: 8 })).show(ctx, |ui| self.main_panel(ui));
        self.windows(ctx);
    }

    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        theme::panel().to_normalized_gamma_f32()
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_session();
    }
}
