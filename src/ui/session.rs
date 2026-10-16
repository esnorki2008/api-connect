//! Ciclo de vida de la app: carga y guardado de datos, pestañas y request abierto.

use super::*;

pub(super) fn load_or<T: DeserializeOwned>(path: &'static str, default: impl FnOnce() -> T, errors: &mut Vec<String>, blocked: &mut Vec<&'static str>) -> T {
    match storage::load(path) {
        Ok(v) => v.unwrap_or_else(default),
        Err(e) => {
            errors.push(tr!("No se pudo leer {e}. No se guardarán cambios en ese archivo hasta corregirlo.", "Could not read {e}. Changes to that file will not be saved until it is fixed."));
            blocked.push(path);
            default()
        }
    }
}

impl App {
    pub(super) fn new() -> Self {
        let (mut errors, mut blocked) = (Vec::new(), Vec::new());
        let cols = load_or(COLLECTIONS, Vec::new, &mut errors, &mut blocked);
        let envs = load_or(ENVIRONMENTS, Environments::default, &mut errors, &mut blocked);
        let agents = load_or(AGENTS, model::default_agents, &mut errors, &mut blocked);
        let settings = load_or(SETTINGS, Settings::default, &mut errors, &mut blocked);
        i18n::set(i18n::from_env().unwrap_or(settings.language));
        cookies::replace(load_or(cookies::FILE, Vec::new, &mut errors, &mut blocked));
        cookies::take_dirty();
        cookies::enable(true);
        let mut app = App {
            client: http::client(),
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
            import_url: String::new(),
            import_pending: None,
            import_keep_scripts: false,
            import_err: String::new(),
            confirm: None,
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
            agent_waits_task: false,
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
    pub(super) fn restore_tabs(&mut self) {
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

    pub(super) fn save_session(&mut self) {
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

    pub(super) fn persist(&mut self, file: &'static str) {
        if self.blocked.contains(&file) {
            self.error = tr!("{file} no se pudo leer al iniciar; no se sobrescribe. Corrígelo y reinicia la app.", "{file} could not be read at startup; it will not be overwritten. Fix it and restart the app.");
            return;
        }
        let r = match file {
            COLLECTIONS => storage::save(file, &self.cols),
            ENVIRONMENTS => storage::save(file, &self.envs),
            SETTINGS => storage::save(file, &self.settings),
            cookies::FILE => storage::save(file, &cookies::snapshot()),
            _ => storage::save(file, &self.agents),
        };
        if let Err(e) = r {
            self.error = e;
        }
    }

    pub(super) fn active_col(&self) -> Option<usize> {
        (self.col_sel < self.cols.len()).then_some(self.col_sel)
    }

    /// Rellena los editores de variables desde el modelo (al cambiar de entorno/colección o tras un envío).
    pub(super) fn sync_buffers(&mut self) {
        if !self.envs.envs.contains_key(&self.env_edit) {
            self.env_edit = self.envs.active.clone();
        }
        self.env_rows = vars_rows(self.envs.envs.get(&self.env_edit));
        self.col_rows = vars_rows(self.vars_win.and_then(|i| self.cols.get(i)).map(|c| &c.variables));
    }

    /// Envío o agente en curso: sus resultados van a la pestaña activa, así que no se cambia de pestaña.
    pub(super) fn busy(&self) -> bool {
        self.sending.is_some() || self.asking.is_some() || self.agent_waits_send
    }

    pub(super) fn stash(&mut self) -> TabState {
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

    pub(super) fn unstash(&mut self, t: TabState) {
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

    pub(super) fn switch_tab(&mut self, i: usize) {
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

    pub(super) fn new_tab(&mut self) {
        if self.busy() {
            return;
        }
        self.tabs.push(TabState { col_sel: self.col_sel, ..Default::default() });
        self.switch_tab(self.tabs.len() - 1);
    }

    pub(super) fn close_tab(&mut self, i: usize) {
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

    pub(super) fn request_close_tab(&mut self, i: usize) {
        let dirty = if i == self.active_tab { is_dirty(&self.cols, self.cur, &self.ed) } else { self.tabs.get(i).is_some_and(|t| is_dirty(&self.cols, t.cur, &t.ed)) };
        if dirty {
            self.confirm = Some(Confirm::CloseTab(i));
        } else {
            self.close_tab(i);
        }
    }

    /// Ajusta el request abierto tras borrar un request (`r`) o una colección entera (`r` = None).
    pub(super) fn after_delete(&mut self, c: usize, r: Option<usize>) {
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
    pub(super) fn move_request(&mut self, from: (usize, usize), to_col: usize, to_idx: usize) {
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
    pub(super) fn open(&mut self, c: usize, r: usize) {
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

    pub(super) fn new_request(&mut self) {
        self.cur = None;
        self.ed = Request { method: "GET".into(), ..Default::default() };
        (self.resp, self.tests, self.logs) = (None, Vec::new(), Vec::new());
        self.resp_view.clear();
    }

    pub(super) fn save_request(&mut self) {
        if self.cols.is_empty() {
            self.cols.push(Collection { name: "General".into(), ..Default::default() });
            self.col_sel = 0;
        }
        let ci = self.active_col().unwrap_or(0);
        if self.ed.name.trim().is_empty() {
            self.ed.name = if self.ed.url.is_empty() { t("Sin nombre", "Untitled").into() } else { self.ed.url.clone() };
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

    /// Variables visibles para el request abierto (colección < entorno), sin las locales del script.
    pub(super) fn visible_vars(&self) -> Vars {
        let mut v = self.active_col().map(|i| self.cols[i].variables.clone()).unwrap_or_default();
        v.extend(self.envs.envs.get(&self.envs.active).cloned().unwrap_or_default());
        v
    }

    pub(super) fn current_host(&self) -> Option<String> {
        actions::host_of(&resolve(&self.ed.url, &self.visible_vars()))
    }

    pub(super) fn is_trusted(&self, host: &str) -> bool {
        self.trusted_hosts.contains(host) || self.trusted_hosts.contains(host.split(':').next().unwrap_or(""))
    }

    /// Hosts a los que una acción del agente mandaría requests; None si alguno no se puede resolver.
    pub(super) fn action_hosts(&self, a: &Action) -> Option<Vec<String>> {
        match a {
            Action::Send {} => {
                let col = self.active_col().map(|i| &self.cols[i]);
                let mut hosts = vec![self.current_host()?];
                hosts.extend(oauth_hosts(&model::with_effective_auth(&self.ed, col).auth, &self.visible_vars())?);
                Some(hosts)
            }
            Action::RunCollection { collection, .. } => {
                let Some(c) = self.cols.iter().find(|c| c.name.eq_ignore_ascii_case(collection.trim())) else { return Some(Vec::new()) };
                let mut vars = c.variables.clone();
                vars.extend(self.envs.envs.get(&self.envs.active).cloned().unwrap_or_default());
                let mut hosts = Vec::new();
                for r in &c.requests {
                    hosts.push(actions::host_of(&resolve(&r.url, &vars))?);
                    hosts.extend(oauth_hosts(&model::with_effective_auth(r, Some(c)).auth, &vars)?);
                }
                Some(hosts)
            }
            Action::ImportOpenapi { url: Some(u), .. } => actions::host_of(u).map(|h| vec![h]),
            _ => Some(Vec::new()),
        }
    }
}

/// Hosts a los que OAuth 2.0 manda tus credenciales (token_url y, si hay login, auth_url); None si no se resuelven.
fn oauth_hosts(auth: &model::Auth, vars: &Vars) -> Option<Vec<String>> {
    let model::Auth::Oauth2(o) = auth else { return Some(Vec::new()) };
    let mut urls = vec![&o.token_url];
    if o.grant == "authorization_code" {
        urls.push(&o.auth_url);
    }
    urls.into_iter().map(|u| actions::host_of(&resolve(u, vars))).collect()
}
