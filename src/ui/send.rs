//! Envío de requests (con scripts y auth), tokens OAuth 2.0 y sondeo de tareas en segundo plano.

use super::*;

impl App {
    /// Lo que tú envías marca el host como de confianza para el agente.
    pub(super) fn send_by_user(&mut self, ctx: &egui::Context) {
        if let Some(h) = self.current_host() {
            self.trusted_hosts.insert(h);
        }
        self.send(ctx);
    }

    pub(super) fn send(&mut self, ctx: &egui::Context) {
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

    pub(super) fn finish_send(&mut self, ctx: &egui::Context, p: Pending, d: SendDone) {
        self.tests = d.tests;
        self.logs = d.logs;
        self.resp_view = match &d.result {
            Ok(r) => {
                let pretty = serde_json::from_str::<serde_json::Value>(&r.body)
                    .and_then(|v| serde_json::to_string_pretty(&v))
                    .unwrap_or_else(|_| r.body.clone());
                if pretty.len() > VIEW_LIMIT {
                    let cut = (0..=VIEW_LIMIT).rev().find(|&i| pretty.is_char_boundary(i)).unwrap_or(0);
                    tr!("{}\n\n…(respuesta recortada a {VIEW_LIMIT} bytes para mostrarla)", "{}\n\n…(response truncated to {VIEW_LIMIT} bytes for display)", &pretty[..cut])
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
                Some(Err(e)) => tr!("falló: {e}", "failed: {e}"),
                None => String::new(),
            };
            let tests: Vec<String> = self
                .tests
                .iter()
                .map(|t| format!("{} {}{}", if t.pass { "✔" } else { "✖" }, t.name, t.error.as_ref().map(|e| format!(" — {e}")).unwrap_or_default()))
                .collect();
            let text = tr!(
                "Resultado del envío: {summary}\nTests: {}",
                "Send result: {summary}\nTests: {}",
                if tests.is_empty() { i18n::t("(ninguno)", "(none)").into() } else { tests.join("\n") }
            );
            self.tool_result(ctx, text);
        }
    }

    /// Resultado de algo que pidió el agente; sigue la conversación cuando ya no espera nada más.
    pub(super) fn tool_result(&mut self, ctx: &egui::Context, text: String) {
        self.chat.push(ChatMsg { role: "tool", shown: text.clone(), sent: text, error: false });
        if !self.agent_waits_send && !self.agent_waits_task {
            self.continue_agent(ctx);
        }
    }

    /// Guarda las variables que cambiaron los scripts en su entorno y colección (si siguen existiendo).
    pub(super) fn write_back(&mut self, env: &str, col: Option<(usize, String)>, scopes: Scopes) {
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

    /// "Obtener token": corre fuera de la UI (authorization code espera al navegador hasta 3 minutos).
    pub(super) fn fetch_token(&mut self, ctx: &egui::Context, target: AuthTarget) {
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
            let _ = tx.send(auth::fetch_token(&client, &o, &vars));
            ctx.request_repaint();
        });
        self.oauth_pending = Some((rx, target));
    }

    pub(super) fn finish_token(&mut self, res: Result<auth::Token, String>, target: AuthTarget) {
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
            auth::store(o, &t);
            self.notice = i18n::t("Token de OAuth 2.0 obtenido.", "OAuth 2.0 token obtained.").into();
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

    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        if let Some((rx, target)) = self.oauth_pending.take() {
            match rx.try_recv() {
                Ok(r) => self.finish_token(r, target),
                Err(TryRecvError::Empty) => self.oauth_pending = Some((rx, target)),
                Err(TryRecvError::Disconnected) => self.error = i18n::t("la obtención del token falló inesperadamente", "fetching the token failed unexpectedly").into(),
            }
        }
        self.poll_run(ctx);
        if let Some((rx, for_agent)) = self.import_pending.take() {
            match rx.try_recv() {
                Ok(r) => self.finish_import(ctx, r, for_agent),
                Err(TryRecvError::Empty) => self.import_pending = Some((rx, for_agent)),
                Err(TryRecvError::Disconnected) => self.finish_import(ctx, Err(i18n::t("la descarga falló inesperadamente", "the download failed unexpectedly").into()), for_agent),
            }
        }
        if let Some(p) = self.sending.take() {
            match p.rx.try_recv() {
                Ok(done) => self.finish_send(ctx, p, done),
                Err(TryRecvError::Empty) => self.sending = Some(p),
                Err(TryRecvError::Disconnected) => self.resp = Some(Err(i18n::t("el envío falló inesperadamente", "the send failed unexpectedly").into())),
            }
        }
        if let Some((rx, col)) = self.repo_pending.take() {
            match rx.try_recv() {
                Ok(r) => self.finish_repo(r, col),
                Err(TryRecvError::Empty) => self.repo_pending = Some((rx, col)),
                Err(TryRecvError::Disconnected) => self.repo_msg = i18n::t("la conexión con el repo falló inesperadamente", "the repo connection failed unexpectedly").into(),
            }
        }
        if let Some(rx) = self.asking.take() {
            let reply = match rx.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Empty) => {
                    self.asking = Some(rx);
                    return;
                }
                Err(TryRecvError::Disconnected) => Err(i18n::t("el agente falló inesperadamente", "the agent failed unexpectedly").into()),
            };
            let (text, error) = match reply {
                Ok(t) if t.trim().is_empty() => (i18n::t("(sin respuesta)", "(no reply)").to_string(), false),
                Ok(t) => (t, false),
                Err(e) => (e, true),
            };
            self.chat.push(ChatMsg { role: "assistant", shown: text.clone(), sent: text.clone(), error });
            if !error {
                self.run_actions(ctx, actions::extract(&text), false);
            }
        }
    }
}
