# api-connect

*[English](README.en.md)*

Cliente de APIs REST **agentic first**: todo lo que puedes hacer en la app también lo puede hacer un agente de IA, y tú ves, apruebas o deshaces cada cambio. Es una app de escritorio en Rust (egui) e incluye una consola (`api-connect-cli`) pensada para CI y para agentes que trabajan desde la terminal.

## Qué puedes hacer

- **Pedirle al agente lo que quieres lograr.** Por ejemplo: "importa este contrato y prueba los GET", "escribe tests para este request" o "prepara una prueba de carga de 20 usuarios".
  - El agente importa contratos, crea y edita requests, colecciones y entornos, escribe tests, envía requests, corre colecciones y crea y lanza pruebas de estrés. Recibe los resultados y los interpreta.
  - Pide tu aprobación antes de borrar, generar carga, escribir scripts, cambiar la auth o enviar a un host que no usaste. Con un clic deshaces todo lo que hizo en tu última pregunta.
  - Funciona con [Claude Code](https://claude.com/claude-code) (`claude -p`) o con Ollama. Los valores de tus variables (tokens, claves) nunca se le envían.
- **Importar OpenAPI 3 / Swagger 2** (YAML o JSON) desde un archivo, pegando el texto o desde una URL. Se crea un request por endpoint con `{{baseUrl}}`, ejemplos de body y validación contra el schema. También importa colecciones de Postman v2 y comandos cURL. El formato se detecta solo.
- **Armar requests** con variables `{{nombre}}` (prioridad: request > datos > entorno > colección) y variables dinámicas (`{{$guid}}`, `{{$timestamp}}`, …).
- **Escribir scripts pre-request y tests en JavaScript** con la API `pm.*` (motor Boa, sin Node).
- **Autorizar** con Bearer, Basic, API Key u OAuth 2.0 (incluye PKCE). Las cookies se guardan entre sesiones.
- **Correr colecciones** con sus tests y archivos de datos (CSV/JSON), y exportar reportes JUnit o JSON.
- **Hacer pruebas de estrés** con baterías reutilizables, rampas y gráficos de latencia y throughput.
- **Sincronizar contratos desde repos git** (Azure DevOps u otro). Al actualizar el contrato, se hace un merge a 3 vías con tus cambios.

## Instalar

### Desde GitHub Releases (recomendado)

Descarga el paquete de tu sistema desde [Releases](https://github.com/esnorki2008/api-connect/releases), descomprímelo y ejecuta el instalador. No necesitas permisos de administrador.

| Sistema | Paquete | Instalar | Queda en |
|---|---|---|---|
| Windows x64 | `api-connect-vX.Y.Z-windows-x64.zip` | `powershell -ExecutionPolicy Bypass -File install.ps1` | `%LOCALAPPDATA%\Programs\api-connect`, en el PATH y en el menú Inicio |
| Linux x64 | `api-connect-vX.Y.Z-linux-x64.tar.gz` | `./install.sh` | `~/.local/bin` y el menú de aplicaciones |

Para desinstalar: `install.ps1 -Uninstall` (queda copiado en la carpeta de instalación) o `./install.sh --uninstall`. Tus datos no se borran.

En Windows, el ejecutable no está firmado: la primera vez SmartScreen puede avisar. Elige "Más información → Ejecutar de todas formas".

En Linux, la app necesita OpenSSL y, para los diálogos de archivos, `xdg-desktop-portal`; ambos vienen en los escritorios habituales.

### Desde el código

Requisitos:

- [Rust](https://rustup.rs) 1.85 o superior (edition 2024).
- En **Windows**, una de estas dos opciones:
  - [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) con "Desarrollo para el escritorio con C++" (toolchain `msvc`, el predeterminado).
  - MinGW-w64, con `rustup default stable-x86_64-pc-windows-gnu`.
- En **Linux**: `sudo apt install libxkbcommon-dev libssl-dev pkg-config`.
- `git` en el PATH, solo si vas a sincronizar repos.

```sh
git clone https://github.com/esnorki2008/api-connect.git
cd api-connect

# Compila e instala para tu usuario (mismo resultado que el paquete)
./packaging/install.sh                                                # Linux
powershell -ExecutionPolicy Bypass -File packaging\install.ps1        # Windows

# O solo compila: los binarios quedan en target/release/
cargo build --release

# O instala los dos binarios en ~/.cargo/bin
cargo install --path .
```

Para correr los tests: `cargo test`.

Para publicar una versión, sube un tag `vX.Y.Z` (`git tag v0.1.0 && git push --tags`). El workflow `.github/workflows/release.yml` compila para Windows y Linux y adjunta los paquetes al release.

## Dónde guarda tus datos

La app guarda `collections.json`, `environments.json`, `settings.json`, `agents.json`, `cookies.json` y `repos/` en:

- **Windows:** `%APPDATA%\api-connect`
- **Linux:** `$XDG_DATA_HOME/api-connect`, o `~/.local/share/api-connect` si esa variable no existe.

Hay una excepción, el **modo portable**: si la carpeta desde donde la abres ya tiene `collections.json` o `settings.json`, usa esa carpeta. Para pasar datos de una versión anterior, copia esos archivos a la carpeta de datos.

Estos archivos pueden contener tokens y contraseñas. Por eso están en `.gitignore` y nunca se le envían al agente.

## Configurar el agente

El agente usa una de estas dos opciones (elige en el selector del panel; **Configurar** edita la lista):

- **Claude** (predeterminado, modelo Haiku): instala [Claude Code](https://claude.com/claude-code) e inicia sesión una vez con `claude` en una terminal. La app lo llama con `claude -p`, sin herramientas ni MCP.
- **Ollama** (local, sin cuenta): instala [Ollama](https://ollama.com), descarga un modelo (`ollama pull llama3.2`) y deja corriendo el servicio en `http://127.0.0.1:11434`.

Si el agente responde que no encuentra `claude`, revisa que esté en el PATH y reinicia la app.

## Idioma

La interfaz, la consola y las respuestas del agente están en **español** (por defecto) o en **inglés**. En la app, el botón **EN / ES** de la barra superior cambia el idioma y lo recuerda. En la consola el orden es: `--lang en|es`, luego la variable `API_CONNECT_LANG` y luego el idioma elegido en la app. *English documentation: [README.en.md](README.en.md).*

## Usar la app

Abre **api-connect** desde el menú o ejecuta `api-connect`. El panel del agente está a la derecha: escribe lo que quieres lograr o elige una sugerencia. Lo demás también lo puedes hacer a mano:

- **📥 Importar:** OpenAPI, Postman o cURL, desde URL, archivo o texto pegado.
- **Barra lateral:** colecciones, entornos e historial.
- **Centro:** editor del request y respuesta.
- **▶ Runner / ⚡ Estrés:** corre la colección o lanza una batería de estrés.

Atajos: `Ctrl+Enter` envía (o le pregunta al agente si el cursor está en su caja de texto) · `Ctrl+S` guarda · `Ctrl+T` abre una pestaña nueva · `Ctrl+W` cierra la pestaña.

### Acciones del agente

El agente responde con bloques `actions` que contienen JSON, y la app los ejecuta en orden:

| Acción | Para qué | ¿Pide aprobación? |
|---|---|---|
| `create_collection`, `create_request`, `update_request`, `edit_request`, `save_request`, `open_request` | Crear y editar requests y colecciones (incluida su `auth`) | Solo si escribe scripts (`pre`/`tests`) o cambia la `auth`: tocan tus secretos |
| `set_collection_auth` | Bearer, Basic, API Key u OAuth 2.0 de la colección (secretos como `{{variables}}`) | **Siempre** |
| `set_variables`, `create_environment`, `select_environment` | Variables y entornos | No |
| `send` | Envía el request abierto y le devuelve status, body y tests | Si el host (o el de OAuth) es nuevo |
| `import_openapi` (`url` o `spec`) | Importa un contrato como colección | Si el host es nuevo |
| `run_collection` | Corre la colección con sus tests y le devuelve el resumen | Si algún host es nuevo |
| `create_battery` | Guarda una batería de estrés (etapas de usuarios virtuales, duración y rampa) | No |
| `run_stress` | Lanza una prueba de estrés y le devuelve p50/p95/p99, errores y req/s | **Siempre** |
| `delete_request`, `delete_collection` | Borrar | **Siempre** |

## Consola (`api-connect-cli`)

Usa los mismos datos que la app. Para usar otra carpeta, pásala con `--dir`. Con `--json -`, el resultado sale como JSON por stdout (los errores también, como `{"error": …}`) y el progreso por stderr, para que lo lea otro programa o un agente que trabaja desde la terminal (Claude Code, Codex, etc.).

```sh
# Importar un contrato (archivo, URL o - para stdin): agrega la colección
api-connect-cli import https://petstore3.swagger.io/api/v3/openapi.json

# Ver colecciones, entornos y baterías (en JSON)
api-connect-cli list --json -

# Aplicar acciones del agente (mismo JSON que en la app); borrar, scripts o auth exigen --yes.
# Las que salen bien se guardan aunque otras fallen; el JSON dice cuál falló y por qué.
echo '[{"action":"create_environment","name":"dev","values":{"token":"..."},"activate":true},
       {"action":"set_collection_auth","collection":"Mi API","auth":{"type":"bearer","token":"{{token}}"}}]'   | api-connect-cli apply - --yes --json -

# Enviar un request guardado: status, headers, body y tests en JSON
api-connect-cli send "Mi API" "Listar" --env dev --json -

# Correr una colección con sus tests
api-connect-cli run "Mi colección" --env dev -n 3 --data datos.csv --junit reporte.xml

# Prueba de estrés de un endpoint, con login previo y umbrales
api-connect-cli stress "Mi colección" --request "Listar" --setup "Login" --vus 20 --secs 30 --ramp 5 --max-p95 500 --max-errors 1 --json -
```

Ayuda completa: `api-connect-cli --help`. Para mensajes en inglés: `--lang en` (o `API_CONNECT_LANG=en`).

Códigos de salida: `0` todo pasó · `1` fallaron tests o se superaron los umbrales · `2` error de uso o de configuración.

## Arquitectura

Las capas van de adentro hacia afuera, y cada una solo usa las que están más adentro:

```
src/
├── domain/   entidades y reglas puras: model, vars ({{variables}}), actions (protocolo del agente), overlay (merge/validación)
├── engine/   casos de uso: script (JS pm.*), runner (colecciones y estrés)
├── import/   OpenAPI, Postman y cURL → colecciones (detect elige el formato)
├── infra/    mundo exterior: http, storage (JSON y carpeta de datos), cookies, auth (OAuth), repo (git), agent (Claude/Ollama)
├── ui/       app de escritorio (egui): App en mod.rs; cada archivo agrega una parte (sidebar, editor, agent_panel, run_window…)
├── i18n.rs   textos en español (por defecto) e inglés: t("…", "…") y tr!("…", "…", args)
├── main.rs              binario api-connect (solo abre ui::run)
└── bin/api-connect-cli.rs  binario de consola
packaging/    instaladores (install.ps1, install.sh)
```
