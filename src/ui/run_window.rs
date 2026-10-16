//! Ventana del runner de colecciones y de las pruebas de estrés, con sus gráficos.

use super::*;

pub(super) type Series = (&'static str, Color32, fn(&runner::Bucket) -> f64, bool);

/// Latencia y throughput segundo a segundo de todas las etapas, con líneas verticales entre etapas.
pub(super) fn timeline_charts(ui: &mut egui::Ui, reports: &[runner::StageReport]) {
    let pts: Vec<&runner::Bucket> = reports.iter().flat_map(|r| &r.timeline).collect();
    if pts.len() < 2 {
        return;
    }
    let bounds: Vec<usize> = reports.iter().scan(0, |acc, r| Some(std::mem::replace(acc, *acc + r.timeline.len()))).skip(1).collect();
    chart(ui, i18n::t("Latencia (ms)", "Latency (ms)"), &pts, &bounds, &[("p95", theme::accent(), |b| b.p95 as f64, false), ("p50", theme::muted(), |b| b.p50 as f64, false)]);
    chart(
        ui,
        "Throughput",
        &pts,
        &bounds,
        &[("req/s", theme::ok(), |b| b.count as f64, false), (i18n::t("errores/s", "errors/s"), theme::err(), |b| b.errors as f64, false), (i18n::t("VUs (escala propia)", "VUs (own scale)"), theme::warn(), |b| b.vus as f64, true)],
    );
}

/// Gráfico de líneas mínimo con el painter de egui; `own` = la serie usa su propia escala.
pub(super) fn chart(ui: &mut egui::Ui, title: &str, pts: &[&runner::Bucket], bounds: &[usize], series: &[Series]) {
    let max_of = |f: fn(&runner::Bucket) -> f64| pts.iter().map(|b| f(b)).fold(0.0, f64::max);
    let shared = series.iter().filter(|s| !s.3).map(|s| max_of(s.2)).fold(1.0, f64::max);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).small().strong());
        for (name, color, f, _) in series {
            ui.label(RichText::new(tr!("— {name} máx {:.0}", "— {name} max {:.0}", max_of(*f))).small().color(*color));
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
        resp.on_hover_text_at_pointer(tr!("{i} s · {} VUs · {} req/s · {} errores · p50 {} ms · p95 {} ms", "{i} s · {} VUs · {} req/s · {} errors · p50 {} ms · p95 {} ms", b.vus, b.count, b.errors, b.p50, b.p95));
    }
}

/// Iniciar / Detener del runner y del estrés.
pub(super) fn run_buttons(ui: &mut egui::Ui, running: bool, stop: &AtomicBool, start: &mut Option<bool>, stress: bool) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if running {
            if ui.button(i18n::t("Detener", "Stop")).clicked() {
                stop.store(true, Relaxed);
            }
            ui.spinner();
        } else {
            let label = if stress { i18n::t("Iniciar batería", "Start battery") } else { i18n::t("Ejecutar", "Run") };
            if ui.add(Button::new(RichText::new(label).strong().color(theme::on_accent())).fill(theme::accent())).clicked() {
                *start = Some(stress);
            }
        }
    });
}

impl App {
    pub(super) fn open_runner(&mut self, ci: usize, stress: bool) {
        if self.run.rx.is_none() {
            if self.run.col != ci {
                (self.run.target, self.run.setup) = (None, None);
            }
            self.run.col = ci;
        }
        self.run.open = true;
        self.run.stress = stress;
    }

    /// `plan` = etapas que pidió el agente (si no, las de la batería elegida en la ventana).
    pub(super) fn start_run(&mut self, ctx: &egui::Context, stress: bool, plan: Option<Vec<Stage>>) {
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
        let stages = match (plan, self.settings.batteries.get(r.battery)) {
            (Some(p), _) => p,
            (None, Some(b)) => b.stages.clone(),
            (None, None) => {
                let ramp = (r.ramp_secs > 0).then_some(model::Stage { vus: r.vus, secs: r.ramp_secs, ramp: true });
                ramp.into_iter().chain([model::Stage { vus: r.vus, secs: r.secs, ramp: false }]).collect()
            }
        };
        std::thread::spawn(move || {
            let res = runner::stress(&client, &setup, &targets, scopes, &stages, &stop, &mut |p| {
                let text = tr!(
                    "Etapa {}/{} · {} VUs · {}/{} s · {} requests · {} fallidos · {:.0} req/s",
                    "Stage {}/{} · {} VUs · {}/{} s · {} requests · {} failed · {:.0} req/s",
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

    pub(super) fn poll_run(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.run.rx.take() else { return };
        loop {
            match rx.try_recv() {
                Ok(RunMsg::Step(s)) => self.run.steps.push(s),
                Ok(RunMsg::Progress(p)) => self.run.progress = p,
                Ok(RunMsg::RunDone(scopes)) => {
                    let (env, col) = self.run.dest.clone();
                    self.write_back(&env, col, scopes);
                    break;
                }
                Ok(RunMsg::StressDone(r)) => {
                    match r {
                        Ok(reports) => self.run.reports = reports,
                        Err(e) => self.run.err = e,
                    }
                    break;
                }
                Err(TryRecvError::Empty) => {
                    self.run.rx = Some(rx);
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    self.run.err = i18n::t("la ejecución terminó inesperadamente", "the run ended unexpectedly").into();
                    break;
                }
            }
        }
        if std::mem::take(&mut self.agent_waits_task) {
            let text = self.run_summary();
            self.tool_result(ctx, text);
        }
    }

    /// Resultado de la última ejecución en texto compacto (para el agente y para copiar).
    pub(super) fn run_summary(&self) -> String {
        let col = self.cols.get(self.run.col).map_or("?", |c| c.name.as_str());
        if !self.run.err.is_empty() {
            return tr!("La ejecución sobre \"{col}\" falló: {}", "The run on \"{col}\" failed: {}", self.run.err);
        }
        if !self.run.stress {
            let steps = &self.run.steps;
            let failed: Vec<String> = steps
                .iter()
                .filter(|s| !s.passed())
                .take(30)
                .map(|s| {
                    let why = s.error.clone().unwrap_or_else(|| s.tests.iter().filter(|t| !t.pass).map(|t| format!("{}: {}", t.name, t.error.as_deref().unwrap_or(i18n::t("falló", "failed")))).collect::<Vec<_>>().join("; "));
                    format!("✖ #{} {} {} → {} · {why}", s.iteration, s.method, s.name, s.status)
                })
                .collect();
            let tests: usize = steps.iter().map(|s| s.tests.len()).sum();
            let tf = steps.iter().flat_map(|s| &s.tests).filter(|t| !t.pass).count();
            return tr!(
                "Runner de \"{col}\": {} requests · {} fallidos · {tests} tests · {tf} tests fallidos{}{}",
                "Runner on \"{col}\": {} requests · {} failed · {tests} tests · {tf} failed tests{}{}",
                steps.len(),
                steps.iter().filter(|s| !s.passed()).count(),
                if failed.is_empty() { "" } else { "\n" },
                failed.join("\n")
            );
        }
        let mut out = vec![tr!("Prueba de estrés sobre \"{col}\" (latencias en ms; éxito = 2xx/3xx):", "Stress test on \"{col}\" (latencies in ms; success = 2xx/3xx):")];
        for (i, r) in self.run.reports.iter().enumerate() {
            out.push(tr!("Etapa {} · {} VUs{} · {} s", "Stage {} · {} VUs{} · {} s", i + 1, r.vus, if r.ramp { i18n::t(" (rampa)", " (ramp)") } else { "" }, r.secs));
            for s in r.endpoints.iter().chain([&r.total]) {
                let codes: Vec<String> = s.statuses.iter().map(|(k, v)| format!("{k}×{v}")).collect();
                out.push(tr!(
                    "  {}: n={} err={:.2}% {:.1} req/s p50={} p95={} p99={} max={} códigos [{}]{}",
                    "  {}: n={} err={:.2}% {:.1} req/s p50={} p95={} p99={} max={} codes [{}]{}",
                    s.name,
                    s.count,
                    s.error_pct,
                    s.rps,
                    s.p50,
                    s.p95,
                    s.p99,
                    s.max,
                    codes.join(" "),
                    s.errors.first().map(|e| format!(" · error: {e}")).unwrap_or_default()
                ));
            }
        }
        out.join("\n")
    }

    /// Ejecución pedida por el agente (ya aprobada si hacía falta): se ve en la ventana del runner.
    pub(super) fn launch(&mut self, ctx: &egui::Context, l: Launch) {
        if self.run.rx.is_some() && !matches!(l, Launch::Import(_)) {
            self.tool_result(ctx, i18n::t("✖ ya hay una ejecución en curso; espera a que termine", "✖ a run is already in progress; wait for it to finish").into());
            return;
        }
        self.agent_waits_task = true;
        match l {
            Launch::Run { col, iterations } => {
                self.open_runner(col, false);
                (self.run.iterations, self.run.bail, self.run.data, self.run.data_name) = (iterations, false, Vec::new(), String::new());
                self.start_run(ctx, false, None);
            }
            Launch::Stress { col, target, setup, stages, label } => {
                self.open_runner(col, true);
                (self.run.target, self.run.setup) = (target, setup);
                self.run.battery = self.settings.batteries.iter().position(|b| b.name == label).unwrap_or(self.settings.batteries.len());
                self.start_run(ctx, true, Some(stages));
            }
            Launch::Import(source) => self.start_import(ctx, source, true),
        }
    }

    pub(super) fn runner_window(&mut self, ctx: &egui::Context) {
        let mut open = self.run.open;
        let title = format!("Runner · {}", self.cols.get(self.run.col).map_or("", |c| c.name.as_str()));
        let mut start = None;
        egui::Window::new(title).id(Id::new("runner")).open(&mut open).default_width(860.0).default_height(560.0).show(ctx, |ui| {
            let running = self.run.rx.is_some();
            let Some(col) = self.cols.get(self.run.col) else {
                ui.label(i18n::t("La colección ya no existe.", "The collection no longer exists."));
                return;
            };
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!running, |ui| {
                    ui.selectable_value(&mut self.run.stress, false, "▶ Runner");
                    ui.selectable_value(&mut self.run.stress, true, i18n::t("⚡ Estrés", "⚡ Stress"));
                });
                let env = if self.envs.active.is_empty() { i18n::t("(ninguno)", "(none)") } else { self.envs.active.as_str() };
                ui.label(RichText::new(tr!("entorno: {env}", "environment: {env}")).color(theme::muted()));
            });
            ui.separator();
            if !self.run.stress {
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!running, |ui| {
                        ui.label(i18n::t("Iteraciones", "Iterations"));
                        ui.add(egui::DragValue::new(&mut self.run.iterations).range(1..=100_000));
                        ui.checkbox(&mut self.run.bail, i18n::t("Detener al primer fallo", "Stop at first failure"));
                        if ui.button(i18n::t("Datos CSV/JSON…", "CSV/JSON data…")).on_hover_text(i18n::t("Una fila por iteración: {{columna}} y pm.iterationData.get('columna')", "One row per iteration: {{column}} and pm.iterationData.get('column')")).clicked() {
                            let picked = rfd::FileDialog::new()
                                .set_title(i18n::t("Archivo de datos para el runner", "Data file for the runner"))
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
                            ui.label(RichText::new(tr!("{} · {} filas", "{} · {} rows", self.run.data_name, self.run.data.len())).color(theme::muted()));
                            if ui.small_button("×").on_hover_text(i18n::t("Quitar datos", "Remove data")).clicked() {
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
                        ui.label(RichText::new(tr!("Ejecuta los {} requests de \"{}\" en orden, con sus tests", "Runs the {} requests of \"{}\" in order, with their tests", col.requests.len(), col.name)).size(16.0).color(theme::muted()));
                        ui.label(
                            RichText::new(i18n::t("Las variables que guarden los scripts pasan al siguiente request. Con un archivo de datos, cada fila es una iteración.", "Variables saved by scripts carry over to the next request. With a data file, each row is one iteration."))
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
                        tr!("Iteración {done}/{} · {total} requests · {failed} fallidos · {tests} tests · {tf} tests fallidos", "Iteration {done}/{} · {total} requests · {failed} failed · {tests} tests · {tf} failed tests", self.run.iterations),
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
                        None => tr!("Toda la colección ({})", "Whole collection ({})", col.requests.len()),
                    };
                    egui::ComboBox::from_id_salt("stress_target").width(300.0).selected_text(shown).show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.run.target, None, tr!("Toda la colección ({})", "Whole collection ({})", col.requests.len()));
                        for (i, r) in col.requests.iter().enumerate() {
                            ui.selectable_value(&mut self.run.target, Some(i), format!("{} {}", r.method, r.name));
                        }
                    });
                    ui.label(i18n::t("Batería", "Battery"));
                    let shown = self.settings.batteries.get(self.run.battery).map_or(i18n::t("Personalizada", "Custom").to_string(), |b| b.name.clone());
                    egui::ComboBox::from_id_salt("battery").selected_text(shown).show_ui(ui, |ui| {
                        for (i, b) in self.settings.batteries.iter().enumerate() {
                            ui.selectable_value(&mut self.run.battery, i, &b.name).on_hover_text(&b.description);
                        }
                        ui.selectable_value(&mut self.run.battery, nb, i18n::t("Personalizada", "Custom"));
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(i18n::t("Preparación", "Setup"));
                    let shown = self.run.setup.and_then(|i| col.requests.get(i)).map_or(i18n::t("(ninguna)", "(none)").to_string(), |r| format!("{} {}", r.method, r.name));
                    egui::ComboBox::from_id_salt("stress_setup").width(300.0).selected_text(shown).show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.run.setup, None, i18n::t("(ninguna)", "(none)"));
                        for (i, r) in col.requests.iter().enumerate() {
                            ui.selectable_value(&mut self.run.setup, Some(i), format!("{} {}", r.method, r.name));
                        }
                    });
                    ui.label(RichText::new(i18n::t("se envía una vez antes de medir (p. ej. el login que guarda el token)", "sent once before measuring (e.g. the login that stores the token)")).small().color(theme::muted()));
                });
                match self.settings.batteries.get(self.run.battery) {
                    Some(b) => {
                        let plan: Vec<String> = b.stages.iter().map(|s| s.label()).collect();
                        let total: u64 = b.stages.iter().map(|s| s.secs).sum();
                        ui.label(RichText::new(format!("{} — {} (≈{total} s)", b.description, plan.join(" → "))).small().color(theme::muted()));
                    }
                    None => {
                        ui.horizontal(|ui| {
                            ui.label(i18n::t("Usuarios virtuales", "Virtual users"));
                            ui.add(egui::DragValue::new(&mut self.run.vus).range(1..=model::MAX_VUS));
                            ui.label(i18n::t("Duración", "Duration"));
                            ui.add(egui::DragValue::new(&mut self.run.secs).range(1..=3600).suffix(" s"));
                            ui.label(i18n::t("Rampa previa", "Ramp-up"));
                            ui.add(egui::DragValue::new(&mut self.run.ramp_secs).range(0..=600).suffix(" s"))
                                .on_hover_text(i18n::t("Sube de 1 a los usuarios virtuales en este tiempo antes de sostener la carga (0 = de golpe)", "Ramps from 1 to the virtual users over this time before holding the load (0 = all at once)"));
                        });
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new(i18n::t("Úsalo solo contra servicios propios o con autorización. Pídele al agente baterías nuevas o edítalas en settings.json.", "Use it only against your own services or with authorization. Ask the agent for new batteries or edit them in settings.json.")).small().color(theme::warn()));
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
                    if ui.small_button(i18n::t("Copiar resumen", "Copy summary")).on_hover_text(i18n::t("Texto compacto para pegar en un chat o un ticket", "Compact text to paste into a chat or a ticket")).clicked() {
                        ui.ctx().copy_text(self.run_summary());
                    }
                    if ui.small_button(i18n::t("Copiar JSON", "Copy JSON")).clicked() {
                        ui.ctx().copy_text(serde_json::to_string_pretty(&self.run.reports).unwrap_or_default());
                    }
                    ui.label(RichText::new(i18n::t("Latencias en ms · éxito = 2xx/3xx · en estrés no se corren los tests", "Latencies in ms · success = 2xx/3xx · tests do not run under stress")).small().color(theme::muted()));
                });
            }
            ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                timeline_charts(ui, &self.run.reports);
                for (i, r) in self.run.reports.iter().enumerate() {
                    ui.add_space(6.0);
                    let ramp = if r.ramp { i18n::t(" (rampa)", " (ramp)") } else { "" };
                    ui.label(RichText::new(tr!("Etapa {} · {} VUs{ramp} · {} s", "Stage {} · {} VUs{ramp} · {} s", i + 1, r.vus, r.secs)).strong());
                    egui::Grid::new(("stage", i)).striped(true).num_columns(11).show(ui, |ui| {
                        for h in ["Endpoint", "n", "err %", "req/s", "p50", "p90", "p95", "p99", "max", i18n::t("media", "mean"), i18n::t("códigos", "codes")] {
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
                            let tip = if s.errors.is_empty() { i18n::t("sin errores de red", "no network errors").to_string() } else { s.errors.join("\n") };
                            ui.label(codes.join(" ")).on_hover_text(tip);
                            ui.end_row();
                        }
                    });
                }
            });
        });
        self.run.open = open;
        if let Some(stress) = start {
            self.start_run(ctx, stress, None);
        }
    }
}
