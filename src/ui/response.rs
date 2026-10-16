//! Panel de respuesta: cuerpo, headers, tests y consola.

use super::*;

impl App {
    pub(super) fn response_panel(&mut self, ui: &mut egui::Ui) {
        // El panel guarda el alto de su contenido: sin esto, el estado vacío lo encoge para siempre.
        ui.set_min_height(ui.available_height());
        let pass = self.tests.iter().filter(|t| t.pass).count();
        ui.horizontal(|ui| {
            ui.label(RichText::new(i18n::t("Respuesta", "Response")).strong());
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
                    widgets::pill(ui, i18n::t("Error de conexión", "Connection error"), theme::err());
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
                    (RespTab::Console, RichText::new(format!("{}{}", i18n::t("Consola", "Console"), if self.logs.is_empty() { String::new() } else { format!(" ({})", self.logs.len()) }))),
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
                None if self.sending.is_some() => empty(ui, i18n::t("Enviando…", "Sending…"), i18n::t("La respuesta aparecerá aquí", "The response will appear here")),
                None => empty(ui, i18n::t("Envía el request para ver la respuesta", "Send the request to see the response"), i18n::t("Ctrl+Enter · las respuestas no salen de tu máquina", "Ctrl+Enter · responses never leave your machine")),
                Some(Err(e)) => {
                    egui::Frame::new().fill(theme::err().gamma_multiply(0.12)).stroke(egui::Stroke::new(1.0_f32, theme::err().gamma_multiply(0.5))).corner_radius(6.0).inner_margin(10.0).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(i18n::t("No se obtuvo respuesta", "No response received")).strong().color(theme::err()));
                        ui.add(egui::Label::new(RichText::new(e).monospace()).wrap());
                        ui.label(RichText::new(i18n::t("Revisa la URL, que el servidor esté arriba y que las {{variables}} existan en el entorno.", "Check the URL, that the server is up and that the {{variables}} exist in the environment.")).small().color(theme::muted()));
                    });
                }
                Some(Ok(r)) => {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.raw, false, "Pretty");
                        ui.selectable_value(&mut self.raw, true, "Raw");
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button(i18n::t("Copiar", "Copy")).clicked() {
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
                _ => empty(ui, i18n::t("Sin headers", "No headers"), i18n::t("Envía el request primero", "Send the request first")),
            },
            RespTab::Cookies => {
                let set: Vec<&String> = match &self.resp {
                    Some(Ok(r)) => r.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie")).map(|(_, v)| v).collect(),
                    _ => Vec::new(),
                };
                if set.is_empty() {
                    empty(ui, i18n::t("Esta respuesta no fijó cookies", "This response set no cookies"), i18n::t("Las que lleguen se guardan solas y se envían a su dominio", "Incoming cookies are saved and sent to their domain automatically"));
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
                        if ui.button(i18n::t("Ver todas las cookies guardadas", "View all saved cookies")).clicked() {
                            self.show_cookies = true;
                        }
                    });
                }
            }
            RespTab::Tests if self.tests.is_empty() => empty(ui, i18n::t("Este request no tiene tests", "This request has no tests"), i18n::t("Agrégalos en la pestaña Tests: hay fragmentos listos para usar", "Add them in the Tests tab: there are ready-to-use snippets")),
            RespTab::Tests => {
                ScrollArea::vertical().id_salt("resp_tests").auto_shrink([false; 2]).show(ui, |ui| {
                    for t in &self.tests {
                        egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                widgets::pill(ui, if t.pass { i18n::t("PASA", "PASS") } else { i18n::t("FALLA", "FAIL") }, if t.pass { theme::ok() } else { theme::err() });
                                ui.label(&t.name);
                            });
                            if let Some(e) = &t.error {
                                ui.label(RichText::new(e).small().monospace().color(theme::err()));
                            }
                        });
                    }
                });
            }
            RespTab::Console if self.logs.is_empty() => empty(ui, i18n::t("Consola vacía", "Empty console"), i18n::t("console.log(...) en tus scripts aparece aquí", "console.log(...) in your scripts shows up here")),
            RespTab::Console => {
                ScrollArea::vertical().id_salt("resp_logs").auto_shrink([false; 2]).show(ui, |ui| {
                    for l in &self.logs {
                        ui.add(egui::Label::new(RichText::new(format!("› {l}")).monospace()).wrap());
                    }
                });
            }
        }
    }
}
