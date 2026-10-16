//! Interfaz de escritorio (egui). `App` tiene el estado; cada archivo agrega una parte de su comportamiento.

mod agent_panel;
mod auth_editor;
mod bars;
mod dialogs;
mod editor;
mod repos;
mod response;
mod run_window;
mod send;
mod session;
mod sidebar;
mod style;
pub mod theme;
mod util;
pub mod widgets;

use auth_editor::*;
use style::*;
use util::*;
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

use eframe::egui::{self, Align, Button, Color32, Id, Key, Layout, Modifiers, RichText, ScrollArea, TextEdit, TextStyle};
use serde::de::DeserializeOwned;

use crate::{
    i18n::{self, t},
    tr,
    domain::{
        actions::{self, Action, Effects, Launch, Ws},
        model::{self, AgentCfg, Collection, Environments, RepoCfg, Request, Response, Settings, Source, Stage},
        overlay,
        vars::{self, Vars, resolve},
    },
    engine::{
        runner::{self, SendDone},
        script::{ReqParts, Scopes, TestResult},
    },
    import::{self, Imported},
    infra::{
        agent, auth, cookies, http, repo,
        storage::{self, AGENTS, COLLECTIONS, ENVIRONMENTS, SETTINGS},
    },
};

const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
const HISTORY_MAX: usize = 100;
const VIEW_LIMIT: usize = 2_000_000; // ponytail: el visor de egui se vuelve lento con cuerpos enormes; se recorta
const REPO_CACHE: &str = "repos"; // clones dispersos de los repos, junto a los JSON de la app
const AGENT_ROUNDS: u8 = 5; // rondas automáticas (acción -> resultado -> agente) por pregunta

/// Abre la ventana principal.
pub fn run() -> eframe::Result {
    // Instalada, la app arranca en cualquier carpeta: sus JSON van a la carpeta de datos del usuario.
    let dir = storage::data_dir();
    if storage::create_private_dir(&dir).and_then(|_| std::env::set_current_dir(&dir)).is_err() {
        eprintln!("{}", tr!("No se pudo usar {}; los datos quedan en la carpeta actual.", "Could not use {}; data stays in the current folder.", dir.display()));
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("api-connect").with_inner_size([1440.0, 900.0]),
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
    batteries: Vec<model::Battery>,
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
    oauth_pending: Option<(Receiver<Result<auth::Token, String>>, AuthTarget)>,
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
    import_url: String,
    /// Descarga de un contrato en curso: (texto, URL de origen) y si la pidió el agente.
    import_pending: Option<(Receiver<Result<(String, Option<String>), String>>, bool)>,
    import_keep_scripts: bool,
    import_err: String,
    confirm: Option<Confirm>,

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
    /// El agente lanzó una ejecución (runner, estrés o importación) y espera su resultado.
    agent_waits_task: bool,
    /// Acciones del agente que esperan aprobación (borrados o envío a un host nuevo).
    pending_actions: Vec<Action>,
    undo: Option<Snapshot>,
    /// Hosts a los que TÚ enviaste o que vienen de un contrato importado: el agente puede enviar ahí sin preguntar.
    trusted_hosts: HashSet<String>,
    contract_view: Option<ContractView>,
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
        if self.settings.agent_panel {
            egui::SidePanel::right("agent")
                .default_width(380.0)
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
