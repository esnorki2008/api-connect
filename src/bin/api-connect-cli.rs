//! Consola para CI: corre colecciones (como Newman) y baterías de estrés.
//! Códigos de salida: 0 todo bien, 1 fallaron tests/umbrales, 2 error de uso o de configuración.

use std::{process::ExitCode, sync::atomic::AtomicBool};

use api_connect::{
    model::{self, COLLECTIONS, Collection, ENVIRONMENTS, Environments, SETTINGS, Settings},
    runner::{self, Stage},
    script::Scopes,
};

const USAGE: &str = "\
api-connect-cli: runner de colecciones y pruebas de estrés

USO
  api-connect-cli list
  api-connect-cli run <colección> [--env NOMBRE] [-n ITERACIONES] [--data DATOS.csv|.json] [--bail]
                      [--junit ARCHIVO.xml] [--json ARCHIVO.json]
  api-connect-cli stress <colección> [--request NOMBRE] [--setup NOMBRE]... [--battery NOMBRE | --vus N --secs S [--ramp S]]
                         [--env NOMBRE] [--max-p95 MS] [--max-errors PCT] [--json ARCHIVO.json]

RUN
  --data      CSV con encabezado (coma o punto y coma) o JSON [{...}]: una fila por iteración, en {{columna}} y pm.iterationData.
              Sin -n, corre una iteración por fila.
STRESS
  --request   estresa solo ese endpoint (por defecto, toda la colección)
  --setup     request a enviar una vez antes de medir (p. ej. el login que guarda el token); se puede repetir
  --ramp S    en una batería personalizada, sube de 1 a --vus usuarios en S segundos antes de sostener --secs

OPCIONES COMUNES
  --dir CARPETA   donde están collections.json / environments.json / settings.json (por defecto, la actual)
  --env NOMBRE    entorno a usar (por defecto, el activo en la app). Los cambios de variables no se guardan.

SALIDA
  0 = todo pasó · 1 = fallaron tests o se superaron umbrales · 2 = error de uso o configuración";

fn main() -> ExitCode {
    match cli(std::env::args().skip(1).collect()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e}\n(ayuda: api-connect-cli --help)");
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
        const FLAGS: [&str; 2] = ["--bail", "--help"];
        let (mut pos, mut opts, mut flags) = (Vec::new(), Vec::new(), Vec::new());
        let mut it = raw.into_iter();
        while let Some(a) = it.next() {
            if FLAGS.contains(&a.as_str()) || a == "-h" {
                flags.push(a);
            } else if a.starts_with('-') {
                let v = it.next().ok_or(format!("{a} necesita un valor"))?;
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
        self.get(k).map(|v| v.parse().map_err(|_| format!("{k}: \"{v}\" no es un número válido"))).transpose()
    }

    fn check(&self, allowed: &[&str]) -> Result<(), String> {
        match self.opts.iter().find(|(o, _)| !allowed.contains(&o.as_str()) && o != "--dir") {
            Some((o, _)) => Err(format!("opción desconocida: {o}")),
            None => Ok(()),
        }
    }
}

fn cli(raw: Vec<String>) -> Result<bool, String> {
    let a = Args::parse(raw)?;
    if a.pos.is_empty() || a.flags.iter().any(|f| f == "--help" || f == "-h") {
        println!("{USAGE}");
        return Ok(true);
    }
    if let Some(dir) = a.get("--dir") {
        std::env::set_current_dir(dir).map_err(|e| format!("--dir {dir}: {e}"))?;
    }
    let cols: Vec<Collection> = model::load(COLLECTIONS)?.ok_or(format!("no existe {COLLECTIONS} en esta carpeta (usa --dir)"))?;
    let envs: Environments = model::load(ENVIRONMENTS)?.unwrap_or_default();
    let settings: Settings = model::load(SETTINGS)?.unwrap_or_default();

    let cmd = a.pos[0].as_str();
    if cmd == "list" {
        println!("Colecciones:");
        for c in &cols {
            println!("  {} ({} requests)", c.name, c.requests.len());
        }
        println!("Entornos: {}", envs.envs.keys().cloned().collect::<Vec<_>>().join(", "));
        println!("Baterías:");
        for b in &settings.batteries {
            println!("  {:<12} {}", b.name, b.description);
        }
        return Ok(true);
    }
    let name = a.pos.get(1).ok_or("falta el nombre de la colección")?;
    let col = find(&cols, |c| &c.name, name).ok_or(format!("no hay una colección \"{name}\" (mira `api-connect-cli list`)"))?;
    let env_name = a.get("--env").unwrap_or(&envs.active);
    let env = match envs.envs.get(env_name) {
        Some(v) => v.clone(),
        None if env_name.is_empty() => Default::default(),
        None => return Err(format!("no hay un entorno \"{env_name}\"")),
    };
    let mut scopes = Scopes { env, collection: col.variables.clone(), ..Default::default() };
    let client = model::client();
    let stop = AtomicBool::new(false);
    // Cookie jar en memoria durante la ejecución (como Newman); no se lee ni se guarda cookies.json.
    api_connect::cookies::enable(true);

    match cmd {
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
            let rows = if data.is_empty() { String::new() } else { format!(" · {} fila(s) de datos", data.len()) };
            println!("▶ {} · entorno: {} · {n} iteración(es){rows}\n", col.name, if env_name.is_empty() { "(ninguno)" } else { env_name });
            let bail = a.flags.iter().any(|f| f == "--bail");
            let steps = runner::run_collection(&client, col, &mut scopes, n, &data, bail, &stop, &mut |s| {
                let mark = if s.passed() { "✔" } else { "✖" };
                let status = if s.status == 0 { "---".into() } else { s.status.to_string() };
                println!("{mark} [{}] {:<7} {:<40} {status} {:>6} ms", s.iteration, s.method, s.name, s.ms);
                if let Some(e) = &s.error {
                    println!("      error: {e}");
                }
                for t in &s.tests {
                    println!("      {} {}{}", if t.pass { "✔" } else { "✖" }, t.name, t.error.as_ref().map(|e| format!(" — {e}")).unwrap_or_default());
                }
            });
            let failed = steps.iter().filter(|s| !s.passed()).count();
            let tests = steps.iter().map(|s| s.tests.len()).sum::<usize>();
            let tests_failed = steps.iter().flat_map(|s| &s.tests).filter(|t| !t.pass).count();
            println!("\nRequests: {} · fallidos: {failed} · tests: {tests} · tests fallidos: {tests_failed}", steps.len());
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
                .map(|r| find(&col.requests, |q| &q.name, r).map(|q| model::with_effective_auth(q, Some(col))).ok_or(format!("no hay un request \"{r}\" en {}", col.name)))
                .collect::<Result<Vec<_>, _>>()?;
            let targets = match a.get("--request") {
                Some(r) => vec![model::with_effective_auth(find(&col.requests, |q| &q.name, r).ok_or(format!("no hay un request \"{r}\" en {}", col.name))?, Some(col))],
                // La preparación (login) no se martilla junto con el resto.
                None => col.requests.iter().filter(|q| !setup.iter().any(|s| s.name == q.name)).map(|q| model::with_effective_auth(q, Some(col))).collect(),
            };
            let ramp = a.num::<u64>("--ramp")?;
            let (label, stages) = match (a.get("--battery"), a.num::<u32>("--vus")?, a.num::<u64>("--secs")?) {
                (Some(b), None, None) if ramp.is_none() => {
                    let b = find(&settings.batteries, |x| &x.name, b).ok_or(format!("no hay una batería \"{b}\" (mira `api-connect-cli list`)"))?;
                    (b.name.clone(), b.stages.clone())
                }
                (None, vus, secs) => {
                    let vus = vus.unwrap_or(10);
                    let mut stages: Vec<Stage> = ramp.map(|r| Stage { vus, secs: r, ramp: true }).into_iter().collect();
                    stages.push(Stage { vus, secs: secs.unwrap_or(30), ramp: false });
                    ("Personalizada".into(), stages)
                }
                _ => return Err("usa --battery o bien --vus/--secs/--ramp, no ambos".into()),
            };
            let plan: Vec<String> = stages.iter().map(|s| s.label()).collect();
            println!("⚡ {} · {label} · {} endpoint(s) · {}", col.name, targets.len(), plan.join(" → "));
            let mut last = std::time::Instant::now();
            let reports = runner::stress(&client, &setup, &targets, scopes, &stages, &stop, &mut |p| {
                if last.elapsed().as_secs() >= 5 {
                    last = std::time::Instant::now();
                    println!("  etapa {}/{} · {} VUs · {}/{} s · {} requests · {} fallidos", p.stage + 1, p.stages, p.vus, p.elapsed.as_secs(), p.secs, p.count, p.failed);
                }
            })?;
            for (i, r) in reports.iter().enumerate() {
                println!("\nEtapa {} · {} VUs{} · {} s", i + 1, r.vus, if r.ramp { " (rampa)" } else { "" }, r.secs);
                println!("  {:<38} {:>7} {:>7} {:>8} {:>6} {:>6} {:>6} {:>6} {:>6}  códigos", "endpoint", "n", "err %", "req/s", "p50", "p90", "p95", "p99", "max");
                for s in r.endpoints.iter().chain([&r.total]) {
                    let codes = s.statuses.iter().map(|(k, v)| format!("{k}×{v}")).collect::<Vec<_>>().join(" ");
                    println!(
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
                        println!("      error: {e}");
                    }
                }
            }
            let tl: Vec<&runner::Bucket> = reports.iter().flat_map(|r| &r.timeline).collect();
            if tl.len() > 1 {
                println!("\nEn el tiempo (1 carácter = 1 s{}):", if tl.len() > 120 { ", últimos 120 s" } else { "" });
                let tl = &tl[tl.len().saturating_sub(120)..];
                let line = |f: &dyn Fn(&runner::Bucket) -> f64| runner::sparkline(&tl.iter().map(|b| f(b)).collect::<Vec<_>>());
                let max = |f: &dyn Fn(&runner::Bucket) -> f64| tl.iter().map(|b| f(b)).fold(0.0, f64::max);
                for (name, f) in [
                    ("VUs    ", &(|b: &runner::Bucket| b.vus as f64) as &dyn Fn(&runner::Bucket) -> f64),
                    ("req/s  ", &|b: &runner::Bucket| b.count as f64),
                    ("p95 ms ", &|b: &runner::Bucket| b.p95 as f64),
                    ("errores", &|b: &runner::Bucket| b.errors as f64),
                ] {
                    println!("  {name} {}  máx {}", line(f), max(f));
                }
            }
            if let Some(path) = a.get("--json") {
                let json = serde_json::json!({ "collection": col.name, "battery": label, "stages": reports });
                write(path, &serde_json::to_string_pretty(&json).unwrap_or_default())?;
            }
            let violations = runner::check_thresholds(&reports, a.num("--max-p95")?, a.num("--max-errors")?);
            for v in &violations {
                println!("✖ {v}");
            }
            Ok(violations.is_empty())
        }
        _ => Err(format!("comando desconocido: {cmd}")),
    }
}

/// Nombre exacto primero; si no, sin distinguir mayúsculas.
fn find<'a, T>(items: &'a [T], name: impl Fn(&T) -> &String, wanted: &str) -> Option<&'a T> {
    items.iter().find(|x| name(x) == wanted).or_else(|| items.iter().find(|x| name(x).eq_ignore_ascii_case(wanted)))
}

fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.into() } else { s.chars().take(n - 1).chain(['…']).collect() }
}

fn write(path: &str, content: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|e| format!("{path}: {e}"))?;
    println!("Reporte: {path}");
    Ok(())
}
