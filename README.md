# api-connect

Cliente HTTP de escritorio tipo Postman, escrito en Rust (egui). Incluye una consola (`api-connect-cli`) para correr colecciones y pruebas de estrés en CI.

## Funciones

- Requests HTTP con variables `{{nombre}}` (request > datos > entorno > colección) y dinámicas (`{{$guid}}`, `{{$timestamp}}`, …).
- Scripts pre-request y tests en JavaScript con API compatible con `pm.*` de Postman (motor Boa, sin Node).
- Autorización: Bearer, Basic, API Key y OAuth 2.0 (con PKCE).
- Cookie jar persistente.
- Importación de cURL, colecciones de Postman v2.x y OpenAPI 3.x / Swagger 2.0 (YAML o JSON), con merge a 3 vías al actualizar el contrato.
- Sincronización de colecciones desde repos git (Azure DevOps u otro) usando las credenciales de git.
- Asistente de IA opcional vía [Claude Code](https://claude.com/claude-code) (`claude -p`) u Ollama.
- Runner de colecciones con archivos de datos (CSV/JSON), reportes JUnit/JSON y pruebas de estrés.

## Compilar

Requisitos:

- [Rust](https://rustup.rs) 1.85 o superior (edition 2024).
- En Windows, [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) con la carga de trabajo "Desarrollo para el escritorio con C++" (provee `link.exe`).
- `git` en el PATH (solo para sincronizar repos).
- En Linux, las dependencias de egui/rfd: `sudo apt install libgtk-3-dev libxkbcommon-dev libssl-dev pkg-config`.

```sh
git clone https://github.com/esnorki2008/api-connect.git
cd api-connect
cargo build --release
```

Los binarios quedan en `target/release/`:

| Binario | Uso |
|---|---|
| `api-connect` (`.exe`) | App de escritorio |
| `api-connect-cli` (`.exe`) | Consola para CI |

Tests: `cargo test`

## Uso

### App de escritorio

```sh
cargo run --release
```

La app guarda sus datos **en la carpeta desde donde se ejecuta**: `collections.json`, `environments.json`, `settings.json`, `agents.json`, `cookies.json` y `repos/`. Esos archivos están en `.gitignore` porque pueden contener tokens y contraseñas.

Atajos: `Ctrl+Enter` enviar · `Ctrl+S` guardar · `Ctrl+T` nueva pestaña · `Ctrl+W` cerrar pestaña.

### Consola (CI)

Usa los mismos JSON que la app. Corre desde esa carpeta o indícala con `--dir`.

```sh
# Ver colecciones, entornos y baterías de estrés
api-connect-cli list

# Correr una colección (como Newman)
api-connect-cli run "Mi colección" --env dev -n 3 --data datos.csv --junit reporte.xml

# Prueba de estrés de un endpoint, con login previo y umbrales
api-connect-cli stress "Mi colección" --request "Listar" --setup "Login" --vus 20 --secs 30 --ramp 5 --max-p95 500 --max-errors 1
```

Ayuda completa: `api-connect-cli --help`.

Códigos de salida: `0` todo pasó · `1` fallaron tests o se superaron umbrales · `2` error de uso o configuración.
