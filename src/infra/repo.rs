//! Repos de Azure DevOps (o cualquier git): clon disperso de /collections y sincronización con la copia local.
//! La autenticación la pone git (Git Credential Manager): la app nunca ve ni guarda tokens.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    domain::model::{Collection, Origin},
    i18n::t,
    import::{openapi, postman},
    tr,
};

pub const FOLDER: &str = "collections";

pub struct Scan {
    pub url: String,
    pub branch: String,
    pub commit: String,
    /// (ruta relativa al repo, colección o error de lectura)
    pub files: Vec<(String, Result<Collection, String>)>,
}

/// Clona (la primera vez) o actualiza el repo dentro de `cache` y lee todo lo que haya en /collections.
pub fn fetch_and_scan(cache: &Path, url: &str, branch: &str) -> Result<Scan, String> {
    let (url, branch) = (url.trim(), branch.trim());
    // Un valor que empiece con "-" se colaría como opción de git.
    if url.is_empty() || url.starts_with('-') || branch.starts_with('-') {
        return Err(t("URL o rama inválida", "invalid URL or branch").into());
    }
    let dir = cache_dir(cache, url);
    if dir.join(".git").exists() {
        git(&dir, &["fetch", "--depth", "1", "origin", if branch.is_empty() { "HEAD" } else { branch }])?;
        git(&dir, &["reset", "--hard", "FETCH_HEAD"])?;
    } else {
        std::fs::create_dir_all(cache).map_err(|e| e.to_string())?;
        let target = dir.to_string_lossy().to_string();
        let mut args = vec!["clone", "--depth", "1", "--filter=blob:none", "--sparse"];
        if !branch.is_empty() {
            args.extend(["--branch", branch]);
        }
        args.extend(["--", url, &target]);
        git(cache, &args)?;
        git(&dir, &["sparse-checkout", "set", FOLDER])?;
    }
    let commit = git(&dir, &["rev-parse", "--short", "HEAD"])?.trim().to_string();

    let root = dir.join(FOLDER);
    if !root.is_dir() {
        return Err(tr!("El repo no tiene la carpeta /{FOLDER} en la raíz (commit {commit})", "The repo has no /{FOLDER} folder at its root (commit {commit})"));
    }
    let mut paths = Vec::new();
    walk(&root, &mut paths);
    paths.sort();
    let files = paths
        .into_iter()
        .map(|p| {
            let rel = p.strip_prefix(&dir).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            (rel, load_file(&p))
        })
        .collect();
    Ok(Scan { url: url.into(), branch: branch.into(), commit, files })
}

fn cache_dir(cache: &Path, url: &str) -> PathBuf {
    let clean: String = url.split("://").last().unwrap_or(url).chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let tail = clean.len().saturating_sub(80);
    // Absoluta: git clone corre con current_dir = cache y la ruta destino no debe quedar relativa a ella.
    std::path::absolute(cache.join(&clean[tail..])).unwrap_or_else(|_| cache.join(&clean[tail..]))
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir).env("GIT_TERMINAL_PROMPT", "0"); // sin prompts de consola: GCM usa su propia ventana
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().map_err(|e| tr!("no se pudo ejecutar git (¿está instalado?): {e}", "could not run git (is it installed?): {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("git {}: {}", args[0], String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(&p, out);
        } else if matches!(p.extension().and_then(|e| e.to_str()), Some("yaml" | "yml" | "json")) {
            out.push(p);
        }
    }
}

/// OpenAPI (YAML/JSON) o colección de Postman -> colección con `origin` en cada request.
pub fn load_file(path: &Path) -> Result<Collection, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(t("colección", "collection"));
    let spec = openapi::parse(&text)?;
    if openapi::is_openapi(&spec) {
        return openapi::to_collection(&spec, stem);
    }
    if spec["info"].is_object() && spec["item"].is_array() {
        let mut c = postman::import_postman(&serde_json::to_string(&spec).unwrap_or_default())?;
        // Postman no tiene una clave estable por endpoint: se usa el nombre (con carpeta).
        for r in &mut c.requests {
            r.origin = Some(Origin { key: r.name.clone(), base: r.parts(), schema: serde_json::Value::Null });
        }
        return Ok(c);
    }
    Err(t("no es OpenAPI/Swagger ni colección de Postman", "not an OpenAPI/Swagger spec or a Postman collection").into())
}

#[derive(Default, Debug, PartialEq)]
pub struct Report {
    pub added: Vec<String>,
    /// Cambiaron en el contrato y no tenían cambios tuyos.
    pub updated: Vec<String>,
    /// Cambiaron en el contrato y tenían cambios tuyos: se fusionó campo por campo, conservando los tuyos.
    pub merged: Vec<String>,
    /// Ya no están en el repo: se conservan, nunca se borra trabajo local.
    pub gone: Vec<String>,
}

impl Report {
    pub fn summary(&self) -> String {
        let list = |v: &Vec<String>| if v.is_empty() { String::new() } else { format!(": {}", v.join(", ")) };
        tr!(
            "{} nuevos{} · {} actualizados{} · {} fusionados con tus cambios{} · {} ya no están en el repo{}",
            "{} new{} · {} updated{} · {} merged with your changes{} · {} no longer in the repo{}",
            self.added.len(),
            list(&self.added),
            self.updated.len(),
            list(&self.updated),
            self.merged.len(),
            list(&self.merged),
            self.gone.len(),
            list(&self.gone),
        )
    }
}

/// Lleva a `local` (tu versión) los cambios del contrato `remote` sin perder tu trabajo:
/// - endpoint nuevo en el contrato -> se agrega
/// - endpoint que cambió en el contrato -> merge a 3 vías por campo (ver overlay::rebase): tus campos
///   cambiados se conservan, el resto toma el contrato nuevo; nombre y scripts siempre son tuyos
/// - requests agregados a mano o que ya no están en el repo -> se conservan
/// - variables nuevas del repo -> se agregan; las existentes no se pisan
pub fn merge(local: &mut Collection, remote: Collection) -> Report {
    let mut report = Report::default();
    let remote_keys: HashSet<String> = remote.requests.iter().filter_map(|r| r.origin.as_ref().map(|o| o.key.clone())).collect();
    for r in remote.requests {
        let Some(origin) = r.origin.clone() else { continue };
        match local.requests.iter_mut().find(|l| l.origin.as_ref().is_some_and(|o| o.key == origin.key)) {
            None => {
                report.added.push(r.name.clone());
                local.requests.push(r);
            }
            Some(l) => {
                let old = l.origin.as_ref().unwrap();
                if old.base == origin.base && old.schema == origin.schema {
                    continue; // el contrato no cambió este endpoint
                }
                let (list, parts) = if l.parts() == old.base {
                    (&mut report.updated, origin.base.clone())
                } else {
                    (&mut report.merged, crate::domain::overlay::rebase(&old.base, &l.parts(), &origin.base))
                };
                list.push(l.name.clone());
                l.set_parts(parts);
                l.origin = Some(origin);
            }
        }
    }
    report.gone = local
        .requests
        .iter()
        .filter(|l| l.origin.as_ref().is_some_and(|o| !remote_keys.contains(&o.key)))
        .map(|l| l.name.clone())
        .collect();
    for (k, v) in remote.variables {
        local.variables.entry(k).or_insert(v);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::Request;

    fn spec(get_url_suffix: &str, extra_path: &str) -> String {
        format!(
            "openapi: 3.0.0\ninfo: {{ title: Svc }}\nservers: [{{ url: 'http://svc' }}]\npaths:\n  /a:\n    get: {{ summary: A{get_url_suffix} }}\n  /b:\n    post:\n      summary: B\n      requestBody: {{ content: {{ application/json: {{ example: {{ v: 1 }} }} }} }}\n{extra_path}"
        )
    }

    fn remote(text: &str) -> Collection {
        openapi::to_collection(&openapi::parse(text).unwrap(), "x").unwrap()
    }

    #[test]
    fn merge_keeps_local_work() {
        let mut local = remote(&spec("", ""));
        local.variables.insert("baseUrl".into(), "http://localhost:8080".into());
        // cambios locales: B editado, tests agregados a A, un request propio
        local.requests[1].body = "{\"v\": 99}".into();
        local.requests[0].tests = "pm.test('ok', () => {})".into();
        local.requests.push(Request { name: "Mío".into(), method: "GET".into(), url: "/x".into(), ..Default::default() });

        // el repo cambia A y B, agrega C y una variable
        let mut new = remote(&spec("", "  /c:\n    delete: { summary: C }\n").replace("v: 1", "v: 2").replace("/a:\n    get: { summary: A }", "/a:\n    get: { summary: A, parameters: [ { name: q, in: query, required: true } ] }"));
        new.variables.insert("extra".into(), "1".into());
        let report = merge(&mut local, new);

        assert_eq!(report.added, ["C"]);
        assert_eq!(report.updated, ["A"]);
        assert_eq!(report.merged, ["B"]);
        assert!(report.gone.is_empty());
        let a = &local.requests[0];
        assert_eq!((a.url.as_str(), a.tests.as_str()), ("{{baseUrl}}/a?q={{q}}", "pm.test('ok', () => {})"));
        assert!(!a.modified());
        assert_eq!(local.requests[1].body, "{\n  \"v\": 99\n}", "tu valor gana; el merge por campo formatea el JSON");
        assert!(local.requests[1].modified());
        assert!(local.requests.iter().any(|r| r.name == "Mío"));
        assert_eq!(local.variables.get("baseUrl").unwrap(), "http://localhost:8080");
        assert_eq!(local.variables.get("extra").unwrap(), "1");

        // si C desaparece del repo se conserva y se reporta
        let report = merge(&mut local, remote(&spec("", "")));
        assert_eq!(report.gone, ["C"]);
        assert_eq!(local.requests.len(), 4);
    }

    #[test]
    fn clones_repo_and_scans_collections() {
        let tmp = std::env::temp_dir().join(format!("api-connect-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let origin = tmp.join("origin");
        std::fs::create_dir_all(origin.join("collections/v2")).unwrap();
        std::fs::write(origin.join("collections/openapi.yaml"), spec("", "")).unwrap();
        std::fs::write(origin.join("collections/v2/postman.json"), r#"{"info":{"name":"P"},"item":[{"name":"x","request":"http://p"}]}"#).unwrap();
        std::fs::write(origin.join("collections/notas.json"), "[1, 2]").unwrap();
        std::fs::create_dir_all(origin.join("src")).unwrap();
        std::fs::write(origin.join("src/app.rs"), "no se descarga").unwrap();
        let run = |args: &[&str]| assert!(Command::new("git").args(args).current_dir(&origin).output().unwrap().status.success(), "{args:?}");
        run(&["init", "-q", "-b", "main"]);
        run(&["add", "."]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "x"]);

        let cache = tmp.join("repos");
        let url = format!("file:///{}", origin.to_string_lossy().replace('\\', "/"));
        let scan = fetch_and_scan(&cache, &url, "main").unwrap();
        assert_eq!(scan.files.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["collections/notas.json", "collections/openapi.yaml", "collections/v2/postman.json"]);
        assert!(scan.files[0].1.is_err());
        assert_eq!(scan.files[1].1.as_ref().unwrap().requests.len(), 2);
        assert_eq!(scan.files[2].1.as_ref().unwrap().requests[0].origin.as_ref().unwrap().key, "x");

        // segundo commit: se actualiza con fetch en la caché existente
        std::fs::write(origin.join("collections/openapi.yaml"), spec("", "  /c:\n    get: { summary: C }\n")).unwrap();
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qam", "y"]);
        let scan2 = fetch_and_scan(&cache, &url, "main").unwrap();
        assert_ne!(scan.commit, scan2.commit);
        assert_eq!(scan2.files[1].1.as_ref().unwrap().requests.len(), 3);
        assert!(!cache_dir(&cache, &url).join("src/app.rs").exists(), "el clon disperso no baja otras carpetas");

        assert_eq!(fetch_and_scan(&cache, "--upload-pack=calc", "").err().unwrap(), "URL o rama inválida");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
