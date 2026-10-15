//! Runner de colecciones (como Newman) y baterías de estrés con estadísticas por endpoint.

use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
    time::{Duration, Instant},
};

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::{
    model::{self, Collection, Request, Response, Vars, parse_headers, resolve},
    script::{self, Outcome, ReqParts, Scopes, TestResult},
};

pub struct SendDone {
    pub scopes: Scopes,
    /// El request tal como quedó tras el pre-request (sin resolver variables).
    pub req: ReqParts,
    pub result: Result<Response, String>,
    pub tests: Vec<TestResult>,
    pub logs: Vec<String>,
    /// Último pm.setNextRequest (pre-request o tests).
    pub next: Option<Option<String>>,
}

fn collect(o: Outcome, stage: &str, d: &mut SendDone) {
    d.tests.extend(o.tests);
    d.logs.extend(o.logs);
    if o.next.is_some() {
        d.next = o.next;
    }
    if let Some(e) = o.error {
        d.tests.push(TestResult { name: stage.into(), pass: false, error: Some(e) });
    }
}

/// pre-request -> resolver variables -> HTTP -> tests.
pub fn run_send(client: &Client, ed: &Request, mut scopes: Scopes) -> SendDone {
    let mut req = ed.parts();
    let pre = script::run(&ed.pre, &mut scopes, &mut req, None);
    let vars = scopes.merged();
    // La auth se aplica después del pre-request (que puede haber fijado el token) y entra en `req`,
    // así el estrés reutiliza el request ya autorizado.
    let result = crate::auth::apply(client, &ed.auth, &mut req, &vars).and_then(|_| send_parts(client, &req, &vars));
    let mut d = SendDone { scopes, req, result, tests: Vec::new(), logs: Vec::new(), next: None };
    collect(pre, "Pre-request", &mut d);
    if let Ok(r) = &d.result {
        let o = script::run(&ed.tests, &mut d.scopes, &mut d.req, Some(r));
        collect(o, "Tests", &mut d);
    }
    d
}

fn send_parts(client: &Client, req: &ReqParts, vars: &Vars) -> Result<Response, String> {
    let r = |s: &str| resolve(s, vars);
    model::send_http(client, &req.method, &r(&req.url), &parse_headers(&r(&req.headers)), &r(&req.body))
}

// ---- Archivos de datos ----

/// CSV (con encabezado; separador "," o ";", comillas dobles) o JSON (arreglo de objetos) -> una fila por iteración.
pub fn parse_data(text: &str) -> Result<Vec<Vars>, String> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim_start().starts_with('[') {
        let rows: Vec<serde_json::Map<String, serde_json::Value>> = serde_json::from_str(text).map_err(|e| format!("JSON de datos inválido: {e}"))?;
        return Ok(rows
            .into_iter()
            .map(|r| r.into_iter().map(|(k, v)| (k, v.as_str().map_or_else(|| v.to_string(), String::from))).collect())
            .collect());
    }
    let first = text.lines().next().unwrap_or("");
    let sep = if first.contains(';') && !first.contains(',') { ';' } else { ',' };
    let mut rows = csv_rows(text, sep).into_iter();
    let header = rows.next().ok_or("el archivo de datos está vacío")?;
    let header: Vec<String> = header.into_iter().map(|h| h.trim().to_string()).collect();
    Ok(rows
        .filter(|r| r.iter().any(|c| !c.is_empty()))
        .map(|r| header.iter().cloned().zip(r.into_iter().chain(std::iter::repeat(String::new()))).collect())
        .collect())
}

fn csv_rows(text: &str, sep: char) -> Vec<Vec<String>> {
    let (mut rows, mut row, mut cell, mut quoted) = (Vec::new(), Vec::new(), String::new(), false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == sep && !quoted => row.push(std::mem::take(&mut cell)),
            '\r' if !quoted => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            c => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

// ---- Runner ----

#[derive(Clone, Debug, Serialize)]
pub struct Step {
    pub iteration: u32,
    pub name: String,
    pub method: String,
    /// 0 si no hubo respuesta.
    pub status: u16,
    pub ms: u64,
    pub tests: Vec<TestResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Step {
    pub fn passed(&self) -> bool {
        self.error.is_none() && self.tests.iter().all(|t| t.pass)
    }
}

// ponytail: tope fijo de pasos por iteración para cortar bucles de setNextRequest; súbelo si un polling legítimo lo alcanza.
const MAX_STEPS_PER_ITERATION: usize = 10_000;

/// Corre los requests en orden, `iterations` veces. Con `data`, cada iteración usa una fila (pm.iterationData
/// y {{columna}}), repitiendo desde el inicio si hay más iteraciones que filas. Las variables que fijen los
/// scripts pasan al siguiente request (como en Newman); las locales se limpian en cada uno.
/// pm.setNextRequest(nombre) salta a ese request y pm.setNextRequest(null) termina la iteración.
#[allow(clippy::too_many_arguments)]
pub fn run_collection(
    client: &Client,
    col: &Collection,
    scopes: &mut Scopes,
    iterations: u32,
    data: &[Vars],
    bail: bool,
    stop: &AtomicBool,
    on_step: &mut dyn FnMut(&Step),
) -> Vec<Step> {
    let mut steps = Vec::new();
    for iteration in 1..=iterations.max(1) {
        scopes.data = if data.is_empty() { Vars::new() } else { data[(iteration as usize - 1) % data.len()].clone() };
        let (mut i, mut count) = (0, 0);
        while let Some(r) = col.requests.get(i) {
            if stop.load(Relaxed) {
                return steps;
            }
            scopes.local.clear();
            let d = run_send(client, &model::with_effective_auth(r, Some(col)), std::mem::take(scopes));
            *scopes = d.scopes;
            let (status, ms) = d.result.as_ref().map_or((0, 0), |r| (r.status, r.ms));
            let mut step = Step { iteration, name: r.name.clone(), method: r.method.clone(), status, ms, tests: d.tests, error: d.result.err() };
            count += 1;
            i = match d.next {
                None => i + 1,
                Some(None) => usize::MAX,
                Some(Some(name)) => col.requests.iter().position(|q| q.name == name).unwrap_or_else(|| {
                    step.tests.push(TestResult { name: "setNextRequest".into(), pass: false, error: Some(format!("no existe el request \"{name}\"")) });
                    usize::MAX
                }),
            };
            if count >= MAX_STEPS_PER_ITERATION && i < col.requests.len() {
                step.tests.push(TestResult {
                    name: "setNextRequest".into(),
                    pass: false,
                    error: Some(format!("más de {MAX_STEPS_PER_ITERATION} pasos en la iteración: ¿bucle infinito?")),
                });
                i = usize::MAX;
            }
            on_step(&step);
            let failed = !step.passed();
            steps.push(step);
            if bail && failed {
                return steps;
            }
        }
    }
    steps
}

/// Reporte JUnit: Azure DevOps lo muestra con la tarea "Publish Test Results".
pub fn junit(suite: &str, steps: &[Step]) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let failures = steps.iter().filter(|s| !s.passed()).count();
    let mut x = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"{}\" tests=\"{}\" failures=\"{failures}\">\n", esc(suite), steps.len());
    for s in steps {
        let fails = s.tests.iter().filter(|t| !t.pass).count() + usize::from(s.error.is_some());
        let cases = s.tests.len() + usize::from(s.error.is_some() || s.tests.is_empty());
        x += &format!(
            "  <testsuite name=\"{} {} #{}\" tests=\"{cases}\" failures=\"{fails}\" time=\"{:.3}\">\n",
            esc(&s.method),
            esc(&s.name),
            s.iteration,
            s.ms as f64 / 1000.0
        );
        if let Some(e) = &s.error {
            x += &format!("    <testcase name=\"request\"><failure message=\"{}\"/></testcase>\n", esc(e));
        } else if s.tests.is_empty() {
            x += &format!("    <testcase name=\"HTTP {}\"/>\n", s.status);
        }
        for t in &s.tests {
            match &t.error {
                Some(e) if !t.pass => x += &format!("    <testcase name=\"{}\"><failure message=\"{}\"/></testcase>\n", esc(&t.name), esc(e)),
                _ if !t.pass => x += &format!("    <testcase name=\"{}\"><failure/></testcase>\n", esc(&t.name)),
                _ => x += &format!("    <testcase name=\"{}\"/>\n", esc(&t.name)),
            }
        }
        x += "  </testsuite>\n";
    }
    x + "</testsuites>\n"
}

// ---- Estrés ----

/// Una etapa: `vus` usuarios virtuales en paralelo durante `secs` segundos, sin pausa entre requests.
/// Con `ramp`, los usuarios suben (o bajan) en línea recta desde los de la etapa anterior (1 si es la primera).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    pub vus: u32,
    pub secs: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ramp: bool,
}

impl Stage {
    /// Usuarios activos a los `t` segundos de la etapa.
    pub fn vus_at(&self, from: u32, t: f64) -> u32 {
        if !self.ramp {
            return self.vus;
        }
        let k = (t / self.secs.max(1) as f64).clamp(0.0, 1.0);
        ((from as f64 + (self.vus as f64 - from as f64) * k).round() as u32).max(1)
    }

    pub fn label(&self) -> String {
        if self.ramp { format!("rampa→{} VUs·{} s", self.vus, self.secs) } else { format!("{} VUs·{} s", self.vus, self.secs) }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Battery {
    pub name: String,
    pub description: String,
    pub stages: Vec<Stage>,
}

pub const MAX_VUS: u32 = 500;

pub fn default_batteries() -> Vec<Battery> {
    // (vus, segundos, rampa)
    let b = |name: &str, description: &str, stages: &[(u32, u64, bool)]| Battery {
        name: name.into(),
        description: description.into(),
        stages: stages.iter().map(|&(vus, secs, ramp)| Stage { vus, secs, ramp }).collect(),
    };
    vec![
        b("Humo", "1 usuario, 10 s: ¿responde y en cuánto?", &[(1, 10, false)]),
        b("Carga", "Rampa a 10 usuarios y 60 s sostenidos: tráfico normal", &[(10, 15, true), (10, 60, false)]),
        b(
            "Estrés",
            "Escalera con rampas 10→50→100 usuarios: ¿dónde se degrada?",
            &[(10, 10, true), (10, 20, false), (50, 20, true), (50, 20, false), (100, 20, true), (100, 20, false)],
        ),
        b("Pico", "1→100→1 usuarios de golpe: ráfaga repentina y recuperación", &[(1, 10, false), (100, 15, false), (1, 10, false)]),
        b("Rampa", "Sube de 1 a 100 usuarios en 2 min: encuentra el punto de quiebre", &[(100, 120, true)]),
        b("Resistencia", "Rampa a 20 usuarios y 10 min sostenidos: fugas de memoria / conexiones", &[(20, 30, true), (20, 600, false)]),
    ]
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Stats {
    pub name: String,
    pub count: usize,
    /// Respuestas 2xx/3xx.
    pub ok: usize,
    pub error_pct: f64,
    pub rps: f64,
    pub min: u64,
    pub mean: u64,
    pub p50: u64,
    pub p90: u64,
    pub p95: u64,
    pub p99: u64,
    pub max: u64,
    pub bytes: usize,
    /// "200" -> 950, "503" -> 12, "error" -> 3
    pub statuses: BTreeMap<String, usize>,
    /// Primeros errores de red distintos.
    pub errors: Vec<String>,
}

/// Un segundo de la etapa, para los gráficos de latencia en el tiempo.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Bucket {
    pub sec: u64,
    pub vus: u32,
    pub count: usize,
    pub errors: usize,
    pub p50: u64,
    pub p95: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StageReport {
    pub vus: u32,
    pub secs: u64,
    pub ramp: bool,
    pub elapsed_ms: u64,
    pub total: Stats,
    pub endpoints: Vec<Stats>,
    pub timeline: Vec<Bucket>,
}

pub struct Progress {
    pub stage: usize,
    pub stages: usize,
    pub vus: u32,
    pub elapsed: Duration,
    pub secs: u64,
    pub count: usize,
    pub failed: usize,
}

struct Sample {
    req: usize,
    /// Fin del request, en ms desde el inicio de la etapa.
    at: u64,
    ms: u64,
    status: Option<u16>,
    bytes: usize,
    err: Option<String>,
}

impl Sample {
    fn ok(&self) -> bool {
        self.status.is_some_and(|c| (200..400).contains(&c))
    }
}

/// 1) `setup` (p. ej. login) se envía una vez con sus scripts; si falla, no se mide nada.
/// 2) Calentamiento: cada endpoint una vez en orden con pre-request y tests (no cuenta en las estadísticas).
/// 3) Martilla los requests ya preparados etapa por etapa.
/// ponytail: no corre los tests por request (Boa por request mediría al script, no a la API); el éxito es 2xx/3xx.
pub fn stress(
    client: &Client,
    setup: &[Request],
    targets: &[Request],
    mut scopes: Scopes,
    stages: &[Stage],
    stop: &AtomicBool,
    on_progress: &mut dyn FnMut(&Progress),
) -> Result<Vec<StageReport>, String> {
    if targets.is_empty() || stages.is_empty() {
        return Err("No hay endpoints o etapas que ejecutar".into());
    }
    for s in setup {
        scopes.local.clear();
        let d = run_send(client, s, scopes);
        let problem = match &d.result {
            Err(e) => Some(e.clone()),
            Ok(r) if r.status >= 400 => Some(format!("HTTP {}", r.status)),
            Ok(_) => d.tests.iter().find(|t| !t.pass).map(|t| format!("{}: {}", t.name, t.error.clone().unwrap_or_default())),
        };
        if let Some(p) = problem {
            return Err(format!("La preparación \"{}\" falló ({p}); no se ejecutó la batería", s.name));
        }
        scopes = d.scopes;
    }
    let mut warm = Vec::new();
    for t in targets {
        if stop.load(Relaxed) {
            return Ok(Vec::new());
        }
        scopes.local.clear();
        let d = run_send(client, t, scopes);
        if let Some(e) = d.tests.iter().find(|x| x.name == "Pre-request").and_then(|x| x.error.as_ref()) {
            return Err(format!("Pre-request de \"{}\": {e}", t.name));
        }
        scopes = d.scopes;
        warm.push((d.req, scopes.local.clone()));
    }
    scopes.local.clear();
    let prepared: Vec<(ReqParts, Vars)> = warm
        .into_iter()
        .map(|(parts, local)| {
            let mut v = scopes.merged();
            v.extend(local);
            (parts, v)
        })
        .collect();

    let mut reports = Vec::new();
    let mut prev_vus = 1;
    for (si, st) in stages.iter().enumerate() {
        if stop.load(Relaxed) {
            break;
        }
        let st = Stage { vus: st.vus.clamp(1, MAX_VUS), ..st.clone() };
        let from = prev_vus;
        let workers_n = if st.ramp { st.vus.max(from) } else { st.vus };
        let (count, failed) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let start = Instant::now();
        let deadline = start + Duration::from_secs(st.secs.max(1));
        let samples: Vec<Sample> = std::thread::scope(|sc| {
            let workers: Vec<_> = (0..workers_n)
                .map(|vu| {
                    let (prepared, count, failed, st) = (&prepared, &count, &failed, &st);
                    sc.spawn(move || {
                        let mut out = Vec::new();
                        // Cada usuario empieza en un endpoint distinto para repartir la carga.
                        let mut i = vu as usize % prepared.len();
                        while Instant::now() < deadline && !stop.load(Relaxed) {
                            // En una rampa, el usuario `vu` espera hasta que le toque estar activo.
                            if vu >= st.vus_at(from, start.elapsed().as_secs_f64()) {
                                std::thread::sleep(Duration::from_millis(20));
                                continue;
                            }
                            let (parts, vars) = &prepared[i];
                            let t = Instant::now();
                            let res = send_parts(client, parts, vars);
                            let at = start.elapsed().as_millis() as u64;
                            let s = match res {
                                Ok(r) => Sample { req: i, at, ms: r.ms, status: Some(r.status), bytes: r.size, err: None },
                                Err(e) => Sample { req: i, at, ms: t.elapsed().as_millis() as u64, status: None, bytes: 0, err: Some(e) },
                            };
                            count.fetch_add(1, Relaxed);
                            if !s.ok() {
                                failed.fetch_add(1, Relaxed);
                            }
                            out.push(s);
                            i = (i + 1) % prepared.len();
                        }
                        out
                    })
                })
                .collect();
            while Instant::now() < deadline && !stop.load(Relaxed) {
                std::thread::sleep(Duration::from_millis(250));
                let elapsed = start.elapsed();
                on_progress(&Progress {
                    stage: si,
                    stages: stages.len(),
                    vus: st.vus_at(from, elapsed.as_secs_f64()),
                    elapsed,
                    secs: st.secs,
                    count: count.load(Relaxed),
                    failed: failed.load(Relaxed),
                });
            }
            workers.into_iter().flat_map(|w| w.join().unwrap_or_default()).collect()
        });
        let elapsed = start.elapsed();
        let all: Vec<&Sample> = samples.iter().collect();
        reports.push(StageReport {
            vus: st.vus,
            secs: st.secs,
            ramp: st.ramp,
            elapsed_ms: elapsed.as_millis() as u64,
            total: stats("Total", &all, elapsed),
            endpoints: targets
                .iter()
                .enumerate()
                .map(|(i, t)| stats(&format!("{} {}", t.method, t.name), &samples.iter().filter(|s| s.req == i).collect::<Vec<_>>(), elapsed))
                .collect(),
            timeline: timeline(&samples, &st, from, elapsed),
        });
        prev_vus = st.vus;
    }
    Ok(reports)
}

fn percentile(sorted: &[u64], q: f64) -> u64 {
    let n = sorted.len();
    // Rango más cercano.
    if n == 0 { 0 } else { sorted[((q / 100.0 * n as f64).ceil() as usize).clamp(1, n) - 1] }
}

fn stats(name: &str, samples: &[&Sample], elapsed: Duration) -> Stats {
    let mut lat: Vec<u64> = samples.iter().map(|s| s.ms).collect();
    lat.sort_unstable();
    let n = lat.len();
    let ok = samples.iter().filter(|s| s.ok()).count();
    let mut statuses = BTreeMap::new();
    let mut errors: Vec<String> = Vec::new();
    for s in samples {
        *statuses.entry(s.status.map_or("error".into(), |c| c.to_string())).or_insert(0) += 1;
        if let Some(e) = &s.err {
            if errors.len() < 5 && !errors.contains(e) {
                errors.push(e.clone());
            }
        }
    }
    Stats {
        name: name.into(),
        count: n,
        ok,
        error_pct: if n == 0 { 0.0 } else { (n - ok) as f64 * 100.0 / n as f64 },
        rps: n as f64 / elapsed.as_secs_f64().max(0.001),
        min: lat.first().copied().unwrap_or(0),
        mean: if n == 0 { 0 } else { lat.iter().sum::<u64>() / n as u64 },
        p50: percentile(&lat, 50.0),
        p90: percentile(&lat, 90.0),
        p95: percentile(&lat, 95.0),
        p99: percentile(&lat, 99.0),
        max: lat.last().copied().unwrap_or(0),
        bytes: samples.iter().map(|s| s.bytes).sum(),
        statuses,
        errors,
    }
}

fn timeline(samples: &[Sample], st: &Stage, from: u32, elapsed: Duration) -> Vec<Bucket> {
    // Un balde por segundo planificado; lo que termina tras el cierre (requests en vuelo) cae en el último.
    let secs = (elapsed.as_secs_f64().ceil() as usize).clamp(1, st.secs.max(1) as usize);
    let mut lat: Vec<Vec<u64>> = vec![Vec::new(); secs];
    let mut errors = vec![0; secs];
    for s in samples {
        let b = ((s.at / 1000) as usize).min(secs - 1);
        lat[b].push(s.ms);
        errors[b] += usize::from(!s.ok());
    }
    lat.into_iter()
        .zip(errors)
        .enumerate()
        .map(|(sec, (mut l, errors))| {
            l.sort_unstable();
            Bucket { sec: sec as u64, vus: st.vus_at(from, sec as f64 + 0.5), count: l.len(), errors, p50: percentile(&l, 50.0), p95: percentile(&l, 95.0) }
        })
        .collect()
}

/// Umbrales para CI: devuelve las violaciones (vacío = pasa).
pub fn check_thresholds(reports: &[StageReport], max_p95: Option<u64>, max_error_pct: Option<f64>) -> Vec<String> {
    let mut v = Vec::new();
    for (i, r) in reports.iter().enumerate() {
        for s in std::iter::once(&r.total).chain(&r.endpoints).filter(|s| s.count > 0) {
            if let Some(m) = max_p95.filter(|&m| s.p95 > m) {
                v.push(format!("Etapa {} ({} VUs) · {}: p95 {} ms > {m} ms", i + 1, r.vus, s.name, s.p95));
            }
            if let Some(m) = max_error_pct.filter(|&m| s.error_pct > m) {
                v.push(format!("Etapa {} ({} VUs) · {}: errores {:.2}% > {m}%", i + 1, r.vus, s.name, s.error_pct));
            }
        }
    }
    v
}

/// "▁▂▅█" para la consola.
pub fn sparkline(values: &[f64]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = values.iter().cloned().fold(0.0, f64::max);
    values.iter().map(|v| if max <= 0.0 { BARS[0] } else { BARS[((v / max) * 7.0).round() as usize] }).collect()
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    use super::*;

    /// Servidor HTTP mínimo: responde 200 con {"path": ...}, o 500 en /fail.
    fn server() -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for mut s in l.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match s.read(&mut chunk) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let head = String::from_utf8_lossy(&buf);
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let (code, body) = if path == "/fail" { ("500 Internal Server Error", "{}".into()) } else { ("200 OK", format!("{{\"path\":\"{path}\"}}")) };
                    let _ = write!(s, "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                });
            }
        });
        format!("http://{addr}")
    }

    fn req(name: &str, url: &str, tests: &str) -> Request {
        Request { name: name.into(), method: "GET".into(), url: url.into(), tests: tests.into(), ..Default::default() }
    }

    #[test]
    fn runner_chains_variables_and_reports() {
        let base = server();
        let col = Collection {
            name: "c".into(),
            requests: vec![
                req("login", "{{base}}/token123", "pm.environment.set('tok', pm.response.json().path.slice(1)); pm.test('200', () => pm.response.to.have.status(200));"),
                req("usa", "{{base}}/x/{{tok}}", "pm.test('encadena', () => pm.expect(pm.response.json().path).to.eql('/x/token123'));"),
                req("falla", "{{base}}/fail", "pm.test('ok', () => pm.response.to.have.status(200));"),
                req("nunca", "{{base}}/y", ""),
            ],
            ..Default::default()
        };
        let mut scopes = Scopes { env: [("base".into(), base)].into(), ..Default::default() };
        let stop = AtomicBool::new(false);
        let mut seen = 0;
        let steps = run_collection(&model::client(), &col, &mut scopes, 1, &[], true, &stop, &mut |_| seen += 1);
        assert_eq!((steps.len(), seen), (3, 3), "bail se detiene en el primer fallo");
        assert!(steps[0].passed() && steps[1].passed() && !steps[2].passed(), "{steps:?}");
        assert_eq!(scopes.env["tok"], "token123");
        let x = junit("c <&>", &steps);
        assert!(x.contains("name=\"c &lt;&amp;&gt;\"") && x.contains("failures=\"1\"") && x.contains("<failure"), "{x}");
    }

    #[test]
    fn runner_data_rows_and_set_next_request() {
        let base = server();
        // "a" usa la fila de datos; "polling" se repite hasta 3 veces con setNextRequest; "salta" nunca corre.
        let col = Collection {
            name: "c".into(),
            requests: vec![
                req("a", "{{base}}/u/{{rol}}", "pm.test('dato', () => pm.expect(pm.response.json().path).to.eql('/u/' + pm.iterationData.get('rol')));"),
                req(
                    "polling",
                    "{{base}}/p",
                    "const n = Number(pm.environment.get('n') || 0) + 1; pm.environment.set('n', n); pm.setNextRequest(n < 3 ? 'polling' : 'fin');",
                ),
                req("salta", "{{base}}/nunca", ""),
                req("fin", "{{base}}/fin", "pm.environment.unset('n'); pm.setNextRequest(null);"),
                req("tras-null", "{{base}}/nunca", ""),
            ],
            ..Default::default()
        };
        let data = parse_data("user;rol\n\"ana; \"\"la jefa\"\"\";admin\r\nbeto;dev\n").unwrap();
        assert_eq!(data[0]["user"], "ana; \"la jefa\"");
        let mut scopes = Scopes { env: [("base".into(), base)].into(), ..Default::default() };
        let stop = AtomicBool::new(false);
        let steps = run_collection(&model::client(), &col, &mut scopes, 3, &data, false, &stop, &mut |_| {});
        let names: Vec<&str> = steps.iter().filter(|s| s.iteration == 1).map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a", "polling", "polling", "polling", "fin"]);
        assert_eq!(steps.len(), 15);
        assert!(steps.iter().all(|s| s.passed()), "{steps:?}");

        let bad = Collection { requests: vec![req("x", "{{base}}/x", "pm.setNextRequest('no-existe')")], ..Default::default() };
        let steps = run_collection(&model::client(), &bad, &mut scopes, 1, &[], false, &stop, &mut |_| {});
        assert!(!steps[0].passed());
        assert_eq!(parse_data(r#"[{"id": 7, "n": "x"}]"#).unwrap()[0]["id"], "7");
    }

    #[test]
    fn stress_collects_stats_per_endpoint() {
        let base = server();
        let targets = [req("ok", &format!("{base}/a/{{{{$guid}}}}"), ""), req("err", &format!("{base}/fail"), "")];
        let stop = AtomicBool::new(false);
        let mut ticks = 0;
        let stages = [Stage { vus: 4, secs: 1, ramp: false }, Stage { vus: 8, secs: 2, ramp: true }];
        let r = stress(&model::client(), &[], &targets, Scopes::default(), &stages, &stop, &mut |_| ticks += 1).unwrap();
        let (t, ok, err) = (&r[0].total, &r[0].endpoints[0], &r[0].endpoints[1]);
        assert!(ticks > 0 && t.count > 10 && t.count == ok.count + err.count, "{t:?}");
        assert_eq!((ok.ok, err.ok, err.error_pct), (ok.count, 0, 100.0));
        assert_eq!(err.statuses["500"], err.count);
        assert!(t.min <= t.p50 && t.p50 <= t.p95 && t.p95 <= t.p99 && t.p99 <= t.max);
        assert!(!check_thresholds(&r, None, Some(1.0)).is_empty() && check_thresholds(&r, None, None).is_empty());
        // La rampa 4→8 se refleja en la línea de tiempo y cada segundo suma lo que se midió.
        let tl = &r[1].timeline;
        assert!(tl.len() >= 2 && tl[0].vus < tl[tl.len() - 1].vus && tl[tl.len() - 1].vus <= 8, "{tl:?}");
        assert_eq!((tl.len(), tl.iter().map(|b| b.count).sum::<usize>()), (2, r[1].total.count));

        let login = req("login", &format!("{base}/fail"), "");
        let e = stress(&model::client(), &[login], &targets, Scopes::default(), &stages, &stop, &mut |_| {}).unwrap_err();
        assert!(e.contains("login") && e.contains("500"), "{e}");
    }

    #[test]
    fn percentiles_ramps_and_sparkline() {
        let s: Vec<Sample> = (1..=100).map(|ms| Sample { req: 0, at: 0, ms, status: Some(200), bytes: 1, err: None }).collect();
        let st = stats("x", &s.iter().collect::<Vec<_>>(), Duration::from_secs(2));
        assert_eq!((st.p50, st.p90, st.p95, st.p99, st.max, st.mean, st.rps), (50, 90, 95, 99, 100, 50, 50.0));
        let ramp = Stage { vus: 100, secs: 10, ramp: true };
        assert_eq!((ramp.vus_at(1, 0.0), ramp.vus_at(1, 5.0), ramp.vus_at(1, 10.0), ramp.vus_at(1, 99.0)), (1, 51, 100, 100));
        assert_eq!(Stage { vus: 1, secs: 10, ramp: true }.vus_at(50, 10.0), 1);
        assert_eq!(sparkline(&[0.0, 5.0, 10.0]), "▁▅█");
    }
}
