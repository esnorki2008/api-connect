//! Panel central: editor del request y diferencias con el contrato.

use super::*;

impl App {
    pub(super) fn main_panel(&mut self, ui: &mut egui::Ui) {
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
                    ui.label(RichText::new(t("Guardar en", "Save to")).small().color(theme::muted()));
                    let before = self.col_sel;
                    let shown = self.cols.get(self.col_sel).map_or(t("(nueva: General)", "(new: General)").to_string(), |c| c.name.clone());
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
                let save = if dirty { Button::new(RichText::new(t("Guardar", "Save")).strong().color(theme::on_accent())).fill(theme::accent()) } else { Button::new(t("Guardar", "Save")) };
                if ui.add(save).on_hover_text("Ctrl+S").clicked() {
                    self.save_request();
                }
                if self.ed.modified()
                    && ui.button(t("Descartar cambios locales", "Discard local changes")).on_hover_text(t("Vuelve a la versión del repo de la última sincronización", "Go back to the repo version from the last sync")).clicked()
                {
                    let base = self.ed.origin.as_ref().map(|o| o.base.clone()).unwrap_or_default();
                    self.ed.set_parts(base.clone());
                    if let Some((c, i)) = self.cur {
                        self.cols[c].requests[i].set_parts(base);
                        self.persist(COLLECTIONS);
                    }
                }
                if dirty {
                    ui.label(RichText::new(t("● sin guardar", "● unsaved")).small().color(theme::warn()));
                }
                ui.add(
                    TextEdit::singleline(&mut self.ed.name)
                        .hint_text(t("Request sin nombre", "Untitled request"))
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
                    let label = if self.sending.is_some() { t("Enviando…", "Sending…") } else { t("Enviar", "Send") };
                    let send = Button::new(RichText::new(label).strong().size(15.0).color(theme::on_accent())).fill(theme::accent()).min_size(egui::vec2(96.0, 30.0));
                    if ui.add_enabled(self.sending.is_none(), send).on_hover_text("Ctrl+Enter").clicked() {
                        self.send_by_user(&ctx);
                    }
                    let mut lay = widgets::line_layouter(Lang::Plain, 14.0);
                    ui.add(
                        TextEdit::singleline(&mut self.ed.url)
                            .hint_text(t("{{baseUrl}}/recurso?x=1   —   pega aquí una URL", "{{baseUrl}}/resource?x=1   —   paste a URL here"))
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
                    0 => t("Contrato", "Contract").to_string(),
                    n => tr!("Contrato · {n} cambio{}", "Contract · {n} change{}", if n == 1 { "" } else { "s" }),
                };
                ui.selectable_value(&mut self.tab, Tab::Contract, label).on_hover_text(t("Tu versión comparada con el contrato del repo (solo lectura)", "Your version compared with the repo contract (read-only)"));
                if n_issues > 0 && ui.link(RichText::new(tr!("⚠ {n_issues} aviso{}", "⚠ {n_issues} warning{}", if n_issues == 1 { "" } else { "s" })).color(theme::warn())).clicked() {
                    self.tab = Tab::Contract;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| match self.tab {
                Tab::Headers => {
                    ui.checkbox(&mut self.bulk, t("Editar como texto", "Edit as text"));
                }
                Tab::Body => {
                    if ui.small_button(t("{ } Embellecer", "{ } Beautify")).on_hover_text(t("Formatea el JSON", "Format the JSON")).clicked() {
                        match serde_json::from_str::<serde_json::Value>(&self.ed.body).and_then(|v| serde_json::to_string_pretty(&v)) {
                            Ok(p) => self.ed.body = p,
                            Err(e) => self.error = tr!("El body no es JSON válido ({e}). Si usas {{{{vars}}}} sin comillas, ponlas entre comillas para formatear.", "The body is not valid JSON ({e}). If you use {{{{vars}}}} without quotes, quote them to format."),
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
    pub(super) fn refresh_contract_view(&mut self) {
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

    pub(super) fn contract_tab(&mut self, ui: &mut egui::Ui) {
        let (Some(o), Some(view)) = (self.ed.origin.clone(), self.contract_view.as_ref()) else { return };
        let mut restore: Option<String> = None;
        ScrollArea::vertical().id_salt("contract").max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
            ui.label(RichText::new(tr!("Contrato: {}  ·  lo que editas es tu versión; el contrato no se modifica", "Contract: {}  ·  you edit your own version; the contract is not changed", o.key)).small().color(theme::muted()));
            if view.changes.is_empty() {
                ui.label(t("Tu versión es igual al contrato.", "Your version matches the contract."));
            } else {
                egui::Grid::new("changes").striped(true).num_columns(4).show(ui, |ui| {
                    ui.strong(t("Campo", "Field"));
                    ui.strong(t("Contrato", "Contract"));
                    ui.strong(t("Tu versión", "Your version"));
                    ui.label("");
                    ui.end_row();
                    for c in &view.changes {
                        ui.monospace(&c.path);
                        ui.label(RichText::new(cut(c.contract.as_deref().unwrap_or(t("(no existe)", "(missing)")), 80)).color(theme::muted()));
                        let yours = c.yours.as_deref().unwrap_or(t("(quitado)", "(removed)"));
                        // Campo tuyo que el contrato no tiene: se conserva y se marca.
                        let out = c.contract.is_none() && c.path.starts_with("body.");
                        ui.label(if out { RichText::new(tr!("{} ⚠ fuera de contrato", "{} ⚠ not in contract", cut(yours, 80))).color(theme::warn()) } else { RichText::new(cut(yours, 80)) });
                        if ui.small_button(t("Restaurar", "Restore")).on_hover_text(t("Deja este campo como en el contrato", "Set this field back to the contract")).clicked() {
                            restore = Some(c.path.clone());
                        }
                        ui.end_row();
                    }
                });
            }
            if !view.issues.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(t("Validación contra el schema (no bloquea el envío):", "Schema validation (does not block sending):")).strong());
                for i in &view.issues {
                    ui.label(RichText::new(format!("⚠ {i}")).color(theme::warn()).monospace());
                }
            }
            egui::CollapsingHeader::new(t("Ver contrato (solo lectura)", "View contract (read-only)")).show(ui, |ui| {
                let b = &o.base;
                let text = format!("{} {}\n\n{}\n\n{}", b.method, b.url, b.headers, b.body);
                ui.add(TextEdit::multiline(&mut text.as_str()).code_editor().desired_width(f32::INFINITY));
            });
        });
        if let Some(path) = restore {
            self.ed.set_parts(overlay::restore(&o.base, &self.ed.parts(), &path));
        }
    }

    pub(super) fn request_editor(&mut self, ui: &mut egui::Ui) {
        let mut get_token = None;
        ScrollArea::vertical().id_salt(("editor", self.tab as u8)).auto_shrink([false; 2]).show(ui, |ui| match self.tab {
            Tab::Params => {
                let (base, mut rows) = widgets::url_to_rows(&self.ed.url);
                ui.label(RichText::new(t("Parámetros de query: se sincronizan con la URL. Los {{vars}} se resuelven al enviar.", "Query parameters: kept in sync with the URL. {{vars}} are resolved when sending.")).small().color(theme::muted()));
                if widgets::kv_table(ui, "params", &mut rows, false, (t("clave", "key"), t("valor", "value"))) {
                    self.ed.url = widgets::rows_to_url(&base, &rows);
                }
            }
            Tab::Headers if self.bulk => {
                ui.label(RichText::new(t("Un header por línea: Clave: valor. Empieza con // para desactivarlo.", "One header per line: Key: value. Start with // to disable it.")).small().color(theme::muted()));
                widgets::code_editor(ui, &mut self.ed.headers, Lang::Plain, "Content-Type: application/json\nAuthorization: Bearer {{token}}", 10);
            }
            Tab::Headers => {
                let mut rows = widgets::headers_to_rows(&self.ed.headers);
                if widgets::kv_table(ui, "headers", &mut rows, true, ("Content-Type", "application/json")) {
                    self.ed.headers = widgets::rows_to_headers(&rows);
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(t("Rápidos:", "Quick:")).small().color(theme::muted()));
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
                widgets::code_editor(ui, &mut self.ed.body, lang, t("{\n  \"clave\": \"{{valor}}\"\n}", "{\n  \"key\": \"{{value}}\"\n}"), 14);
            }
            Tab::Pre | Tab::Tests => {
                let pre = self.tab == Tab::Pre;
                let snippets: &[(&str, &str)] = if pre {
                    &[
                        (t("Variable local", "Local variable"), "pm.variables.set('ts', Date.now());"),
                        (t("Header dinámico", "Dynamic header"), "pm.request.headers.upsert({ key: 'X-Request-Id', value: pm.variables.replaceIn('{{$guid}}') });"),
                        (t("Login previo", "Login first"), "const res = await pm.sendRequest({ url: pm.variables.replaceIn('{{baseUrl}}/login'), method: 'POST', header: { 'Content-Type': 'application/json' }, body: { mode: 'raw', raw: JSON.stringify({ user: 'demo' }) } });\npm.environment.set('token', res.json().token);"),
                    ]
                } else {
                    &[
                        ("Status 200", "pm.test('status 200', () => pm.response.to.have.status(200));"),
                        (t("Tiempo < 500 ms", "Time < 500 ms"), "pm.test('responde en menos de 500 ms', () => pm.expect(pm.response.responseTime).to.be.below(500));"),
                        (t("Tiene campo", "Has field"), "pm.test('trae id', () => pm.expect(pm.response.json()).to.have.property('id'));"),
                        (t("Guardar token", "Save token"), "pm.environment.set('token', pm.response.json().token);"),
                        (t("Siguiente request", "Next request"), "pm.setNextRequest('nombre del request'); // null termina la iteración"),
                    ]
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(t("Fragmentos:", "Snippets:")).small().color(theme::muted()));
                    for (label, code) in snippets {
                        if ui.small_button(*label).on_hover_text(*code).clicked() {
                            let target = if pre { &mut self.ed.pre } else { &mut self.ed.tests };
                            *target = if target.trim().is_empty() { code.to_string() } else { format!("{}\n{code}", target.trim_end()) };
                        }
                    }
                });
                let (text, hint) = if pre {
                    (&mut self.ed.pre, t("// Corre antes de enviar. pm.variables (solo este envío), pm.collectionVariables, pm.environment", "// Runs before sending. pm.variables (this send only), pm.collectionVariables, pm.environment"))
                } else {
                    (&mut self.ed.tests, t("// Corre con la respuesta. pm.test(...), pm.expect(...), pm.response.json()", "// Runs with the response. pm.test(...), pm.expect(...), pm.response.json()"))
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
}
