//! Barra superior, barra de estado y pestañas de requests.

use super::*;

impl App {
    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("⚡ api-connect").strong().size(17.0).color(theme::accent()));
            ui.label(RichText::new(t("cliente de APIs REST · agentic first · local", "REST API client · agentic first · local")).small().color(theme::muted()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let on = self.settings.agent_panel;
                let agent = RichText::new(t("✨ Agente", "✨ Agent")).strong().color(if on { theme::accent() } else { ui.visuals().text_color() });
                if ui.add(Button::new(agent).selected(on)).on_hover_text(t("Mostrar u ocultar el agente: puede hacer por ti todo lo que hace la app", "Show or hide the agent: it can do for you everything the app does")).clicked() {
                    self.settings.agent_panel = !on;
                    self.persist(SETTINGS);
                }
                let n = cookies::snapshot().len();
                if ui.button(format!("Cookies ({n})")).on_hover_text(t("Cookies guardadas: se envían solas a su dominio", "Saved cookies: sent automatically to their domain")).clicked() {
                    self.show_cookies = !self.show_cookies;
                }
                let other = if i18n::en() { i18n::Language::Es } else { i18n::Language::En };
                if ui.button(if i18n::en() { "ES" } else { "EN" }).on_hover_text(tr!("Cambiar a {}", "Switch to {}", other.name())).clicked() {
                    i18n::set(other);
                    self.settings.language = other;
                    self.persist(SETTINGS);
                }
                let label = if self.settings.light_theme { t("Tema oscuro", "Dark theme") } else { t("Tema claro", "Light theme") };
                if ui.button(label).on_hover_text(t("Cambiar entre One Dark y One Light (se guarda)", "Switch between One Dark and One Light (saved)")).clicked() {
                    self.settings.light_theme = !self.settings.light_theme;
                    apply_theme(ui.ctx(), self.settings.light_theme);
                    self.persist(SETTINGS);
                }
                if ui.add_enabled(!self.cols.is_empty(), Button::new(t("▶ Runner / ⚡ Estrés", "▶ Runner / ⚡ Stress"))).on_hover_text(t("Corre la colección actual con sus tests, o una prueba de estrés", "Run the current collection with its tests, or a stress test")).clicked() {
                    self.open_runner(self.active_col().unwrap_or(0), false);
                }
                if ui.button(t("📥 Importar", "📥 Import")).on_hover_text(t("OpenAPI / Swagger (archivo, texto o URL), colección de Postman o cURL", "OpenAPI / Swagger (file, text or URL), Postman collection or cURL")).clicked() {
                    self.show_import = true;
                }
                ui.separator();
                let before = self.envs.active.clone();
                let shown = if self.envs.active.is_empty() { t("Sin entorno", "No environment").to_string() } else { self.envs.active.clone() };
                egui::ComboBox::from_id_salt("env").width(170.0).selected_text(RichText::new(shown).strong()).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.envs.active, String::new(), t("Sin entorno", "No environment"));
                    for name in self.envs.envs.keys() {
                        ui.selectable_value(&mut self.envs.active, name.clone(), name);
                    }
                });
                ui.label(RichText::new(t("Entorno", "Environment")).small().color(theme::muted()));
                if self.envs.active != before {
                    self.env_edit = self.envs.active.clone();
                    self.persist(ENVIRONMENTS);
                    self.sync_buffers();
                }
            });
        });
    }

    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let n: usize = self.cols.iter().map(|c| c.requests.len()).sum();
            let small = |t: String| RichText::new(t).small().color(theme::muted());
            ui.label(small(tr!("{} colecciones · {n} requests", "{} collections · {n} requests", self.cols.len())));
            ui.separator();
            ui.label(small(tr!("entorno: {}", "environment: {}", if self.envs.active.is_empty() { t("ninguno", "none") } else { &self.envs.active })));
            if self.sending.is_some() {
                ui.separator();
                ui.spinner();
                ui.label(small(t("enviando…", "sending…").into()));
            }
            if self.asking.is_some() {
                ui.separator();
                ui.spinner();
                ui.label(small(t("el agente está pensando…", "the agent is thinking…").into()));
            }
            if self.run.rx.is_some() {
                ui.separator();
                ui.spinner();
                ui.label(small(t("runner en curso", "runner in progress").into()));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(small(t("Ctrl+Enter enviar · Ctrl+S guardar · Ctrl+T nueva pestaña · Ctrl+W cerrar", "Ctrl+Enter send · Ctrl+S save · Ctrl+T new tab · Ctrl+W close").into()));
            });
        });
    }

    pub(super) fn tabs_bar(&mut self, ui: &mut egui::Ui) {
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
                        t("Nuevo request", "New request").into()
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
                                ui.label(RichText::new("●").small().color(theme::warn())).on_hover_text(t("Cambios sin guardar", "Unsaved changes"));
                            }
                            if ui.add(Button::new(RichText::new("×").color(theme::muted())).frame(false)).on_hover_text(t("Cerrar (Ctrl+W)", "Close (Ctrl+W)")).clicked() {
                                close = Some(i);
                            }
                        });
                    });
                    if active {
                        let r = out.response.rect;
                        ui.painter().hline(r.x_range(), r.top() + 0.5, egui::Stroke::new(2.0_f32, theme::accent()));
                    } else if busy {
                        out.response.on_hover_text(t("Espera a que termine el envío o el agente para cambiar de pestaña", "Wait for the send or the agent to finish before switching tabs"));
                    }
                }
                if ui.add(Button::new(RichText::new(" + ").strong().color(theme::muted())).frame(false)).on_hover_text(t("Nueva pestaña (Ctrl+T)", "New tab (Ctrl+T)")).clicked() {
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
}
