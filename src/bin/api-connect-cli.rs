//! Consola para CI y para agentes: importa contratos, corre colecciones con sus tests y baterías de estrés.
//! Con `--json -` el resultado sale como JSON por stdout y el progreso por stderr.
//! Códigos de salida: 0 todo bien, 1 fallaron tests/umbrales, 2 error de uso o de configuración.

use std::{
    process::ExitCode,
    sync::atomic::{AtomicBool, Ordering::Relaxed},
};

use api_connect::{
    domain::{
        actions::{self, Effects, Ws},
        model::{self, Collection, Environments, Settings, Stage},
    },
    engine::{runner, script::Scopes},
    i18n::{self, Language, t},
    import::{self, Imported},
    tr,
    infra::{
        cookies, http,
        storage::{self, COLLECTIONS, ENVIRONMENTS, SETTINGS},
    },
};

/// Con `--json -` el texto para personas va a stderr y stdout queda solo para el JSON.
static HUMAN_TO_STDERR: AtomicBool = AtomicBool::new(false);

macro_rules! say {
    ($($t:tt)*) => {
        if HUMAN_TO_STDERR.load(Relaxed) { eprintln!($($t)*) } else { println!($($t)*) }
    };
}

const USAGE_ES: &str = "\
api-connect-cli: importa contratos, corre colecciones y pruebas de estrés (para CI y para agentes)

USO
  api-connect-cli list [--json -]
  api-connect-cli import <ARCHIVO|URL|-> [--keep-scripts] [--json -]
  api-connect-cli apply <ACCIONES.json|-> [--yes] [--json -]
  api-connect-cli send <colección> <request> [--env NOMBRE] [--json -]
  api-connect-cli run <colección> [--env NOMBRE] [-n ITERACIONES] [--data DATOS.csv|.json] [--bail]
                      [--junit ARCHIVO.xml] [--json ARCHIVO.json]
  api-connect-cli stress <colección> [--request NOMBRE] [--setup NOMBRE]... [--battery NOMBRE | --vus N --secs S [--ramp S]]
                         [--env NOMBRE] [--max-p95 MS] [--max-errors PCT] [--json ARCHIVO.json]

IMPORT
  Detecta OpenAPI 3 / Swagger 2 (YAML o JSON), colección de Postman v2 o cURL y la agrega a collections.json.
  Los scripts de colecciones de Postman se quitan salvo --keep-scripts (corren con acceso a tus variables).
APPLY
  Aplica acciones del agente (el mismo JSON de los bloques ```actions de la app: un objeto o una lista) y guarda.
  Borrar, escribir scripts o cambiar la auth exige --yes. send/run_collection/run_stress/import_openapi: usa send, run, stress, import.
SEND
  Envía un request guardado (con pre-request, auth y tests) y muestra status, headers, body y tests.
RUN
  --data      CSV con encabezado (coma o punto y coma) o JSON [{...}]: una fila por iteración, en {{columna}} y pm.iterationData.
              Sin -n, corre una iteración por fila.
STRESS
  --request   estresa solo ese endpoint (por defecto, toda la colección)
  --setup     request a enviar una vez antes de medir (p. ej. el login que guarda el token); se puede repetir
  --ramp S    en una batería personalizada, sube de 1 a --vus usuarios en S segundos antes de sostener --secs

OPCIONES COMUNES
  --dir CARPETA   donde están collections.json / environments.json / settings.json (por defecto, la misma de la app)
  --env NOMBRE    entorno a usar (por defecto, el activo en la app). Los cambios de variables no se guardan.
  --json -        resultado como JSON por stdout (el progreso va a stderr); con un nombre de archivo, lo guarda ahí.
  --lang es|en    idioma de la salida (por defecto: API_CONNECT_LANG, luego el idioma de la app en settings.json; español)

SALIDA
  0 = todo pasó · 1 = fallaron tests o se superaron umbrales · 2 = error de uso o configuración";

const USAGE_EN: &str = "\
api-connect-cli: imports contracts, runs collections and stress tests (for CI and for agents)

USAGE
  api-connect-cli list [--json -]
  api-connect-cli import <FILE|URL|-> [--keep-scripts] [--json -]
  api-connect-cli apply <ACTIONS.json|-> [--yes] [--json -]
  api-connect-cli send <collection> <request> [--env NAME] [--json -]
  api-connect-cli run <collection> [--env NAME] [-n ITERATIONS] [--data DATA.csv|.json] [--bail]
                      [--junit FILE.xml] [--json FILE.json]
  api-connect-cli stress <collection> [--request NAME] [--setup NAME]... [--battery NAME | --vus N --secs S [--ramp S]]
                         [--env NAME] [--max-p95 MS] [--max-errors PCT] [--json FILE.json]

IMPORT
  Detects OpenAPI 3 / Swagger 2 (YAML or JSON), a Postman v2 collection or cURL and adds it to collections.json.
  Scripts from Postman collections are removed unless --keep-scripts (they run with access to your variables).
APPLY
  Applies agent actions (the same JSON as the app's ```actions blocks: one object or a list) and saves.
  Deleting, writing scripts or changing auth requires --yes. send/run_collection/run_stress/import_openapi: use send, run, stress, import.
SEND
  Sends a saved request (with pre-request, auth and tests) and shows status, headers, body and tests.
RUN
  --data      CSV with a header row (comma or semicolon) or JSON [{...}]: one row per iteration, as {{column}} and pm.iterationData.
              Without -n, runs one iteration per row.
STRESS
  --request   stresses only that endpoint (default: the whole collection)
  --setup     request sent once before measuring (e.g. the login that stores the token); can be repeated
  --ramp S    in a custom battery, ramps from 1 to --vus users over S seconds before holding --secs

COMMON OPTIONS
  --dir FOLDER    where collections.json / environments.json / settings.json live (default: the app's data folder)
  --env NAME      environment to use (default: the app's active one). Variable changes are not saved.
  --json -        result as JSON on stdout (progress goes to stderr); with a file name, saves it there.
  --lang es|en    output language (default: API_CONNECT_LANG, then the app's language in settings.json; Spanish)

EXIT CODES
  0 = all passed · 1 = tests failed or thresholds exceeded · 2 = usage or configuration error";

fn main() -> ExitCode {
    match cli(std::env::args().skip(1).collect()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            if HUMAN_TO_STDERR.load(Relaxed) {
                println!("{}", serde_json::json!({ "error": e }));
            } else {
                eprintln!("error: {e}\n{}", t("(ayuda: api-connect-cli --help)", "(help: api-connect-cli --help)"));
            }
            ExitCode::from(2)
        }
    }
}

struct Args {
    pos: Vec<String>,
    opts: Vec<(String, String)>,
    flags: Vec<String>,
}

impl Args {
    fn parse(raw: Vec<String>) -> Result<Self, String> {
        const FLAGS: [&str; 4] = ["--bail", "--help", "--keep-scripts", "--yes"];
        let (mut pos, mut opts, mut flags) = (Vec::new(), Vec::new(), Vec::new());
        let mut it = raw.into_iter();
        while let Some(a) = it.next() {
            if FLAGS.contains(&a.as_str()) || a == "-h" {
                flags.push(a);
            } else if a.starts_with('-') && a != "-" {
                let v = it.next().ok_or(tr!("{a} necesita un valor", "{a} needs a value"))?;
                opts.push((a, v));
            } else {
                pos.push(a);
            }
        }
        Ok(Self { pos, opts, flags })
    }

    fn all(&self, k: &str) -> Vec<&str> {
        self.opts.iter().filter(|(o, _)| o == k).map(|(_, v)| v.as_str()).collect()
    }

    fn get(&self, k: &str) -> Option<&str> {
        self.opts.iter().rev().find(|(o, _)| o == k).map(|(_, v)| v.as_str())
    }

    fn num<T: std::str::FromStr>(&self, k: &str) -> Result<Option<T>, String> {
        self.get(k).map(|v| v.parse().map_err(|_| tr!("{k}: \"{v}\" no es un número válido", "{k}: \"{v}\" is not a valid number"))).transpose()
    }

    fn check(&self, allowed: &[&str]) -> Result<(), String> {
        match self.opts.iter().find(|(o, _)| !allowed.contains(&o.as_str()) && o != "--dir" && o != "--lang") {
            Some((o, _)) => Err(tr!("opción desconocida: {o}", "unknown option: {o}")),
            None => Ok(()),
        }
    }
}

fn cli(raw: Vec<String>) -> Result<bool, String> {
    let a = Args::parse(raw)?;
    HUMAN_TO_STDERR.store(a.get("--json") == Some("-"), Relaxed);
    let lang = a.get("--lang").map(|l| Language::parse(l).ok_or(tr!("--lang {l}: usa es o en", "--lang {l}: use es or en"))).transpose()?;
    let dir = match a.get("--dir") {
        Some(dir) => {
            std::env::set_current_dir(dir).map_err(|e| format!("--dir {dir}: {e}"))?;
            ".".into()
        }
        None => storage::data_dir(),
    };
    // Idioma: --lang, luego API_CONNECT_LANG, luego el de la app (settings.json).
    let settings: Settings = storage::load(dir.join(SETTINGS))?.unwrap_or_default();
    i18n::set(lang.or(i18n::from_env()).unwrap_or(settings.language));
    if a.pos.is_empty() || a.flags.iter().any(|f| f == "--help" || f == "-h") {
        println!("{}", t(USAGE_ES, USAGE_EN));
        return Ok(true);
    }
    let cmd = a.pos[0].as_str();
    if cmd == "import" {
        a.check(&["--json"])?;
        let msg = import_into(&dir, a.pos.get(1).ok_or(t("falta el archivo, la URL o - (stdin) a importar", "missing the file, URL or - (stdin) to import"))?, a.flags.iter().any(|f| f == "--keep-scripts"))?;
        return report(&a, serde_json::json!({ "imported": msg }), &tr!("✔ Importado {msg}", "✔ Imported {msg}")).map(|_| true);
    }
    let cols: Vec<Collection> =
        storage::load(dir.join(COLLECTIONS))?.ok_or(tr!("no existe {} (usa --dir, o importa un contrato con `import`)", "{} does not exist (use --dir, or import a contract with `import`)", dir.join(COLLECTIONS).display()))?;
    let envs: Environments = storage::load(dir.join(ENVIRONMENTS))?.unwrap_or_default();

    if cmd == "apply" {
        a.check(&["--json"])?;
        return apply(&a, &dir, cols, envs, settings);
    }
    if cmd == "list" {
        a.check(&["--json"])?;
        if let Some(path) = a.get("--json") {
            let json = serde_json::json!({
                "collections": cols.iter().map(|c| serde_json::json!({
                    "name": c.name,
                    "requests": c.requests.iter().map(|r| serde_json::json!({ "name": r.name, "method": r.method, "url": r.url })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "environments": envs.envs.keys().collect::<Vec<_>>(),
                "active_environment": envs.active,
                "batteries": settings.batteries,
            });
            return write(path, &serde_json::to_string_pretty(&json).unwrap_or_default()).map(|_| true);
        }
        say!("{}", t("Colecciones:", "Collections:"));
        for c in &cols {
            say!("  {} ({} requests)", c.name, c.requests.len());
        }
        say!("{} {}", t("Entornos:", "Environments:"), envs.envs.keys().cloned().collect::<Vec<_>>().join(", "));
        say!("{}", t("Baterías:", "Batteries:"));
        for b in &settings.batteries {
            say!("  {:<12} {}", b.name, b.description);
        }
        return Ok(true);
    }
    let name = a.pos.get(1).ok_or(t("falta el nombre de la colección", "missing the collection name"))?;
    let col = find(&cols, |c| &c.name, name).ok_or(tr!("no hay una colección \"{name}\" (mira `api-connect-cli list`)", "there is no collection \"{name}\" (see `api-connect-cli list`)"))?;
    let env_name = a.get("--env").unwrap_or(&envs.active);
    let env = match envs.envs.get(env_name) {
        Some(v) => v.clone(),
        None if env_name.is_empty() => Default::default(),
        None => return Err(tr!("no hay un entorno \"{env_name}\"", "there is no environment \"{env_name}\"")),
    };
    let mut scopes = Scopes { env, collection: col.variables.clone(), ..Default::default() };
    let client = http::client();
    let stop = AtomicBool::new(false);
    // Cookie jar en memoria durante la ejecución; no se lee ni se guarda cookies.json.
    cookies::enable(true);

    match cmd {
        "send" => {
            a.check(&["--env", "--json"])?;
            let rname = a.pos.get(2).ok_or(t("falta el nombre del request", "missing the request name"))?;
            let r = find(&col.requests, |q| &q.name, rname).ok_or(tr!("no hay un request \"{rname}\" en {}", "there is no request \"{rname}\" in {}", col.name))?;
            let d = runner::run_send(&client, &model::with_effective_auth(r, Some(col)), scopes);
            let ok = d.result.is_ok() && d.tests.iter().all(|t| t.pass);
            let json = serde_json::json!({
                "request": { "method": d.req.method, "url": d.req.url },
                "response": d.result.as_ref().ok(),
                "error": d.result.as_ref().err(),
                "tests": d.tests,
                "logs": d.logs,
            });
            let human = match &d.result {
                Ok(r) => {
                    let tests: Vec<String> = d.tests.iter().map(|t| format!("{} {}{}", if t.pass { "✔" } else { "✖" }, t.name, t.error.as_ref().map(|e| format!(" — {e}")).unwrap_or_default())).collect();
                    format!("{} {} · {} ms · {} bytes\n{}\n{}", r.status, r.status_text, r.ms, r.size, r.body, tests.join("\n"))
                }
                Err(e) => format!("✖ {e}"),
            };
            report(&a, json, &human)?;
            Ok(ok)
        }
        "run" => {
            a.check(&["--env", "-n", "--data", "--junit", "--json"])?;
            let data = match a.get("--data") {
                Some(path) => {
                    let text = std::fs::read_to_string(path).map_err(|e| format!("--data {path}: {e}"))?;
                    runner::parse_data(&text).map_err(|e| format!("--data {path}: {e}"))?
                }
                None => Vec::new(),
            };
            let n = a.num::<u32>("-n")?.unwrap_or(data.len().max(1) as u32);
            let rows = if data.is_empty() { String::new() } else { tr!(" · {} fila(s) de datos", " · {} data row(s)", data.len()) };
            let env_shown = if env_name.is_empty() { t("(ninguno)", "(none)") } else { env_name };
            say!("{}", tr!("▶ {} · entorno: {} · {n} iteración(es){rows}\n", "▶ {} · environment: {} · {n} iteration(s){rows}\n", col.name, env_shown));
            let bail = a.flags.iter().any(|f| f == "--bail");
            let steps = runner::run_collection(&client, col, &mut scopes, n, &data, bail, &stop, &mut |s| {
                let mark = if s.passed() { "✔" } else { "✖" };
                let status = if s.status == 0 { "---".into() } else { s.status.to_string() };
                say!("{mark} [{}] {:<7} {:<40} {status} {:>6} ms", s.iteration, s.method, s.name, s.ms);
                if let Some(e) = &s.error {
                    say!("      error: {e}");
                }
                for t in &s.tests {
                    say!("      {} {}{}", if t.pass { "✔" } else { "✖" }, t.name, t.error.as_ref().map(|e| format!(" — {e}")).unwrap_or_default());
                }
            });
            let failed = steps.iter().filter(|s| !s.passed()).count();
            let tests = steps.iter().map(|s| s.tests.len()).sum::<usize>();
            let tests_failed = steps.iter().flat_map(|s| &s.tests).filter(|t| !t.pass).count();
            say!("{}", tr!("\nRequests: {} · fallidos: {failed} · tests: {tests} · tests fallidos: {tests_failed}", "\nRequests: {} · failed: {failed} · tests: {tests} · failed tests: {tests_failed}", steps.len()));
            if let Some(path) = a.get("--junit") {
                write(path, &runner::junit(&col.name, &steps))?;
            }
            if let Some(path) = a.get("--json") {
                let json = serde_json::json!({ "collection": col.name, "environment": env_name, "steps": steps, "failed": failed });
                write(path, &serde_json::to_string_pretty(&json).unwrap_or_default())?;
            }
            Ok(failed == 0)
        }
        "stress" => {
            a.check(&["--env", "--request", "--setup", "--battery", "--vus", "--secs", "--ramp", "--max-p95", "--max-errors", "--json"])?;
            let setup = a
                .all("--setup")
                .into_iter()
                .map(|r| find(&col.requests, |q| &q.name, r).map(|q| model::with_effective_auth(q, Some(col))).ok_or(tr!("no hay un request \"{r}\" en {}", "there is no request \"{r}\" in {}", col.name)))
                .collect::<Result<Vec<_>, _>>()?;
            let targets = match a.get("--request") {
                Some(r) => vec![model::with_effective_auth(find(&col.requests, |q| &q.name, r).ok_or(tr!("no hay un request \"{r}\" en {}", "there is no request \"{r}\" in {}", col.name))?, Some(col))],
                // La preparación (login) no se martilla junto con el resto.
                None => col.requests.iter().filter(|q| !setup.iter().any(|s| s.name == q.name)).map(|q| model::with_effective_auth(q, Some(col))).collect(),
            };
            let ramp = a.num::<u64>("--ramp")?;
            let (label, stages) = match (a.get("--battery"), a.num::<u32>("--vus")?, a.num::<u64>("--secs")?) {
                (Some(b), None, None) if ramp.is_none() => {
                    let b = find(&settings.batteries, |x| &x.name, b).ok_or(tr!("no hay una batería \"{b}\" (mira `api-connect-cli list`)", "there is no battery \"{b}\" (see `api-connect-cli list`)"))?;
                    (b.name.clone(), b.stages.clone())
                }
                (None, vus, secs) => {
                    let vus = vus.unwrap_or(10);
                    let mut stages: Vec<Stage> = ramp.map(|r| Stage { vus, secs: r, ramp: true }).into_iter().collect();
                    stages.push(Stage { vus, secs: secs.unwrap_or(30), ramp: false });
                    (t("Personalizada", "Custom").into(), stages)
                }
                _ => return Err(t("usa --battery o bien --vus/--secs/--ramp, no ambos", "use --battery or --vus/--secs/--ramp, not both").into()),
            };
            let plan: Vec<String> = stages.iter().map(|s| s.label()).collect();
            say!("⚡ {} · {label} · {} endpoint(s) · {}", col.name, targets.len(), plan.join(" → "));
            let mut last = std::time::Instant::now();
            let reports = runner::stress(&client, &setup, &targets, scopes, &stages, &stop, &mut |p| {
                if last.elapsed().as_secs() >= 5 {
                    last = std::time::Instant::now();
                    say!("{}", tr!("  etapa {}/{} · {} VUs · {}/{} s · {} requests · {} fallidos", "  stage {}/{} · {} VUs · {}/{} s · {} requests · {} failed", p.stage + 1, p.stages, p.vus, p.elapsed.as_secs(), p.secs, p.count, p.failed));
                }
            })?;
            for (i, r) in reports.iter().enumerate() {
                let ramp = if r.ramp { t(" (rampa)", " (ramp)") } else { "" };
                say!("{}", tr!("\nEtapa {} · {} VUs{} · {} s", "\nStage {} · {} VUs{} · {} s", i + 1, r.vus, ramp, r.secs));
                say!("  {:<38} {:>7} {:>7} {:>8} {:>6} {:>6} {:>6} {:>6} {:>6}  {}", "endpoint", "n", "err %", "req/s", "p50", "p90", "p95", "p99", "max", t("códigos", "codes"));
                for s in r.endpoints.iter().chain([&r.total]) {
                    let codes = s.statuses.iter().map(|(k, v)| format!("{k}×{v}")).collect::<Vec<_>>().join(" ");
                    say!(
                        "  {:<38} {:>7} {:>7.2} {:>8.1} {:>6} {:>6} {:>6} {:>6} {:>6}  {codes}",
                        cut(&s.name, 38),
                        s.count,
                        s.error_pct,
                        s.rps,
                        s.p50,
                        s.p90,
                        s.p95,
                        s.p99,
                        s.max
                    );
                    for e in &s.errors {
                        say!("      error: {e}");
                    }
                }
            }
            let tl: Vec<&runner::Bucket> = reports.iter().flat_map(|r| &r.timeline).collect();
            if tl.len() > 1 {
                let last = if tl.len() > 120 { t(", últimos 120 s", ", last 120 s") } else { "" };
                say!("{}", tr!("\nEn el tiempo (1 carácter = 1 s{}):", "\nOver time (1 character = 1 s{}):", last));
                let tl = &tl[tl.len().saturating_sub(120)..];
                let line = |f: &dyn Fn(&runner::Bucket) -> f64| runner::sparkline(&tl.iter().map(|b| f(b)).collect::<Vec<_>>());
                let max = |f: &dyn Fn(&runner::Bucket) -> f64| tl.iter().map(|b| f(b)).fold(0.0, f64::max);
                for (name, f) in [
                    ("VUs    ", &(|b: &runner::Bucket| b.vus as f64) as &dyn Fn(&runner::Bucket) -> f64),
                    ("req/s  ", &|b: &runner::Bucket| b.count as f64),
                    ("p95 ms ", &|b: &runner::Bucket| b.p95 as f64),
                    (t("errores", "errors "), &|b: &runner::Bucket| b.errors as f64),
                ] {
                    say!("  {name} {}  {} {}", line(f), t("máx", "max"), max(f));
                }
            }
            if let Some(path) = a.get("--json") {
                let json = serde_json::json!({ "collection": col.name, "battery": label, "stages": reports });
                write(path, &serde_json::to_string_pretty(&json).unwrap_or_default())?;
            }
            let violations = runner::check_thresholds(&reports, a.num("--max-p95")?, a.num("--max-errors")?);
            for v in &violations {
                say!("✖ {v}");
            }
            Ok(violations.is_empty())
        }
        _ => Err(tr!("comando desconocido: {cmd}", "unknown command: {cmd}")),
    }
}

/// Nombre exacto primero; si no, sin distinguir mayúsculas.
fn find<'a, T>(items: &'a [T], name: impl Fn(&T) -> &String, wanted: &str) -> Option<&'a T> {
    items.iter().find(|x| name(x) == wanted).or_else(|| items.iter().find(|x| name(x).eq_ignore_ascii_case(wanted)))
}

fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.into() } else { s.chars().take(n - 1).chain(['…']).collect() }
}

/// Con --json, el JSON (a stdout o a un archivo); si no, el texto para personas.
fn report(a: &Args, json: serde_json::Value, human: &str) -> Result<(), String> {
    match a.get("--json") {
        Some(path) => write(path, &serde_json::to_string_pretty(&json).unwrap_or_default()),
        None => {
            say!("{human}");
            Ok(())
        }
    }
}

/// Acciones del agente sobre los JSON de la app, con las mismas reglas que en la app (lo riesgoso pide --yes).
fn apply(a: &Args, dir: &std::path::Path, mut cols: Vec<Collection>, mut envs: Environments, mut settings: Settings) -> Result<bool, String> {
    let text = read_source(a.pos.get(1).ok_or(t("falta el archivo de acciones o - (stdin)", "missing the actions file or - (stdin)"))?)?;
    let list = actions::extract(&format!("```actions\n{text}\n```"));
    let yes = a.flags.iter().any(|f| f == "--yes");
    let (mut ed, mut cur, mut sel) = (model::Request::default(), None, 0);
    let mut fx = Effects::default();
    let mut results = Vec::new();
    for item in list {
        let r = item.and_then(|act| {
            if act.launches() {
                return Err(tr!("{}: en la consola usa los comandos send, run, stress o import", "{}: in the console use the send, run, stress or import commands", act.describe()));
            }
            if act.needs_approval() && !yes {
                return Err(tr!("{}: necesita --yes", "{}: needs --yes", act.describe()));
            }
            let mut ws = Ws { cols: &mut cols, batteries: &mut settings.batteries, envs: &mut envs, ed: &mut ed, cur: &mut cur, col_sel: &mut sel };
            ws.apply(&act, &mut fx)
        });
        say!("{} {}", if r.is_ok() { "✔" } else { "✖" }, r.as_ref().unwrap_or_else(|e| e));
        results.push(match &r {
            Ok(m) => serde_json::json!({ "ok": true, "message": m }),
            Err(e) => serde_json::json!({ "ok": false, "error": e }),
        });
    }
    if fx.cols {
        storage::save(dir.join(COLLECTIONS), &cols)?;
    }
    if fx.envs {
        storage::save(dir.join(ENVIRONMENTS), &envs)?;
    }
    if fx.settings {
        storage::save(dir.join(SETTINGS), &settings)?;
    }
    let ok = results.iter().all(|r| r["ok"] == true);
    if let Some(path) = a.get("--json") {
        write(path, &serde_json::to_string_pretty(&serde_json::json!({ "results": results })).unwrap_or_default())?;
    }
    Ok(ok)
}

/// Archivo, o `-` para leer stdin.
fn read_source(source: &str) -> Result<String, String> {
    if source == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).map_err(|e| format!("stdin: {e}"))?;
        return Ok(s);
    }
    std::fs::read_to_string(source).map_err(|e| format!("{source}: {e}"))
}

/// `-` = stdout.
fn write(path: &str, content: &str) -> Result<(), String> {
    if path == "-" {
        println!("{content}");
        return Ok(());
    }
    std::fs::write(path, content).map_err(|e| format!("{path}: {e}"))?;
    say!("{} {path}", t("Reporte:", "Report:"));
    Ok(())
}

/// Agrega a collections.json lo que haya en el archivo, la URL o stdin. Un cURL va a la colección "Importados".
fn import_into(dir: &std::path::Path, source: &str, keep_scripts: bool) -> Result<String, String> {
    let (text, url) = if source.starts_with("http://") || source.starts_with("https://") {
        let accept = [("Accept".to_string(), "application/json, application/yaml, text/yaml, */*".to_string())];
        let r = http::send_http(&http::client(), "GET", source, &accept, "")?;
        if r.status >= 400 {
            return Err(format!("{source}: {} {}", r.status, r.status_text));
        }
        (r.body, Some(source))
    } else {
        (read_source(source)?, None)
    };
    let (format, imported) = import::detect(&text, url)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(COLLECTIONS);
    let mut cols: Vec<Collection> = storage::load(&path)?.unwrap_or_default();
    let msg = match imported {
        Imported::Collection(mut c) => {
            if format == "Postman" && !keep_scripts {
                for r in &mut c.requests {
                    r.pre.clear();
                    r.tests.clear();
                }
            }
            let msg = tr!("{format}: colección \"{}\" con {} requests", "{format}: collection \"{}\" with {} requests", c.name, c.requests.len());
            cols.push(c);
            msg
        }
        Imported::Request(r) => {
            let folder = t("Importados", "Imported");
            let msg = tr!("{format}: {} {} en la colección \"{folder}\"", "{format}: {} {} in the \"{folder}\" collection", r.method, r.url);
            match cols.iter_mut().find(|c| c.name == folder) {
                Some(c) => c.requests.push(model::Request { name: r.url.clone(), ..r }),
                None => cols.push(Collection { name: folder.into(), requests: vec![model::Request { name: r.url.clone(), ..r }], ..Default::default() }),
            }
            msg
        }
    };
    storage::save(&path, &cols)?;
    Ok(format!("{msg} → {}", std::path::absolute(&path).unwrap_or(path).display()))
}
