//! Agentes de IA: `claude -p` (Claude Code en modo mínimo) u Ollama por HTTP.

use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    domain::model::AgentCfg,
    i18n::{self, t},
    infra::http::err_chain,
    tr,
};

pub const SYSTEM: &str = "Eres el agente de api-connect, un cliente de APIs REST \"agentic first\": todo lo que la persona hace en la app, lo puedes
hacer tú con acciones. Responde en español, breve y directo; la persona ve tus mensajes y cada cambio que haces.
Cuando escribas scripts, usa SOLO esta API (no existe require ni pm.iterationData; los scripts admiten await):
- Variables con precedencia request > entorno > colección; en URL, headers y body se escriben como {{nombre}}.
  Dinámicas: {{$guid}} {{$timestamp}} {{$isoTimestamp}} {{$randomInt}}.
- pm.variables.get/set/unset/has/toObject/replaceIn(str): set guarda solo para este envío (pre-request -> tests).
- pm.environment.get/set/unset/has/clear/toObject: entorno activo, se guarda en disco.
- pm.collectionVariables.get/set/unset/has/clear/toObject: variables de la colección, se guardan en disco.
- pm.request (en pre-request se puede modificar): method, url (string), body.raw / body.update(objOString),
  headers.add({key,value}) / upsert({key,value}) / remove(key) / get(key) / has(key) / toObject().
- pm.response (solo en tests): code, status, responseTime, json(), text(), headers.get(nombre), to.have.status(n).
- pm.test(nombre, fn) y pm.expect(v) con: to.equal, to.eql, to.deep.equal, to.include, to.be.above, to.be.below,
  to.have.property, to.have.lengthOf, to.be.a('string'|'number'|'array'|...), to.be.true, to.be.false, to.be.ok, to.exist, to.be.empty.
- pm.sendRequest(urlOrReq, (err, res) => {...}) o const res = await pm.sendRequest(urlOrReq);
  urlOrReq = 'https://...' o { url, method, header: [{key,value}], body: { mode: 'raw', raw } | { mode: 'urlencoded', urlencoded: [{key,value}] } }.
  res tiene la misma API que pm.response. No resuelve {{vars}} sola: usa pm.variables.replaceIn(url).
- console.log(...) aparece en la consola de la respuesta.
Pon cada script en su propio bloque ```js. Indica si va en Pre-request o en Tests.

PUEDES MODIFICAR LA PLATAFORMA. Para hacerlo, incluye un bloque ```actions con un objeto JSON o una lista; la app lo
ejecuta en orden y te devuelve el resultado (y la respuesta HTTP si enviaste). Acciones:
- {\"action\":\"edit_request\",\"set\":{campos}}            cambia el request abierto (sin guardar)
- {\"action\":\"save_request\",\"collection\":\"opcional\"}  guarda el request abierto
- {\"action\":\"open_request\",\"collection\":\"C\",\"name\":\"R\"}
- {\"action\":\"create_collection\",\"name\":\"C\"}
- {\"action\":\"create_request\",\"collection\":\"C\",\"request\":{campos con name}}
- {\"action\":\"update_request\",\"collection\":\"C\",\"name\":\"R\",\"set\":{campos}}
- {\"action\":\"delete_request\",\"collection\":\"C\",\"name\":\"R\"}   (pide aprobación al usuario)
- {\"action\":\"delete_collection\",\"name\":\"C\"}                   (pide aprobación al usuario)
- {\"action\":\"set_variables\",\"scope\":\"environment\"|\"collection\",\"values\":{\"k\":\"v\"},\"unset\":[\"k\"]}
- {\"action\":\"create_environment\",\"name\":\"E\",\"values\":{...},\"activate\":true}
- {\"action\":\"select_environment\",\"name\":\"E\"}
- {\"action\":\"send\"}   envía el request abierto; recibes status, body y tests para seguir iterando
- {\"action\":\"import_openapi\",\"url\":\"https://.../openapi.json\"}  o  {\"action\":\"import_openapi\",\"spec\":\"texto YAML/JSON\"}
     crea una colección con un request por endpoint (variable {{baseUrl}}, ejemplos de body y schema para validar)
- {\"action\":\"run_collection\",\"collection\":\"C\",\"iterations\":1}   corre todos sus requests en orden con sus tests;
     recibes el resumen y los fallos
- {\"action\":\"create_battery\",\"name\":\"B\",\"description\":\"...\",\"stages\":[{\"vus\":20,\"secs\":10,\"ramp\":true},{\"vus\":20,\"secs\":60}]}
     guarda una batería de estrés reutilizable (vus 1..500 usuarios virtuales, secs 1..3600; ramp = subir en línea recta)
- {\"action\":\"run_stress\",\"collection\":\"C\",\"request\":\"opcional R\",\"setup\":\"opcional login\",\"battery\":\"B\"}
     o con \"stages\":[...] en lugar de battery. SIEMPRE pide aprobación. Recibes por etapa y endpoint: n, % de error,
     req/s, p50/p95/p99/max en ms y códigos HTTP. setup se envía una vez antes de medir (p. ej. el login que guarda el token).
     Sin request, estresa toda la colección menos el setup. En estrés no corren los tests.
Solo una acción que ejecuta (run_collection, run_stress o import_openapi) por respuesta: espera su resultado antes de seguir.
- {\"action\":\"set_collection_auth\",\"collection\":\"C\",\"auth\":{auth}}   auth que heredan sus requests
campos = name, method, url, headers (texto \"Clave: Valor\" por línea u objeto), body (texto u objeto JSON), pre, tests, auth.
auth = {\"type\":\"inherit\"|\"none\"} | {\"type\":\"bearer\",\"token\":\"{{token}}\"} | {\"type\":\"basic\",\"user\":\"..\",\"pass\":\"{{pass}}\"}
  | {\"type\":\"api_key\",\"key\":\"X-Api-Key\",\"value\":\"{{key}}\",\"in_query\":false}
  | {\"type\":\"oauth2\",\"grant\":\"client_credentials\"|\"password\"|\"authorization_code\",\"token_url\":\"..\",\"auth_url\":\"..\",
     \"client_id\":\"{{client_id}}\",\"client_secret\":\"{{client_secret}}\",\"scope\":\"..\"}  (el token se obtiene solo al enviar).
  Los secretos van SIEMPRE como {{variables}}; tú no ves sus valores.
Escribir scripts (pre o tests) o cambiar una auth siempre pide aprobación al usuario: tocan sus secretos.
Reglas: los requests que vienen de un CONTRATO (openapi del repo) se editan solo como versión del usuario; el contrato es
de solo lectura. Respeta el schema del contrato salvo que el usuario pida lo contrario. Usa {{variables}} en lugar de
valores sensibles. Haz solo lo que el usuario pidió y explica en una línea qué vas a hacer antes del bloque. Para pruebas de estrés, propón
una batería acorde a lo pedido (humo antes que carga) y, al recibir los resultados, interpreta latencias y errores en
lenguaje claro: qué endpoint se degrada, desde cuántos usuarios y qué revisar.";

/// SYSTEM más el idioma en que debe responder (el de la interfaz).
pub fn system_prompt() -> String {
    let lang = if i18n::en() { "English" } else { "español" };
    tr!(
        "{SYSTEM}\nIDIOMA: responde SIEMPRE en {lang} (el idioma de la interfaz), aunque el contexto esté en otro idioma.",
        "{SYSTEM}\nLANGUAGE: ALWAYS reply in {lang} (the UI language), even if the context is in another language.",
    )
}

/// messages: (rol, contenido) con rol "user" | "assistant". Bloquea: llamar desde un hilo de trabajo.
pub fn ask(
    client: &reqwest::blocking::Client,
    a: &AgentCfg,
    messages: &[(String, String)],
    claude_timeout: Duration,
) -> Result<String, String> {
    // El modelo llega como argumento de proceso: sin esto, "--algo" se colaría como flag de claude.
    let m = &a.model;
    if m.is_empty() || m.starts_with('-') || !m.chars().all(|c| c.is_ascii_alphanumeric() || "._:-/".contains(c)) {
        return Err(tr!("modelo inválido: {m:?}", "invalid model: {m:?}"));
    }
    match a.kind.as_str() {
        "claude" => claude(a, messages, claude_timeout),
        "ollama" => ollama(client, a, messages),
        k => Err(tr!("tipo de agente desconocido: {k:?} (usa claude u ollama)", "unknown agent kind: {k:?} (use claude or ollama)")),
    }
}

// claude -p en modo mínimo: sin tools, MCP, skills ni CLAUDE.md; solo responde texto (~600 tokens de contexto).
// Corre en el directorio temporal para no cargar el CLAUDE.md de ningún proyecto.
fn claude(a: &AgentCfg, messages: &[(String, String)], timeout: Duration) -> Result<String, String> {
    let effort = if a.effort.is_empty() { "low" } else { &a.effort };
    if !["low", "medium", "high", "xhigh", "max"].contains(&effort) {
        return Err(tr!("effort inválido: {effort:?}", "invalid effort: {effort:?}"));
    }
    let prompt = messages
        .iter()
        .map(|(role, content)| format!("{}:\n{content}", if role == "assistant" { t("Asistente", "Assistant") } else { t("Usuario", "User") }))
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut cmd = Command::new("claude");
    cmd.args(["-p", "--output-format", "json", "--tools", "", "--strict-mcp-config", "--no-session-persistence"])
        .args(["--disable-slash-commands", "--setting-sources", "local"])
        .args(["--model", &a.model, "--effort", effort, "--system-prompt", &system_prompt()])
        .current_dir(std::env::temp_dir());
    let (stdout, stderr) = run_with_timeout(cmd, &prompt, timeout).map_err(|e| match e {
        RunError::Spawn(e) => tr!(
            "no se pudo ejecutar claude (¿está en el PATH?): {e}. Instala Claude Code e inicia sesión con `claude`, o elige Ollama en Configurar.",
            "could not run claude (is it on the PATH?): {e}. Install Claude Code and sign in with `claude`, or pick Ollama in Configure.",
        ),
        RunError::Timeout => tr!(
            "claude no respondió en {} s. Ajusta el timeout en Agente → Configurar.",
            "claude did not answer within {} s. Adjust the timeout in Agent → Configure.",
            timeout.as_secs()
        ),
        RunError::Io(e) => e,
    })?;

    let v: Value = serde_json::from_slice(&stdout).map_err(|_| {
        let err = String::from_utf8_lossy(if stderr.is_empty() { &stdout } else { &stderr });
        tr!("claude falló: {}", "claude failed: {}", err.trim())
    })?;
    let result = v["result"].as_str().unwrap_or_default().to_string();
    if v["is_error"].as_bool().unwrap_or(false) {
        return Err(format!("claude: {result}"));
    }
    Ok(result)
}

#[derive(Debug)]
enum RunError {
    Spawn(std::io::Error),
    Timeout,
    Io(String),
}

/// Ejecuta `cmd` escribiendo `input` en su stdin; lo mata si no termina dentro de `timeout`.
/// Devuelve (stdout, stderr). Lee ambas salidas en hilos para que un pipe lleno no lo bloquee.
fn run_with_timeout(mut cmd: Command, input: &str, timeout: Duration) -> Result<(Vec<u8>, Vec<u8>), RunError> {
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: sin consola emergente
    }
    let mut child = cmd.spawn().map_err(RunError::Spawn)?;
    let reader = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = r.read_to_end(&mut b);
            b
        })
    };
    let out = reader(Box::new(child.stdout.take().expect("stdout")));
    let err = reader(Box::new(child.stderr.take().expect("stderr")));
    // Un proceso que no lee stdin no debe tumbar la llamada: se ignora el error de escritura.
    let _ = child.stdin.take().expect("stdin").write_all(input.as_bytes());

    let deadline = Instant::now() + timeout;
    while child.try_wait().map_err(|e| RunError::Io(e.to_string()))?.is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok((out.join().unwrap_or_default(), err.join().unwrap_or_default()))
}

fn ollama(client: &reqwest::blocking::Client, a: &AgentCfg, messages: &[(String, String)]) -> Result<String, String> {
    let base = if a.url.is_empty() { "http://127.0.0.1:11434" } else { a.url.trim_end_matches('/') };
    let mut msgs = vec![json!({ "role": "system", "content": system_prompt() })];
    msgs.extend(messages.iter().map(|(role, content)| json!({ "role": role, "content": content })));
    let res = client
        .post(format!("{base}/api/chat"))
        .timeout(Duration::from_secs(300))
        .json(&json!({ "model": a.model, "messages": msgs, "stream": false }))
        .send()
        .map_err(err_chain)?;
    let status = res.status();
    let v: Value = res.json().map_err(err_chain)?;
    if !status.is_success() {
        return Err(format!("ollama {status}: {}", v["error"].as_str().unwrap_or("error")));
    }
    Ok(v["message"]["content"].as_str().unwrap_or_default().to_string())
}

/// Parte la respuesta según los bloques ```lang ... ```: (None, texto) o (Some(lenguaje), código).
pub fn split_code(text: &str) -> Vec<(Option<String>, String)> {
    text.split("```")
        .enumerate()
        .filter_map(|(i, part)| {
            if i % 2 == 0 {
                let t = part.trim();
                (!t.is_empty()).then(|| (None, t.to_string()))
            } else {
                // La primera línea es el lenguaje si es una sola palabra ("js", "json", "actions"...).
                let (lang, code) = match part.split_once('\n') {
                    Some((lang, rest)) if !lang.trim().contains(' ') => (lang.trim(), rest),
                    _ => ("", part),
                };
                Some((Some(lang.to_string()), code.trim_end().to_string()))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_code_blocks() {
        let parts = split_code("Pon esto en Tests:\n```js\npm.test('a', () => {});\n```\nListo.");
        assert_eq!(
            parts,
            vec![
                (None, "Pon esto en Tests:".to_string()),
                (Some("js".to_string()), "pm.test('a', () => {});".to_string()),
                (None, "Listo.".to_string())
            ]
        );
    }

    #[test]
    fn rejects_flag_like_models() {
        let a = AgentCfg { name: "x".into(), kind: "claude".into(), model: "--dangerously-skip-permissions".into(), effort: "".into(), url: "".into() };
        assert!(ask(&crate::infra::http::client(), &a, &[], Duration::from_secs(1)).unwrap_err().contains("modelo inválido"));
    }

    /// Llama a Claude Haiku de verdad (consume cuota): `cargo test --release -- --ignored haiku`
    #[test]
    #[ignore]
    fn haiku_follows_actions_protocol() {
        let a = AgentCfg { name: "h".into(), kind: "claude".into(), model: "claude-haiku-5-5".into(), effort: "low".into(), url: "".into() };
        let q = "Crea la colección \"Demo\" con un request \"Ping\" GET a {{base}}/ping que tenga un test de status 200, \
                 crea el entorno \"qa\" con base=http://localhost:9 y actívalo, y envía el request.\n\n---\nContexto:\nColecciones: (ninguna)\nEntornos: (ninguno)";
        let reply = ask(&crate::infra::http::client(), &a, &[("user".into(), q.into())], Duration::from_secs(120)).unwrap();
        let acts = crate::domain::actions::extract(&reply);
        println!("{reply}\n{acts:#?}");
        assert!(!acts.is_empty() && acts.iter().all(Result::is_ok), "acciones inválidas: {acts:?}");
        let acts: Vec<_> = acts.into_iter().map(Result::unwrap).collect();
        assert!(acts.iter().any(|x| matches!(x, crate::domain::actions::Action::CreateRequest { .. })));
        assert!(acts.iter().any(|x| matches!(x, crate::domain::actions::Action::CreateEnvironment { .. })));
    }

    /// Igual que el anterior, para pruebas de estrés: `cargo test --release -- --ignored haiku`
    #[test]
    #[ignore]
    fn haiku_plans_stress_tests() {
        let a = AgentCfg { name: "h".into(), kind: "claude".into(), model: "claude-haiku-5-5".into(), effort: "low".into(), url: "".into() };
        let q = "Prepara una prueba de carga sobre Tienda / Listar: rampa a 20 usuarios en 10 s y luego 1 minuto sostenido, \
                 con Login como preparación. Guárdala como batería para reusarla y córrela.\n\n---\nContexto:\nColecciones:\n- Tienda (2 requests)\n    POST Login\n    GET Listar\n\
                 Baterías de estrés: Humo (1 VUs·10 s); Carga (rampa→10 VUs·15 s → 10 VUs·60 s)";
        let reply = ask(&crate::infra::http::client(), &a, &[("user".into(), q.into())], Duration::from_secs(120)).unwrap();
        let acts = crate::domain::actions::extract(&reply);
        println!("{reply}\n{acts:#?}");
        assert!(!acts.is_empty() && acts.iter().all(Result::is_ok), "acciones inválidas: {acts:?}");
        use crate::domain::actions::Action;
        assert!(acts.iter().flatten().any(|x| matches!(x, Action::CreateBattery { stages, .. } if stages.len() == 2)));
        assert!(acts.iter().flatten().any(|x| matches!(x, Action::RunStress { setup: Some(s), .. } if s == "Login")));
    }

    #[cfg(windows)]
    #[test]
    fn kills_process_on_timeout() {
        let mut slow = Command::new("cmd");
        slow.args(["/C", "ping -n 10 127.0.0.1 >NUL"]);
        let t = Instant::now();
        assert!(matches!(run_with_timeout(slow, "", Duration::from_millis(500)), Err(RunError::Timeout)));
        assert!(t.elapsed() < Duration::from_secs(5));

        let mut fast = Command::new("cmd");
        fast.args(["/C", "findstr x"]); // repite por stdout las líneas de stdin que contienen "x"
        let (out, _) = run_with_timeout(fast, "xyz\nabc\n", Duration::from_secs(10)).unwrap();
        assert_eq!(String::from_utf8_lossy(&out).trim(), "xyz");
    }
}
