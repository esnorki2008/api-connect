//! Repos git (sincronización de contratos) e importación de OpenAPI / Postman / cURL.

use super::*;

impl App {
    pub(super) fn start_repo(&mut self, ctx: &egui::Context, url: String, branch: String, sync_col: Option<usize>) {
        if self.repo_pending.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(repo::fetch_and_scan(Path::new(REPO_CACHE), &url, &branch));
            ctx.request_repaint();
        });
        self.repo_msg = i18n::t("Conectando con el repo… (si git pide credenciales, se abre la ventana de Git Credential Manager)", "Connecting to the repo… (if git asks for credentials, the Git Credential Manager window opens)").into();
        self.repo_pending = Some((rx, sync_col));
    }

    pub(super) fn finish_repo(&mut self, res: Result<repo::Scan, String>, sync_col: Option<usize>) {
        let scan = match res {
            Ok(s) => s,
            Err(e) => {
                if sync_col.is_some() {
                    self.error = e.clone();
                }
                self.repo_msg = e;
                return;
            }
        };
        let cfg = RepoCfg { url: scan.url.clone(), branch: scan.branch.clone() };
        self.settings.repos.retain(|r| *r != cfg);
        self.settings.repos.insert(0, cfg);
        self.settings.repos.truncate(10);
        self.persist(SETTINGS);
        self.repo_msg = tr!("Commit {} · {} archivo(s) en /{}", "Commit {} · {} file(s) in /{}", scan.commit, scan.files.len(), repo::FOLDER);

        if let Some(ci) = sync_col {
            let file = self.cols.get(ci).and_then(|c| c.source.as_ref()).map(|s| s.file.clone()).unwrap_or_default();
            match scan.files.iter().find(|(p, _)| *p == file) {
                Some((_, Ok(remote))) => self.notice = self.sync_into(ci, &scan, &file, remote.clone()),
                Some((_, Err(e))) => self.error = format!("{file}: {e}"),
                None => self.error = tr!("{file} ya no está en /{} del repo", "{file} is no longer in /{} of the repo", repo::FOLDER),
            }
        }
        self.repo_scan = Some(scan);
    }

    /// Los servidores declarados en un contrato que tú importaste son de confianza para el agente.
    pub(super) fn trust_collection_hosts(&mut self, ci: usize) {
        let hosts: Vec<String> = self.cols[ci].variables.values().filter_map(|v| actions::host_of(v)).collect();
        self.trusted_hosts.extend(hosts);
    }

    pub(super) fn sync_into(&mut self, ci: usize, scan: &repo::Scan, file: &str, remote: Collection) -> String {
        // Si el request abierto es de esta colección y no tiene cambios sin guardar, se recarga tras sincronizar.
        let clean = matches!(self.cur, Some((c, i)) if c == ci && self.cols[ci].requests.get(i) == Some(&self.ed));
        let report = repo::merge(&mut self.cols[ci], remote);
        self.cols[ci].source = Some(source_of(scan, file));
        self.trust_collection_hosts(ci);
        if let (true, Some((_, i))) = (clean, self.cur) {
            self.ed = self.cols[ci].requests[i].clone();
        }
        self.persist(COLLECTIONS);
        self.sync_buffers();
        tr!("{} sincronizada ({}): {}", "{} synced ({}): {}", self.cols[ci].name, scan.commit, report.summary())
    }

    /// Importa desde una URL (se descarga en segundo plano) o desde el texto del contrato / cURL / colección.
    pub(super) fn start_import(&mut self, ctx: &egui::Context, source: String, for_agent: bool) {
        let url = source.trim().to_string();
        if !url.starts_with("http://") && !url.starts_with("https://") {
            self.finish_import(ctx, Ok((source, None)), for_agent);
            return;
        }
        let (tx, rx) = channel();
        let (client, ctx2) = (self.client.clone(), ctx.clone());
        std::thread::spawn(move || {
            let accept = [("Accept".to_string(), "application/json, application/yaml, text/yaml, */*".to_string())];
            let res = http::send_http(&client, "GET", &url, &accept, "").and_then(|r| {
                if r.status < 400 { Ok((r.body, Some(url.clone()))) } else { Err(format!("{url}: {} {}", r.status, r.status_text)) }
            });
            let _ = tx.send(res);
            ctx2.request_repaint();
        });
        self.import_pending = Some((rx, for_agent));
    }

    pub(super) fn finish_import(&mut self, ctx: &egui::Context, res: Result<(String, Option<String>), String>, for_agent: bool) {
        let res = res.and_then(|(text, url)| import::detect(&text, url.as_deref())).map(|(format, imported)| match imported {
            Imported::Request(r) => {
                let msg = tr!("{format}: {} {} abierto en el editor (sin guardar)", "{format}: {} {} opened in the editor (unsaved)", r.method, r.url);
                self.new_tab();
                (self.ed, self.cur) = (r, None);
                msg
            }
            Imported::Collection(mut c) => {
                // Los scripts de una colección ajena corren con acceso a tus entornos: solo se mantienen si lo pides.
                if format == "Postman" && (for_agent || !self.import_keep_scripts) {
                    for r in &mut c.requests {
                        r.pre.clear();
                        r.tests.clear();
                    }
                }
                let msg = tr!("{format}: colección \"{}\" con {} requests", "{format}: collection \"{}\" with {} requests", c.name, c.requests.len());
                self.cols.push(c);
                self.col_sel = self.cols.len() - 1;
                // Solo una importación tuya confía en los hosts del contrato; si la pidió el agente, enviar ahí se aprueba.
                if format == "OpenAPI" && !for_agent {
                    self.trust_collection_hosts(self.col_sel);
                }
                self.persist(COLLECTIONS);
                msg
            }
        });
        self.sync_buffers();
        if for_agent {
            self.agent_waits_task = false;
            let text = match res {
                Ok(m) => tr!("✔ importado {m}", "✔ imported {m}"),
                Err(e) => tr!("✖ no se pudo importar: {e}", "✖ could not import: {e}"),
            };
            self.tool_result(ctx, text);
            return;
        }
        match res {
            Ok(m) => {
                self.notice = tr!("Importado {m}.", "Imported {m}.");
                (self.import_text, self.import_url, self.import_err) = (String::new(), String::new(), String::new());
                self.show_import = false;
            }
            Err(e) => self.import_err = e,
        }
    }
}
