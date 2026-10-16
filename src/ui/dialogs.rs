//! Ventanas emergentes: importar, confirmar, variables, auth, cookies, agentes y repos.

use super::*;

impl App {
    pub(super) fn windows(&mut self, ctx: &egui::Context) {
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
                    RichText::new(t("Usa tus credenciales de git (Git Credential Manager); la app no guarda tokens. Solo se descarga la carpeta /collections del repo.", "Uses your git credentials (Git Credential Manager); the app stores no tokens. Only the repo's /collections folder is downloaded."))
                        .small()
                        .color(theme::muted()),
                );
                ui.horizontal(|ui| {
                    ui.label(t("Repo", "Repo"));
                    ui.add(TextEdit::singleline(&mut self.repo_url).hint_text(t("https://dev.azure.com/org/proyecto/_git/repo", "https://dev.azure.com/org/project/_git/repo")).desired_width(ui.available_width()));
                });
                ui.horizontal(|ui| {
                    ui.label(t("Rama", "Branch"));
                    ui.add(TextEdit::singleline(&mut self.repo_branch).hint_text(t("(por defecto)", "(default)")).desired_width(180.0));
                    let busy = self.repo_pending.is_some();
                    let label = if busy { t("Conectando…", "Connecting…") } else { t("Conectar / actualizar", "Connect / refresh") };
                    if ui.add_enabled(!busy && !self.repo_url.trim().is_empty(), Button::new(label)).clicked() {
                        act = Some(RepoAct::Connect);
                    }
                });
                if !self.settings.repos.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(t("Recientes:", "Recent:")).small());
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
                                                if ui.button(t("Sincronizar", "Sync")).on_hover_text(t("Trae cambios del repo sin pisar tus cambios locales", "Pulls repo changes without overwriting your local edits")).clicked() {
                                                    act = Some(RepoAct::Sync(ci, fi));
                                                }
                                            }
                                            None => {
                                                if ui.button(t("Importar", "Import")).clicked() {
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
                                self.repo_msg = tr!("{path} importado como colección \"{}\"", "{path} imported as collection \"{}\"", remote.name);
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
            let downloading = self.import_pending.as_ref().is_some_and(|(_, agent)| !agent);
            egui::Window::new(t("Importar", "Import")).id(Id::new("import")).open(&mut open).collapsible(false).default_width(720.0).show(ctx, |ui| {
                ui.label(RichText::new(t("Contrato OpenAPI 3 / Swagger 2 (YAML o JSON), colección de Postman v2 o comando cURL. El formato se detecta solo.", "OpenAPI 3 / Swagger 2 contract (YAML or JSON), Postman v2 collection or cURL command. The format is detected automatically.")).color(theme::muted()));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(t("Desde URL", "From URL"));
                    let go = ui
                        .add(TextEdit::singleline(&mut self.import_url).hint_text(t("https://api.ejemplo.com/openapi.json", "https://api.example.com/openapi.json")).desired_width(420.0))
                        .lost_focus()
                        && ui.input(|i| i.key_pressed(Key::Enter));
                    let url = self.import_url.trim().to_string();
                    if (ui.add_enabled(!url.is_empty() && !downloading, Button::new(t("Descargar e importar", "Download and import"))).clicked() || go) && !url.is_empty() && !downloading {
                        self.import_err.clear();
                        self.start_import(ctx, url, false);
                    }
                    if downloading {
                        ui.spinner();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(t("o pega el contenido abajo, o", "or paste the content below, or"));
                    if ui.button(t("Abrir archivo…", "Open file…")).clicked() {
                        // Diálogo nativo del sistema; bloquea la ventana mientras está abierto, como cualquier diálogo modal.
                        let picked = rfd::FileDialog::new()
                            .set_title(t("Importar OpenAPI, colección de Postman o cURL", "Import OpenAPI, Postman collection or cURL"))
                            .add_filter("OpenAPI / Postman / cURL", &["yaml", "yml", "json", "txt", "sh", "curl"])
                            .add_filter(t("Todos los archivos", "All files"), &["*"])
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
                ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
                    ui.add(TextEdit::multiline(&mut self.import_text).code_editor().desired_rows(14).desired_width(f32::INFINITY).hint_text("openapi: 3.0.0 …   ·   { \"info\": …, \"item\": […] }   ·   curl https://…"));
                });
                ui.checkbox(&mut self.import_keep_scripts, t("Mantener los scripts de colecciones de Postman (se ejecutan al enviar y pueden leer tus variables)", "Keep Postman collection scripts (they run when sending and can read your variables)"));
                if !self.import_err.is_empty() {
                    ui.colored_label(theme::err(), &self.import_err);
                }
                if ui.add_enabled(!self.import_text.trim().is_empty(), Button::new(RichText::new(t("Importar", "Import")).strong().color(theme::on_accent())).fill(theme::accent())).clicked() {
                    let text = std::mem::take(&mut self.import_text);
                    self.finish_import(ctx, Ok((text.clone(), None)), false);
                    if !self.import_err.is_empty() {
                        self.import_text = text; // que no se pierda lo pegado si falló
                    }
                }
            });
            self.show_import &= open;
        }

        if self.show_agents_cfg {
            let mut open = true;
            egui::Window::new(t("Agentes", "Agents")).id(Id::new("agents_cfg")).open(&mut open).collapsible(false).default_width(560.0).show(ctx, |ui| {
                ui.label(RichText::new(t("claude: ejecuta `claude -p` sin herramientas ni MCP. Campos: model, effort (low…max).", "claude: runs `claude -p` with no tools or MCP. Fields: model, effort (low…max).")).small());
                ui.label(RichText::new(t("ollama: llama a {url}/api/chat. Campos: model, url (por defecto http://127.0.0.1:11434).", "ollama: calls {url}/api/chat. Fields: model, url (default http://127.0.0.1:11434).")).small());
                ui.add(TextEdit::multiline(&mut self.agents_json).code_editor().desired_rows(14).desired_width(f32::INFINITY));
                ui.horizontal(|ui| {
                    ui.label(t("Timeout de Claude:", "Claude timeout:"));
                    ui.add(egui::DragValue::new(&mut self.timeout_edit).range(10..=3600).suffix(" s"));
                    ui.label(RichText::new(t("se guarda en settings.json", "saved in settings.json")).small().color(theme::muted()));
                });
                if !self.agents_err.is_empty() {
                    ui.colored_label(theme::err(), &self.agents_err);
                }
                if ui.button(t("Guardar", "Save")).clicked() {
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
                        Ok(_) => self.agents_err = t("Cada agente necesita name, kind (claude | ollama) y model", "Each agent needs name, kind (claude | ollama) and model").into(),
                        Err(e) => self.agents_err = tr!("JSON inválido: {e}", "Invalid JSON: {e}"),
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
                    RichText::new(t("Se guardan de las respuestas (Set-Cookie) y se envían solas a su dominio y path. Un header Cookie escrito a mano tiene prioridad.", "Saved from responses (Set-Cookie) and sent automatically to their domain and path. A hand-written Cookie header takes priority."))
                        .small()
                        .color(theme::muted()),
                );
                ui.horizontal(|ui| {
                    let (d, n, v) = &mut self.new_cookie;
                    ui.add(TextEdit::singleline(d).hint_text(t("dominio (api.ejemplo.com)", "domain (api.example.com)")).desired_width(200.0));
                    ui.add(TextEdit::singleline(n).hint_text(t("nombre", "name")).desired_width(140.0));
                    ui.add(TextEdit::singleline(v).hint_text(t("valor", "value")).desired_width(200.0));
                    let ok = !d.trim().is_empty() && !n.trim().is_empty();
                    if ui.add_enabled(ok, Button::new(t("Agregar", "Add"))).clicked() {
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
                        if !jar.is_empty() && ui.button(RichText::new(t("Borrar todas", "Delete all")).color(theme::err())).clicked() {
                            jar.clear();
                            changed = true;
                        }
                    });
                });
                ui.separator();
                if jar.is_empty() {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new(t("No hay cookies guardadas", "No saved cookies")).color(theme::muted())));
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
                            if ui.small_button(t("Borrar dominio", "Delete domain")).clicked() {
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
                                let exp = c.expires.map_or(t("sesión", "session").to_string(), |e| vars::iso8601(e, 0)[..16].replace('T', " "));
                                ui.label(RichText::new(exp).small().color(theme::muted())).on_hover_text(t("Vencimiento (UTC)", "Expires (UTC)"));
                                if c.secure {
                                    widgets::pill(ui, "Secure", theme::ok());
                                }
                                if c.http_only {
                                    widgets::pill(ui, "HttpOnly", theme::cyan());
                                }
                                if ui.small_button("×").on_hover_text(t("Borrar", "Delete")).clicked() {
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
            let title = tr!("Autorización · {}", "Authorization · {}", self.cols.get(ci).map_or("", |c| c.name.as_str()));
            let fetching = self.oauth_pending.is_some();
            let mut get = false;
            egui::Window::new(title).id(Id::new("col_auth")).open(&mut open).collapsible(false).default_width(640.0).show(ctx, |ui| {
                ui.label(RichText::new(t("La heredan los requests de esta colección cuyo tipo sea \"Heredar de la colección\".", "Inherited by this collection's requests whose type is \"Inherit from collection\".")).small().color(theme::muted()));
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
                ui.label(RichText::new(t("Disponibles como {{nombre}} en los requests de esta colección. El entorno activo las pisa si repite el nombre.", "Available as {{name}} in this collection's requests. The active environment overrides any with the same name.")).small().color(theme::muted()));
                ui.add_space(4.0);
                ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    if widgets::kv_table(ui, "col_vars_table", &mut self.col_rows, false, ("baseUrl", t("https://api.ejemplo.com", "https://api.example.com"))) {
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
                Confirm::Env(n) => (t("Borrar entorno", "Delete environment"), tr!("¿Borrar el entorno \"{n}\" y sus variables?", "Delete environment \"{n}\" and its variables?"), t("Borrar", "Delete")),
                Confirm::Col(i) => (t("Borrar colección", "Delete collection"), tr!("¿Borrar la colección \"{}\" y todos sus requests?", "Delete collection \"{}\" and all its requests?", self.cols.get(*i).map_or("", |c| c.name.as_str())), t("Borrar", "Delete")),
                Confirm::Req(c, r) => (
                    t("Borrar request", "Delete request"),
                    tr!("¿Borrar \"{}\"?", "Delete \"{}\"?", self.cols.get(*c).and_then(|c| c.requests.get(*r)).map_or("", |r| r.name.as_str())),
                    t("Borrar", "Delete"),
                ),
                Confirm::CloseTab(_) => (t("Cambios sin guardar", "Unsaved changes"), t("Esta pestaña tiene cambios sin guardar. ¿Cerrarla igual?", "This tab has unsaved changes. Close it anyway?").to_string(), t("Cerrar sin guardar", "Close without saving")),
            };
            egui::Window::new(title).id(Id::new("confirm")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
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
                    if ui.button(t("Cancelar", "Cancel")).clicked() {
                        self.confirm = None;
                    }
                });
            });
        }
    }
}
