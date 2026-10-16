//! Barra lateral: colecciones, entornos e historial.

use super::*;

impl App {
    pub(super) fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for (s, label) in [(Side::Cols, t("Colecciones", "Collections")), (Side::Envs, t("Entornos", "Environments")), (Side::History, t("Historial", "History"))] {
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

    pub(super) fn collections_view(&mut self, ui: &mut egui::Ui) {
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
                ui.label(RichText::new(t("Nueva colección", "New collection")).small().color(theme::muted()));
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut self.new_col).hint_text(t("Nombre", "Name")).desired_width(150.0));
                    let name = self.new_col.trim().to_string();
                    if ui.add_enabled(!name.is_empty(), Button::new(t("Crear", "Create"))).clicked() {
                        self.cols.push(Collection { name, ..Default::default() });
                        self.col_sel = self.cols.len() - 1;
                        self.new_col.clear();
                        self.persist(COLLECTIONS);
                        ui.close_menu();
                    }
                });
                ui.separator();
                if ui.button(t("Nuevo request   Ctrl+T", "New request   Ctrl+T")).clicked() {
                    act = Some(Act::NewTab);
                    ui.close_menu();
                }
                if ui.button(t("Importar OpenAPI / Postman / cURL…", "Import OpenAPI / Postman / cURL…")).clicked() {
                    self.show_import = true;
                    ui.close_menu();
                }
                if ui.button("Azure DevOps (OpenAPI)…").clicked() {
                    self.show_repos = true;
                    ui.close_menu();
                }
            })
            .response
            .on_hover_text(t("Nueva colección, request o importar", "New collection, request or import"));
            ui.add(TextEdit::singleline(&mut self.filter).hint_text(t("🔍  Buscar requests", "🔍  Search requests")).desired_width(ui.available_width()));
        });
        ui.add_space(4.0);

        if self.cols.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(t("Todavía no hay colecciones", "No collections yet")).strong());
                ui.label(RichText::new(t("Importa un contrato OpenAPI, pídeselo al agente, crea una con + o conecta un repo.", "Import an OpenAPI contract, ask the agent, create one with + or connect a repo.")).small().color(theme::muted()));
                ui.add_space(8.0);
                if ui.button(t("📥 Importar OpenAPI / Postman / cURL", "📥 Import OpenAPI / Postman / cURL")).clicked() {
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
                    if ui.button(t("▶ Ejecutar colección", "▶ Run collection")).clicked() {
                        *act = Some(Act::Run(ci, false));
                        ui.close_menu();
                    }
                    if ui.button(t("⚡ Prueba de estrés", "⚡ Stress test")).clicked() {
                        *act = Some(Act::Run(ci, true));
                        ui.close_menu();
                    }
                    if ui.button(t("Autorización…", "Authorization…")).clicked() {
                        *act = Some(Act::Auth(ci));
                        ui.close_menu();
                    }
                    if ui.button(t("Variables de colección", "Collection variables")).clicked() {
                        *act = Some(Act::Vars(ci));
                        ui.close_menu();
                    }
                    if ui.button(t("Nuevo request aquí", "New request here")).clicked() {
                        *act = Some(Act::NewIn(ci));
                        ui.close_menu();
                    }
                    if c.source.is_some() && ui.add_enabled(!busy, Button::new(t("Sincronizar con el repo", "Sync with repo"))).clicked() {
                        *act = Some(Act::Sync(ci));
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.label(RichText::new(t("Renombrar", "Rename")).small().color(theme::muted()));
                    if ui.text_edit_singleline(&mut c.name).changed() {
                        *act = Some(Act::Renamed(ci));
                    }
                    ui.separator();
                    if ui.button(RichText::new(t("Borrar colección", "Delete collection")).color(theme::err())).clicked() {
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
                            ui.label(RichText::new(tr!("repo {short} · {} @{}", "repo {short} · {} @{}", s.file, s.commit)).small().color(theme::muted()))
                                .on_hover_text(tr!("{}\nrama: {}", "{}\nbranch: {}", s.repo, if s.branch.is_empty() { t("(por defecto)", "(default)") } else { &s.branch }));
                        }
                        if c.requests.is_empty() {
                            ui.label(RichText::new(t("Sin requests", "No requests")).small().color(theme::muted()));
                        }
                        for &ri in &matches {
                            let r = &c.requests[ri];
                            // En colecciones de un repo: "+" = agregado localmente, "●" = modificado localmente.
                            let (mark, tip) = match (&c.source, &r.origin) {
                                (Some(_), None) => ("+", t("Agregado localmente: no está en el repo", "Added locally: not in the repo")),
                                (Some(_), Some(_)) if r.modified() => ("●", t("Modificado localmente: la sincronización no lo pisa", "Modified locally: sync will not overwrite it")),
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
                                if ui.button(t("Abrir", "Open")).clicked() {
                                    act = Some(Act::Open(ci, ri));
                                    ui.close_menu();
                                }
                                if ui.button(t("Abrir en pestaña nueva", "Open in new tab")).clicked() {
                                    act = Some(Act::OpenNew(ci, ri));
                                    ui.close_menu();
                                }
                                if ui.button(t("Duplicar", "Duplicate")).clicked() {
                                    act = Some(Act::Dup(ci, ri));
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui.button(RichText::new(t("Borrar", "Delete")).color(theme::err())).clicked() {
                                    act = Some(Act::DelReq(ci, ri));
                                    ui.close_menu();
                                }
                            });
                        }
                        ui.horizontal(|ui| {
                            if ui.small_button(t("▶ Ejecutar", "▶ Run")).on_hover_text(t("Runner: todos los requests en orden, con sus tests", "Runner: every request in order, with its tests")).clicked() {
                                act = Some(Act::Run(ci, false));
                            }
                            if ui.small_button(t("⚡ Estrés", "⚡ Stress")).on_hover_text(t("Baterías de carga / estrés con estadísticas por endpoint", "Load / stress batteries with per-endpoint stats")).clicked() {
                                act = Some(Act::Run(ci, true));
                            }
                            ui.menu_button("···", |ui| col_menu(ui, &mut act, c)).response.on_hover_text(t("Más acciones", "More actions"));
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
                copy.name = tr!("{} (copia)", "{} (copy)", copy.name);
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

    pub(super) fn envs_view(&mut self, ui: &mut egui::Ui) {
        row_rtl(ui, |ui| {
            let name = self.new_env.trim().to_string();
            let create = ui.add_enabled(!name.is_empty() && !self.envs.envs.contains_key(&name), Button::new(t("Crear", "Create")));
            ui.add(TextEdit::singleline(&mut self.new_env).hint_text(t("Nuevo entorno (dev, qa, prod…)", "New environment (dev, qa, prod…)")).desired_width(ui.available_width()));
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
            ui.label(RichText::new(t("Sin entornos. Crea uno para guardar baseUrl, tokens y demás variables.", "No environments. Create one to store baseUrl, tokens and other variables.")).small().color(theme::muted()));
            return;
        }
        let (mut edit, mut activate, mut delete) = (None, None, None);
        for name in self.envs.envs.keys() {
            let active = *name == self.envs.active;
            ui.horizontal(|ui| {
                let mark = RichText::new(if active { "●" } else { "○" }).color(if active { theme::ok() } else { theme::muted() });
                if ui.add(egui::Label::new(mark).sense(egui::Sense::click())).on_hover_text(t("Usar este entorno", "Use this environment")).clicked() {
                    activate = Some(name.clone());
                }
                let text = if active { RichText::new(name).strong() } else { RichText::new(name) };
                let r = ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| ui.selectable_label(self.env_edit == *name, text)).inner;
                if r.clicked() {
                    edit = Some(name.clone());
                }
                r.context_menu(|ui| {
                    if ui.button(t("Usar este entorno", "Use this environment")).clicked() {
                        activate = Some(name.clone());
                        ui.close_menu();
                    }
                    if ui.button(RichText::new(t("Borrar", "Delete")).color(theme::err())).clicked() {
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
                widgets::pill(ui, t("activo", "active"), theme::ok());
            }
        });
        ui.label(RichText::new(t("Úsalas como {{nombre}}. Precedencia: script > datos > entorno > colección.", "Use them as {{name}}. Precedence: script > data > environment > collection.")).small().color(theme::muted()));
        ScrollArea::vertical().id_salt("env_vars").auto_shrink([false; 2]).show(ui, |ui| {
            if widgets::kv_table(ui, "env_table", &mut self.env_rows, false, ("baseUrl", t("https://api.ejemplo.com", "https://api.example.com"))) {
                if let Some(v) = self.envs.envs.get_mut(&self.env_edit) {
                    *v = rows_vars(&self.env_rows);
                }
                self.persist(ENVIRONMENTS);
            }
        });
    }

    pub(super) fn history_view(&mut self, ui: &mut egui::Ui) {
        if self.history.is_empty() {
            ui.label(RichText::new(t("Aquí aparecen los requests que envíes en esta sesión.", "Requests you send in this session show up here.")).small().color(theme::muted()));
            return;
        }
        let mut open = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr!("{} envíos", "{} sends", self.history.len())).small().color(theme::muted()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button(t("Limpiar", "Clear")).clicked() {
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
                let r = ui.interact(r.response.rect, Id::new(("hist", i)), egui::Sense::click()).on_hover_text(t("Abrir en una pestaña", "Open in a tab"));
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
}
