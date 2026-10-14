# Cedar for macOS

A menu bar app that runs cedar and cedar-chat on a Mac from a single DMG. It bundles both server
binaries, starts them as child processes, pulls the sandbox image, and shows the chat UI in its
window. Two things stay outside the app:

- **Docker.** Each agent run's tools execute in a container, so Docker Desktop, OrbStack, Colima,
  or Rancher Desktop must be installed and running. The app finds the `docker` command in the
  usual places and tells you when it can't.
- **The model server.** vLLM doesn't run on macOS, so the app asks for the address of any
  OpenAI-compatible server with tool calling: Ollama or LM Studio on the Mac, or a vLLM on another
  machine (a DGX Spark over Tailscale, say). It lists the server's models and you pick one.

```
Cedar.app (menu bar) ──┬── cedar-chat :13000 ──▶ cedar :18080 ──▶ model server (yours)
                       │       ▲                      │
                       └── window (WKWebView) ────────┘──▶ docker run … (one container per run)
```

## Installing

1. Download `Cedar-<version>-apple-silicon.dmg` or `Cedar-<version>-intel.dmg` from the
   [releases page](https://github.com/hanneshapke/cedar/releases), open it, and drag Cedar to
   Applications. The app is signed and notarized, so it opens without warnings.
2. Start Docker Desktop (or OrbStack, Colima).
3. Open Cedar. A tree appears in the menu bar and the setup window opens:
   - **Docker**: found and running, or what to do.
   - **Sandbox image**: pulled once per version from `ghcr.io/hanneshapke/cedar-sandbox`.
     Tick *Browser tool* to also pull `cedar-browser` (a few hundred MB more) and offer the
     `browser` tool.
   - **Model server**: pick a preset or type the address, add an API key if the server needs
     one, click *List models*, choose one, and *Save*. The app registers it in cedar's model
     registry (as the entry `default`), which cedar keeps in its database.
4. The window switches to the chat once both servers answer and the model is registered. Close
   it any time; the app keeps running in the menu bar, where *Open Cedar* brings it back and
   *Quit Cedar* stops everything.

*Open in Browser* opens the same chat in your default browser; it asks for an access token once,
which *Copy Access Token* puts on the clipboard. Only the Mac itself can reach the servers: both
listen on 127.0.0.1.

To change the model server later, choose *Model Server & Setup…* from the menu; the new choice
is registered with the running cedar, so nothing restarts. The chat's own Settings page can add
further models to the registry and manages the Brave Search key and the Google account: cedar
runs with `CEDAR_SECRET_KEY`, the chat's key is an admin key, and the app hands the chat page its
admin token, so Settings works without typing anything.

## Where things are

| What | Where |
| --- | --- |
| Settings, including the generated keys | `~/Library/Application Support/io.github.hanneshapke.cedar/config.json` (mode 0600) |
| cedar's database (runs, conversations) | `~/Library/Application Support/io.github.hanneshapke.cedar/cedar.db` |
| cedar-chat's database (the sidebar, schedules) | `…/cedar-chat.db` |
| Logs | `~/Library/Logs/io.github.hanneshapke.cedar/cedar.log` and `cedar-chat.log` (*Open Logs Folder* in the menu) |
| Agent workspaces | Docker volumes named `cedar-ws-*`, removed after `WORKSPACE_TTL_SECS` |

`config.json` is plain JSON and can be edited while the app is not running:

```json
{
  "llm_base_url": "http://127.0.0.1:11434/v1",
  "llm_api_key": "",
  "llm_model": "qwen3:8b",
  "cedar_port": 18080,
  "chat_port": 13000,
  "sandbox_image": "",
  "browser_image": "",
  "browser": false,
  "cedar_env": { "SANDBOX_NETWORK": "bridge", "RUN_TIMEOUT_SECS": "1800" },
  "chat_env": {},
  "secrets": { "cedar_api_key": "…", "cedar_secret_key": "…", "chat_access_token": "…", "chat_admin_token": "…" }
}
```

`cedar_env` and `chat_env` are added to the servers' environment last, so any setting from the
[README](../README.md#configuration) or [chat/README.md](../chat/README.md#configuration) can be
set there. An empty `sandbox_image` or `browser_image` means the published image for this app
version; set them to use an image you built yourself (`docker build -t cedar-sandbox sandbox/`).

The servers always get `DOCKER_BIN` (the CLI the app found), `CEDAR_EXIT_ON_STDIN_EOF=1` (they
exit when the app goes away, so nothing is left behind after a crash), `BIND_ADDR` and
`CHAT_BIND_ADDR` on loopback, and the keys from `secrets`. The model server is not passed as
environment: cedar takes models only through its admin API, which the app calls once cedar is
up (`PUT /v1/admin/models/default`).

## How it is built

`macos/` is a [Tauri 2](https://v2.tauri.app) crate written in Rust only; the setup page is the
static HTML in `ui/`, and the chat page is whatever cedar-chat serves. It is not a member of the
workspace (Tauri needs GTK and WebKit on Linux, which the servers don't), so build it from this
directory. The `cedar` and `cedar-chat` binaries are bundled as sidecars: the bundler expects
them in `binaries/` with the target triple as a suffix.

```sh
# From the repository root. Replace the triple with x86_64-apple-darwin for an Intel build.
T=aarch64-apple-darwin
rustup target add $T
cargo build --release -p cedar -p cedar-chat --target $T
install -m 0755 target/$T/release/cedar macos/binaries/cedar-$T
install -m 0755 target/$T/release/cedar-chat macos/binaries/cedar-chat-$T

cargo install tauri-cli --version '^2' --locked     # once
cd macos
cargo tauri build --target $T --bundles dmg          # target/$T/release/bundle/dmg/Cedar_<version>_*.dmg
```

Without the Apple secrets below the bundler signs ad hoc and skips notarization, which is fine
for your own Mac: open the app once with right-click → Open. `cargo tauri dev` runs the app
against the sidecars in `binaries/` without bundling.

The icons in `icons/` and `tray-icon.png` come from `make-icons.py` (needs Pillow). `Info.plist`
is merged into the bundle's: it declares the app a menu bar app and allows the loopback HTTP the
window uses.

## Releasing

Releases are built by [`.github/workflows/release.yml`](../.github/workflows/release.yml) when a
pull request from a branch named `release/<version>` is merged into `main`:

1. Set the version in two places: `[workspace.package] version` in the root `Cargo.toml` (cedar
   and cedar-chat) and `version` in `macos/Cargo.toml` (the app, and the image tag it pulls).
2. Open a pull request from `release/<version>` to `main` and merge it.
3. The workflow checks the versions match the branch and the tag is free, runs the tests, builds
   and pushes `ghcr.io/hanneshapke/cedar-sandbox:<version>` and `cedar-browser:<version>` for
   amd64 and arm64 (and `:latest`), builds the two DMGs, and creates the GitHub Release `v<version>`
   with them and a `checksums.txt`.

*Run workflow* in the Actions tab builds everything for a version from the current commit as
artifacts, without a tag or release, to try the pipeline. The images are pushed under that
version too.

One-time setup:

- **Apple secrets.** In the repository's Actions secrets: `APPLE_CERTIFICATE` (a Developer ID
  Application certificate exported as .p12 and base64-encoded: `base64 -i cert.p12 | pbcopy`),
  `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` (`Developer ID Application: Name
  (TEAMID)`, as `security find-identity -v -p codesigning` prints it), `APPLE_ID`,
  `APPLE_PASSWORD` (an app-specific password from appleid.apple.com), and `APPLE_TEAM_ID`. The
  dmg job fails up front if any is missing.
- **Package visibility.** The first push creates each GHCR package as private, linked to this
  repository. Make `cedar-sandbox` and `cedar-browser` public once (the package's settings →
  Danger Zone → Change visibility), or users' pulls fail with "denied".
- **arm64 runners.** The images job uses `ubuntu-24.04-arm`, which GitHub provides for public
  repositories. For a private repository, replace the matrix with a single `ubuntu-24.04` runner
  and add `docker/setup-qemu-action` (slower, but it works).

## Troubleshooting

| Symptom | What to check |
| --- | --- |
| "No docker command was found" | Docker Desktop, OrbStack, Colima, or Rancher Desktop isn't installed, or its CLI isn't in `/usr/local/bin`, `/opt/homebrew/bin`, `~/.orbstack/bin`, `~/.rd/bin`, or `/Applications/Docker.app/Contents/Resources/bin`. |
| "Docker is installed but not answering" | The daemon isn't running yet. Start it and click *Check again*. |
| The image download fails with "denied" | The GHCR package is private or this version's images haven't been published yet (see above). You can also build the image yourself and set `sandbox_image` in `config.json`. |
| *List models* fails | The address needs the API path (usually `/v1`). Ollama: `http://127.0.0.1:11434/v1`, LM Studio: `http://127.0.0.1:1234/v1`. A remote server must be reachable from this Mac. |
| "Cedar could not register …" | cedar checks the model server itself before keeping the entry, so the server must be running when you save and when the app starts. Start it and click *Save* again; a model registered earlier keeps working meanwhile. |
| Runs finish instantly with tool calls printed as text | The model or server doesn't do tool calling. Pick a model that does; for vLLM see the [deployment guide](../DEPLOYMENT.md#2-make-sure-vllm-is-ready-for-cedar). |
| "port … is already in use" | Something else listens on 18080 or 13000. Change `cedar_port` or `chat_port` in `config.json`, then *Restart Services*. |
| Cedar stopped with an error | *Open Logs Folder*; `cedar.log` says why. After three quick crashes the app stops retrying until *Restart Services*. |
