# api-connect

*[Español](README.md)*

An **agentic first** REST API client: anything you can do in the app, an AI agent can do too, and you see, approve or undo every change. It is a Rust desktop app (egui) and ships with a command-line tool (`api-connect-cli`) built for CI and for agents that work from the terminal.

## What you can do

- **Tell the agent what you want to achieve.** For example: "import this contract and test the GETs", "write tests for this request" or "set up a load test with 20 users".
  - The agent imports contracts, creates and edits requests, collections and environments, writes tests, sends requests, runs collections, and creates and launches stress tests. It gets the results back and explains them.
  - It asks for your approval before deleting, generating load, writing scripts, changing auth or sending to a host you haven't used. One click undoes everything it did for your last question.
  - It works with [Claude Code](https://claude.com/claude-code) (`claude -p`) or with Ollama. Your variable values (tokens, keys) are never sent to it.
- **Import OpenAPI 3 / Swagger 2** (YAML or JSON) from a file, pasted text or a URL. You get one request per endpoint with `{{baseUrl}}`, example bodies and validation against the schema. It also imports Postman v2 collections and cURL commands. The format is detected automatically.
- **Build requests** with `{{name}}` variables (priority: request > data > environment > collection) and dynamic variables (`{{$guid}}`, `{{$timestamp}}`, …).
- **Write pre-request scripts and tests in JavaScript** with the `pm.*` API (Boa engine, no Node).
- **Authorize** with Bearer, Basic, API Key or OAuth 2.0 (including PKCE). Cookies are kept between sessions.
- **Run collections** with their tests and data files (CSV/JSON), and export JUnit or JSON reports.
- **Stress test** with reusable batteries, ramps, and latency and throughput charts.
- **Sync contracts from git repos** (Azure DevOps or any other). When the contract changes, it is 3-way merged with your edits.

## Install

### From GitHub Releases (recommended)

Download the package for your system from [Releases](https://github.com/esnorki2008/api-connect/releases), unpack it and run the installer. No admin rights needed.

| System | Package | Install | Installed to |
|---|---|---|---|
| Windows x64 | `api-connect-vX.Y.Z-windows-x64.zip` | `powershell -ExecutionPolicy Bypass -File install.ps1` | `%LOCALAPPDATA%\Programs\api-connect`, on the PATH and in the Start menu |
| Linux x64 | `api-connect-vX.Y.Z-linux-x64.tar.gz` | `./install.sh` | `~/.local/bin` and the applications menu |

To uninstall: `install.ps1 -Uninstall` (a copy is kept in the install folder) or `./install.sh --uninstall`. Your data is not deleted.

On Windows the executable is not signed, so SmartScreen may warn you the first time. Choose "More info → Run anyway".

On Linux the app needs OpenSSL and, for file dialogs, `xdg-desktop-portal`; both come with the usual desktops.

### From source

Requirements:

- [Rust](https://rustup.rs) 1.85 or newer (edition 2024).
- On **Windows**, one of:
  - [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with "Desktop development with C++" (`msvc` toolchain, the default).
  - MinGW-w64, with `rustup default stable-x86_64-pc-windows-gnu`.
- On **Linux**: `sudo apt install libxkbcommon-dev libssl-dev pkg-config`.
- `git` on the PATH, only if you sync repos.

```sh
git clone https://github.com/esnorki2008/api-connect.git
cd api-connect

# Build and install for your user (same result as the package)
./packaging/install.sh                                                # Linux
powershell -ExecutionPolicy Bypass -File packaging\install.ps1        # Windows

# Or just build: binaries end up in target/release/
cargo build --release

# Or install both binaries into ~/.cargo/bin
cargo install --path .
```

Run the tests with `cargo test`.

To publish a version, push a `vX.Y.Z` tag (`git tag v0.1.0 && git push --tags`). The `.github/workflows/release.yml` workflow builds for Windows and Linux and attaches the packages to the release.

## Where your data lives

The app stores `collections.json`, `environments.json`, `settings.json`, `agents.json`, `cookies.json` and `repos/` in:

- **Windows:** `%APPDATA%\api-connect`
- **Linux:** `$XDG_DATA_HOME/api-connect`, or `~/.local/share/api-connect` if that variable isn't set.

One exception is **portable mode**: if the folder you open it from already has `collections.json` or `settings.json`, it uses that folder. To bring data from an earlier version, copy those files to the data folder.

These files may contain tokens and passwords. That's why they are in `.gitignore` and are never sent to the agent.

## Language

The interface, the command-line tool and the agent's answers are in **Spanish** (default) or **English**. In the app, the **EN / ES** button in the top bar switches the language and remembers it. On the command line the order is: `--lang en|es`, then the `API_CONNECT_LANG` variable, then the language chosen in the app.

## Set up the agent

The agent uses one of these two options (pick it in the panel's selector; **Configure** edits the list):

- **Claude** (default, Haiku model): install [Claude Code](https://claude.com/claude-code) and sign in once by running `claude` in a terminal. The app calls it with `claude -p`, without tools or MCP.
- **Ollama** (local, no account): install [Ollama](https://ollama.com), pull a model (`ollama pull llama3.2`) and keep the service running at `http://127.0.0.1:11434`.

If the agent says it can't find `claude`, check that it is on the PATH and restart the app.

## Using the app

Open **api-connect** from the menu or run `api-connect`. The agent panel is on the right: type what you want to achieve or pick a suggestion. You can also do everything by hand:

- **📥 Import:** OpenAPI, Postman or cURL, from a URL, a file or pasted text.
- **Sidebar:** collections, environments and history.
- **Center:** request editor and response.
- **▶ Runner / ⚡ Stress:** run the collection or launch a stress battery.

Shortcuts: `Ctrl+Enter` sends (or asks the agent if the cursor is in its text box) · `Ctrl+S` saves · `Ctrl+T` opens a new tab · `Ctrl+W` closes the tab.

### Agent actions

The agent answers with `actions` blocks containing JSON, and the app runs them in order:

| Action | What for | Needs approval? |
|---|---|---|
| `create_collection`, `create_request`, `update_request`, `edit_request`, `save_request`, `open_request` | Create and edit requests and collections (including their `auth`) | Only when it writes scripts (`pre`/`tests`) or changes `auth`: they touch your secrets |
| `set_collection_auth` | Collection Bearer, Basic, API Key or OAuth 2.0 (secrets as `{{variables}}`) | **Always** |
| `set_variables`, `create_environment`, `select_environment` | Variables and environments | No |
| `send` | Sends the open request and returns status, body and tests | If the host (or the OAuth one) is new |
| `import_openapi` (`url` or `spec`) | Imports a contract as a collection | If the host is new |
| `run_collection` | Runs the collection with its tests and returns the summary | If any host is new |
| `create_battery` | Saves a stress battery (stages of virtual users, duration and ramp) | No |
| `run_stress` | Launches a stress test and returns p50/p95/p99, errors and req/s | **Always** |
| `delete_request`, `delete_collection` | Delete | **Always** |

## Command line (`api-connect-cli`)

It uses the same data as the app. To use another folder, pass it with `--dir`. With `--json -`, the result goes to stdout as JSON (errors too, as `{"error": …}`) and progress goes to stderr, so another program or a terminal agent (Claude Code, Codex, etc.) can read it.

```sh
# Import a contract (file, URL or - for stdin): adds the collection
api-connect-cli import https://petstore3.swagger.io/api/v3/openapi.json

# List collections, environments and batteries (as JSON)
api-connect-cli list --json -

# Apply agent actions (same JSON as in the app); deletes, scripts and auth need --yes.
# Successful actions are saved even if others fail; the JSON says which one failed and why.
echo '[{"action":"create_environment","name":"dev","values":{"token":"..."},"activate":true},
       {"action":"set_collection_auth","collection":"My API","auth":{"type":"bearer","token":"{{token}}"}}]' \
  | api-connect-cli apply - --yes --json -

# Send a saved request: status, headers, body and tests as JSON
api-connect-cli send "My API" "List" --env dev --json -

# Run a collection with its tests
api-connect-cli run "My collection" --env dev -n 3 --data data.csv --junit report.xml

# Stress test one endpoint, with a login first and thresholds
api-connect-cli stress "My collection" --request "List" --setup "Login" --vus 20 --secs 30 --ramp 5 --max-p95 500 --max-errors 1 --json -
```

Full help: `api-connect-cli --help --lang en`.

Exit codes: `0` everything passed · `1` tests failed or thresholds were exceeded · `2` usage or configuration error.

## Architecture

Layers go from the inside out, and each one only uses the layers inside it:

```
src/
├── domain/   pure entities and rules: model, vars ({{variables}}), actions (agent protocol), overlay (merge/validation)
├── engine/   use cases: script (JS pm.*), runner (collections and stress)
├── import/   OpenAPI, Postman and cURL → collections (detect picks the format)
├── infra/    outside world: http, storage (JSON and data folder), cookies, auth (OAuth), repo (git), agent (Claude/Ollama)
├── ui/       desktop app (egui): App in mod.rs; each file adds one part (sidebar, editor, agent_panel, run_window…)
├── i18n.rs   Spanish (default) / English texts: t("…", "…") and tr!("…", "…", args)
├── main.rs              api-connect binary (just opens ui::run)
└── bin/api-connect-cli.rs  command-line binary
packaging/    installers (install.ps1, install.sh)
```
