//! Panel del agente de IA: contexto, conversación y aplicación de sus acciones.

use super::*;

impl App {
    /// Solo nombres de variables: los valores (tokens, claves) no salen de la máquina.
    pub(super) fn agent_context(&self) -> String {
        let or = |s: &str, empty: &str| if s.trim().is_empty() { empty.to_string() } else { s.to_string() };
        let keys = |v: Option<&Vars>| v.map(|v| v.keys().cloned().collect::<Vec<_>>().join(", ")).filter(|s| !s.is_empty()).unwrap_or(i18n::t("(ninguna)", "(none)").into());
        let last = match &self.resp {
            Some(Ok(r)) => tr!("Última respuesta: {} {}\n{}", "Last response: {} {}\n{}", r.status, r.status_text, cut(&r.body, 3000)),
            Some(Err(e)) => tr!("Último envío falló: {e}", "Last send failed: {e}"),
            None => i18n::t("Aún no hay respuesta.", "No response yet.").into(),
        };
        let mut workspace: Vec<String> = Vec::new();
        for c in &self.cols {
            let src = c.source.as_ref().map(|s| tr!(" [contrato: {}]", " [contract: {}]", s.file)).unwrap_or_default();
            workspace.push(format!("- {}{src} ({} requests)", c.name, c.requests.len()));
            for r in c.requests.iter().take(40) {
                workspace.push(format!("    {} {}", r.method, r.name));
            }
        }
        let open = match self.cur {
            Some((c, i)) => format!("\"{} / {}\"", self.cols[c].name, self.cols[c].requests[i].name),
            None => i18n::t("(nuevo, sin guardar)", "(new, unsaved)").into(),
        };
        let mut parts = vec![
            tr!("Request abierto {open}: {} {}", "Open request {open}: {} {}", self.ed.method, self.ed.url),
            format!("Headers:\n{}", or(&self.ed.headers, i18n::t("(ninguno)", "(none)"))),
            format!("Body:\n{}", or(&cut(&self.ed.body, 2000), i18n::t("(vacío)", "(empty)"))),
            format!("Pre-request:\n{}", or(&self.ed.pre, i18n::t("(vacío)", "(empty)"))),
            format!("Tests:\n{}", or(&self.ed.tests, i18n::t("(vacío)", "(empty)"))),
            tr!(
                "Auth del request: {} · de la colección activa: {} (los secretos no se muestran; usa {{{{variables}}}})",
                "Request auth: {} · active collection auth: {} (secrets are not shown; use {{{{variables}}}})",
                actions::auth_kind(&self.ed.auth),
                self.active_col().map_or(i18n::t("(ninguna)", "(none)"), |i| actions::auth_kind(&self.cols[i].auth))
            ),
        ];
        if let Some(o) = &self.ed.origin {
            let changes: Vec<String> = overlay::diff(&o.base, &self.ed.parts())
                .iter()
                .map(|c| tr!("- {}: contrato {} → tuyo {}", "- {}: contract {} → yours {}", c.path, c.contract.as_deref().unwrap_or(i18n::t("(no existe)", "(missing)")), c.yours.as_deref().unwrap_or(i18n::t("(quitado)", "(removed)"))))
                .collect();
            let issues = overlay::validate(&self.ed.body, &o.schema);
            parts.push(tr!(
                "Este request viene del CONTRATO ({}), que es de solo lectura; lo que editas es la versión del usuario.\nCambios del usuario vs contrato:\n{}\nAvisos de validación:\n{}\nSchema del body según el contrato:\n{}",
                "This request comes from the CONTRACT ({}), which is read-only; what you edit is the user's version.\nUser changes vs contract:\n{}\nValidation warnings:\n{}\nBody schema per the contract:\n{}",
                o.key,
                if changes.is_empty() { i18n::t("(ninguno)", "(none)").into() } else { changes.join("\n") },
                if issues.is_empty() { i18n::t("(ninguno)", "(none)").into() } else { issues.join("\n") },
                if o.schema.is_null() { i18n::t("(sin schema)", "(no schema)").into() } else { cut(&o.schema.to_string(), 3000) },
            ));
        }
        parts.extend([
            tr!("Entornos: {} (activo: {})", "Environments: {} (active: {})", self.envs.envs.keys().cloned().collect::<Vec<_>>().join(", "), or(&self.envs.active, i18n::t("ninguno", "none"))),
            tr!("Variables de entorno disponibles: {}", "Available environment variables: {}", keys(self.envs.envs.get(&self.envs.active))),
            tr!("Colección activa: {}", "Active collection: {}", self.active_col().map_or(i18n::t("(ninguna)", "(none)").into(), |i| self.cols[i].name.clone())),
            tr!("Variables de colección disponibles: {}", "Available collection variables: {}", keys(self.active_col().map(|i| &self.cols[i].variables))),
            tr!("Colecciones:\n{}", "Collections:\n{}", if workspace.is_empty() { i18n::t("(ninguna)", "(none)").into() } else { cut(&workspace.join("\n"), 4000) }),
            tr!(
                "Baterías de estrés: {}",
                "Stress batteries: {}",
                self.settings.batteries.iter().map(|b| format!("{} ({})", b.name, b.stages.iter().map(|s| s.label()).collect::<Vec<_>>().join(" → "))).collect::<Vec<_>>().join("; ")
            ),
            last,
        ]);
        if !self.run.steps.is_empty() || !self.run.reports.is_empty() {
            parts.push(tr!("Última ejecución:\n{}", "Last run:\n{}", cut(&self.run_summary(), 3000)));
        }
        parts.join("\n\n")
    }

    /// Pregunta del usuario: arranca un ciclo nuevo del agente (rondas y punto de deshacer).
    pub(super) fn ask(&mut self, ctx: &egui::Context) {
        let q = self.question.trim().to_string();
        if q.is_empty() || self.asking.is_some() || self.agents.get(self.agent_sel).is_none() {
            return;
        }
        let sent = if self.with_context { tr!("{q}\n\n---\nContexto:\n{}", "{q}\n\n---\nContext:\n{}", self.agent_context()) } else { q.clone() };
        self.chat.push(ChatMsg { role: "user", shown: q, sent, error: false });
        self.question.clear();
        self.pending_actions.clear();
        self.agent_rounds = AGENT_ROUNDS;
        self.undo = Some(Snapshot { cols: self.cols.clone(), batteries: self.settings.batteries.clone(), envs: self.envs.clone(), ed: self.ed.clone(), cur: self.cur, col_sel: self.col_sel });
        self.ask_agent(ctx);
    }

    /// Siguiente ronda automática: el último mensaje "tool" ya tiene el resultado; se añade el contexto actualizado.
    pub(super) fn continue_agent(&mut self, ctx: &egui::Context) {
        if self.agent_rounds == 0 {
            self.chat.push(ChatMsg { role: "tool", shown: i18n::t("Límite de rondas automáticas alcanzado; pregúntale de nuevo para seguir.", "Automatic round limit reached; ask again to continue.").into(), sent: String::new(), error: true });
            return;
        }
        self.agent_rounds -= 1;
        let light = self.agent_context_light();
        if let Some(last) = self.chat.last_mut().filter(|m| m.role == "tool") {
            last.sent = tr!("{}\n\n---\nContexto actualizado:\n{light}", "{}\n\n---\nUpdated context:\n{light}", last.sent);
        }
        self.ask_agent(ctx);
    }

    /// Contexto corto para las rondas automáticas (el completo ya se envió con la pregunta).
    pub(super) fn agent_context_light(&self) -> String {
        tr!("Request abierto: {} {}\nBody:\n{}\nTests:\n{}", "Open request: {} {}\nBody:\n{}\nTests:\n{}", self.ed.method, self.ed.url, cut(&self.ed.body, 1500), cut(&self.ed.tests, 1500))
    }

    pub(super) fn ask_agent(&mut self, ctx: &egui::Context) {
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

    /// Aplica en orden las acciones del agente. Se detiene en la primera que necesite tu aprobación (borrar, estrés,
    /// scripts o un host que no usaste tú). `approved_first` = aprobaste la primera; las demás se revisan otra vez
    /// una por una, con el estado de ese momento (una edición previa puede cambiar el host de un envío).
    pub(super) fn run_actions(&mut self, ctx: &egui::Context, list: Vec<Result<Action, String>>, approved_first: bool) {
        if list.is_empty() {
            return;
        }
        let (mut log, mut fx, mut failed) = (Vec::new(), Effects::default(), false);
        let mut items = list.into_iter();
        let mut pre_ok = approved_first;
        while let Some(item) = items.next() {
            let ok = std::mem::take(&mut pre_ok);
            let a = match item {
                Ok(a) => a,
                Err(e) => {
                    log.push(format!("✖ {e}"));
                    failed = true;
                    continue;
                }
            };
            let launches = matches!(a, Action::RunCollection { .. } | Action::RunStress { .. } | Action::ImportOpenapi { .. });
            if launches && fx.launch.is_some() {
                log.push(tr!("✖ {}: solo una ejecución por respuesta; pídela en la siguiente", "✖ {}: only one run per reply; request it in the next one", a.describe()));
                failed = true;
                continue;
            }
            let untrusted = self.action_hosts(&a).is_none_or(|hs| hs.iter().any(|h| !self.is_trusted(h)));
            if (a.needs_approval() || untrusted) && !ok {
                self.pending_actions = std::iter::once(a).chain(items.by_ref().filter_map(Result::ok)).collect();
                break;
            }
            let mut ws = Ws {
                cols: &mut self.cols,
                batteries: &mut self.settings.batteries,
                envs: &mut self.envs,
                ed: &mut self.ed,
                cur: &mut self.cur,
                col_sel: &mut self.col_sel,
            };
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
        if fx.settings {
            self.persist(SETTINGS);
        }
        self.sync_buffers();
        if !log.is_empty() {
            let text = tr!("Acciones aplicadas:\n{}", "Actions applied:\n{}", log.join("\n"));
            self.chat.push(ChatMsg { role: "tool", shown: text.clone(), sent: text, error: false });
        }
        if !self.pending_actions.is_empty() {
            self.chat.push(ChatMsg { role: "tool", shown: tr!("⚠ Esperando tu aprobación: {}", "⚠ Waiting for your approval: {}", self.pending_text()), sent: String::new(), error: false });
            return;
        }
        if fx.send {
            self.agent_waits_send = true;
            self.send(ctx);
        }
        if let Some(l) = fx.launch {
            self.launch(ctx, l);
        } else if failed && !fx.send {
            self.continue_agent(ctx); // que el agente vea el error y lo corrija
        }
    }

    pub(super) fn approve_pending(&mut self, ctx: &egui::Context) {
        let list = std::mem::take(&mut self.pending_actions);
        // Aprobar un envío o una corrida confía en esos hosts (calculados ahora, con el estado que viste).
        if let Some(first) = list.first() {
            let hosts = self.action_hosts(first).unwrap_or_default();
            self.trusted_hosts.extend(hosts);
        }
        self.run_actions(ctx, list.into_iter().map(Ok).collect(), true);
    }

    /// Qué se aprueba exactamente: la primera acción pendiente, con los hosts a los que iría.
    pub(super) fn pending_text(&self) -> String {
        let Some(a) = self.pending_actions.first() else { return String::new() };
        let hosts = match self.action_hosts(a) {
            Some(h) if h.is_empty() => String::new(),
            Some(h) => format!(" → {}", h.join(", ")),
            None => i18n::t(" → (URL sin resolver: revisa las variables)", " → (unresolved URL: check the variables)").into(),
        };
        let rest = self.pending_actions.len() - 1;
        let more = if rest > 0 { tr!("\n(después hay {rest} acción(es) más; cada una se revisa por separado)", "\n({rest} more action(s) after this; each one is reviewed separately)") } else { String::new() };
        format!("{}{hosts}{more}", a.describe())
    }

    pub(super) fn reject_pending(&mut self) {
        let list: Vec<String> = std::mem::take(&mut self.pending_actions).iter().map(Action::describe).collect();
        let text = tr!("El usuario rechazó: {}", "The user rejected: {}", list.join(", "));
        self.chat.push(ChatMsg { role: "tool", shown: text.clone(), sent: text, error: false });
    }

    pub(super) fn undo_agent(&mut self) {
        let Some(s) = self.undo.take() else { return };
        (self.cols, self.settings.batteries, self.envs, self.ed, self.cur, self.col_sel) = (s.cols, s.batteries, s.envs, s.ed, s.cur, s.col_sel);
        self.pending_actions.clear();
        self.persist(COLLECTIONS);
        self.persist(ENVIRONMENTS);
        self.persist(SETTINGS);
        self.sync_buffers();
        let text = i18n::t("El usuario deshizo todos los cambios del agente de esta pregunta.", "The user undid all the agent's changes from this question.").to_string();
        self.chat.push(ChatMsg { role: "tool", shown: text.clone(), sent: text, error: false });
    }

    pub(super) fn agent_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            let shown = self.agents.get(self.agent_sel).map_or(i18n::t("(sin agentes)", "(no agents)").to_string(), |a| format!("{} · {}", a.name, a.model));
            egui::ComboBox::from_id_salt("agent").width(210.0).selected_text(shown).show_ui(ui, |ui| {
                for (i, a) in self.agents.iter().enumerate() {
                    ui.selectable_value(&mut self.agent_sel, i, format!("{} ({} · {})", a.name, a.kind, a.model));
                }
            });
            if ui.button(i18n::t("Configurar", "Configure")).clicked() {
                self.agents_json = serde_json::to_string_pretty(&self.agents).unwrap_or_default();
                self.agents_err.clear();
                self.timeout_edit = self.settings.claude_timeout_secs;
                self.show_agents_cfg = true;
            }
            if ui.button(i18n::t("Nueva conversación", "New conversation")).clicked() {
                self.chat.clear();
                self.pending_actions.clear();
            }
        });
        if self.undo.is_some() && self.chat.iter().any(|m| m.role == "tool") {
            ui.horizontal(|ui| {
                if ui.button(i18n::t("↶ Deshacer cambios del agente", "↶ Undo the agent's changes")).on_hover_text(i18n::t("Vuelve colecciones, entornos, baterías y editor a como estaban antes de tu última pregunta", "Restores collections, environments, batteries and editor to how they were before your last question")).clicked() {
                    self.undo_agent();
                }
            });
        }
        ui.separator();

        let mut insert: Option<(Tab, String)> = None;
        let pending = self.pending_text();
        let (mut approve, mut reject) = (false, false);
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            let busy = self.asking.is_some() || self.agent_waits_send;
            let busy = busy || self.agent_waits_task;
            let label = if self.agent_waits_task {
                i18n::t("Esperando la ejecución…", "Waiting for the run…")
            } else if self.agent_waits_send {
                i18n::t("Esperando la respuesta HTTP…", "Waiting for the HTTP response…")
            } else if busy {
                i18n::t("Pensando…", "Thinking…")
            } else {
                i18n::t("Pedir al agente  (Ctrl+Enter)", "Ask the agent  (Ctrl+Enter)")
            };
            if ui.add_enabled(!busy, Button::new(RichText::new(label).strong().color(theme::on_accent())).fill(theme::accent())).clicked() {
                self.ask(&ctx);
            }
            ui.add(
                TextEdit::multiline(&mut self.question)
                    .id(Id::new("question"))
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .hint_text(i18n::t("¿Qué quieres hacer con tu API? Ej: importa https://petstore3.swagger.io/api/v3/openapi.json y prueba los GET", "What do you want to do with your API? E.g.: import https://petstore3.swagger.io/api/v3/openapi.json and test the GETs")),
            );
            ui.checkbox(&mut self.with_context, i18n::t("Compartir contexto (request abierto, colecciones, última respuesta; nunca los valores de variables)", "Share context (open request, collections, last response; never variable values)"));
            if !pending.is_empty() {
                let warn = theme::warn();
                egui::Frame::new().fill(warn.gamma_multiply(0.12)).stroke(egui::Stroke::new(1.5_f32, warn)).corner_radius(6.0).inner_margin(10.0).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        if ui.add(Button::new(RichText::new(i18n::t("Aprobar", "Approve")).strong().color(theme::on_accent())).fill(theme::ok())).clicked() {
                            approve = true;
                        }
                        if ui.button(RichText::new(i18n::t("Rechazar", "Reject")).color(theme::err())).clicked() {
                            reject = true;
                        }
                    });
                    ui.add(egui::Label::new(RichText::new(&pending)).wrap());
                    ui.label(RichText::new(i18n::t("⚠ El agente espera tu aprobación para:", "⚠ The agent is waiting for your approval to:")).strong().color(warn));
                });
            }
            ui.separator();
            ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false; 2]).show(ui, |ui| {
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    if self.chat.is_empty() {
                        ui.add_space(24.0);
                        ui.label(RichText::new(i18n::t("Dile al agente qué quieres lograr", "Tell the agent what you want to achieve")).size(18.0).strong());
                        ui.label(
                            RichText::new(i18n::t("Importa contratos OpenAPI, crea y edita requests, colecciones y entornos, escribe tests, corre colecciones y prepara pruebas de estrés. Ves cada cambio, puedes deshacerlo, y te pide aprobación antes de borrar, generar carga, escribir scripts, cambiar la auth o enviar a un host nuevo.", "It imports OpenAPI contracts, creates and edits requests, collections and environments, writes tests, runs collections and sets up stress tests. You see every change, can undo it, and it asks for approval before deleting, generating load, writing scripts, changing auth or sending to a new host."))
                                .color(theme::muted()),
                        );
                        ui.add_space(10.0);
                        ui.label(RichText::new(i18n::t("PRUEBA CON", "TRY")).small().color(theme::muted()));
                        for s in [
                            i18n::t("Importa el contrato https://petstore3.swagger.io/api/v3/openapi.json", "Import the contract https://petstore3.swagger.io/api/v3/openapi.json"),
                            i18n::t("Escribe tests para este request: status 200 y que la respuesta tenga los campos esperados", "Write tests for this request: status 200 and that the response has the expected fields"),
                            i18n::t("Corre la colección activa y explícame los fallos", "Run the active collection and explain the failures"),
                            i18n::t("Prepara una prueba de carga: 20 usuarios con rampa de 10 s y 1 minuto sostenido sobre el endpoint abierto", "Set up a load test: 20 users with a 10 s ramp-up and 1 minute sustained on the open endpoint"),
                            i18n::t("Crea un request de login que guarde el token en el entorno", "Create a login request that stores the token in the environment"),
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
                                    egui::CollapsingHeader::new(RichText::new(i18n::t("⚙ acciones para la plataforma", "⚙ platform actions")).color(theme::accent())).id_salt(&part).show(ui, |ui| {
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
                                    if ui.small_button(i18n::t("Copiar", "Copy")).clicked() {
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
}
