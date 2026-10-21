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

1. Download `Cedar-<version>-apple-silicon.dmg` from the
   [releases page](https://github.com/hanneshapke/cedar/releases), open it, and drag Cedar to
   Applications. Releases are built for Apple Silicon only; on an Intel Mac, build it yourself
   (see below). The app is signed and notarized, so it opens without warnings.
2. Start Docker Desktop (or OrbStack, Colima).
3. Open Cedar. A tree appears in the menu bar and the setup window opens:
   - **Docker**: found and running, or what to do.
   - **Sandbox image**: pulled once per version from `ghcr.io/hanneshapke/cedar-sandbox`.
     Tick *Browser tool* to also pull `cedar-browser` (a few hundred MB more) and offer the
     `browser` tool.
   - **Model server**: pick a preset or type the address, add an API key if the server needs
     one, click *List models*, choose one, and *Save*. The app registers it in cedar's model
     registry (as the entry `default`), which cedar keeps in its database.
4. The window switches to the chat once both servers answer and the model is registered. While
   the window is open Cedar has a Dock icon and shows up in Command-Tab like any app. Close the
   window and it is a menu bar item again, with the servers still running; *Open Cedar* brings the
   window back, and *Quit Cedar* (or Command-Q while the window is up) stops everything.

*Open in Browser* opens the same chat in your default browser; it asks for an access token once,
which *Copy Access Token* puts on the clipboard. Only the Mac itself can reach the servers: both
listen on 127.0.0.1. To use the chat from the iOS app, share it over Tailscale (below).

To change the model server later, choose *Model Server & Setup…* from the menu; the new choice
is registered with the running cedar, so nothing restarts. The chat's own Settings page can add
further models to the registry and manages the Brave Search key and the Google account: cedar
runs with `CEDAR_SECRET_KEY`, the chat's key is an admin key, and the app hands the chat page its
admin token, so Settings works without typing anything.

## Using it from an iPhone or iPad

The [iOS app](../iOS/README.md) connects over HTTPS only, so the setup page's **iPhone & iPad**
section (*iPhone & iPad Access…* in the menu) can serve the chat on your
[Tailscale](https://tailscale.com) network. Tick *Share with the iOS app over Tailscale* and
the app runs `tailscale serve --bg --https=443 http://127.0.0.1:<chat_port>`. Tailscale gets a
Let's Encrypt certificate for the Mac's tailnet name and proxies
`https://<mac>.<tailnet>.ts.net` to the chat. Only devices on your tailnet can reach it, and both
servers stay on loopback.

1. Install Tailscale on the Mac (from [tailscale.com](https://tailscale.com/download/mac) or the
   Mac App Store) and log in. The app finds the CLI that Tailscale's *Install CLI* puts in
   `/usr/local/bin`, Homebrew's, or the binary inside `/Applications/Tailscale.app`.
2. In the [admin console's DNS page](https://login.tailscale.com/admin/dns), turn on **MagicDNS**
   and **HTTPS Certificates**. Without them, the setup page explains what is missing and does not
   start sharing.
3. Tick the checkbox. Cedar restarts once, so the chat is started with the tailnet address as
   `CHAT_PUBLIC_URL` (for the links in notifications and *Connect Google*). A response still
   running when it restarts is stopped.
4. Install Tailscale on the iPhone and log in to the same tailnet. In the Cedar app's Settings,
   tap **Scan QR Code** and point the camera at the code the setup page shows after *Show QR
   code*; the app fills in the address and token and tests the connection. Tap **Save**. Or type
   them in: *Copy address* and *Copy access token* put them on the clipboard.

The QR code holds `cedar://connect?server=<address>&token=<access token>` (made in
`src/connect.rs`; the iOS app reads it in `ConnectionCode.swift`). It is drawn only when asked
and hidden again with *Hide QR code*, since anyone who scans it can use your Cedar.

The setting is saved, so sharing comes back on every time Cedar starts. Cedar turns the serve
off when it quits, and turns it off for good when you untick the checkbox. The phone can reach
the chat only while the Mac is awake and Cedar is running. Anyone on your tailnet can open the
page, so the access token is still what protects it;
[tailnet ACLs](https://tailscale.com/kb/1018/acls) can narrow who gets through. To use another
tailnet port than 443 (when this Mac already serves something there), set
`tailscale_https_port` in `config.json`.

With the address set as `CHAT_PUBLIC_URL`, *Connect Google* sends Google back to
`https://<mac>.<tailnet>.ts.net/api/oauth/google/callback`. Add that redirect URI to your Google
OAuth client before you connect, or set your own `CHAT_PUBLIC_URL` in `chat_env`, which wins.

For push notifications on the phone, set up **iPhone push** in the chat's Settings →
Notifications (see [iOS/README.md](../iOS/README.md#push-notifications)) and choose it as the
default method; the app chooses *Mac notification* on its first start.

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
  "tailscale_share": false,
  "tailscale_https_port": 443,
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

`desktop/` is a [Tauri 2](https://v2.tauri.app) crate written in Rust only; the setup page is the
static HTML in `ui/`, and the chat page is whatever cedar-chat serves. It is not a member of the
workspace (Tauri needs GTK and WebKit on Linux, which the servers don't), so build it from this
directory. The `cedar` and `cedar-chat` binaries are bundled as sidecars: the bundler expects
them in `binaries/` with the target triple as a suffix.

```sh
# From the repository root. Replace the triple with x86_64-apple-darwin for an Intel build.
T=aarch64-apple-darwin
rustup target add $T
cargo build --release -p cedar -p cedar-chat --target $T
install -m 0755 target/$T/release/cedar desktop/binaries/cedar-$T
install -m 0755 target/$T/release/cedar-chat desktop/binaries/cedar-chat-$T

cargo install tauri-cli --version '^2' --locked     # once
cd desktop
cargo tauri build --target $T --bundles dmg          # target/$T/release/bundle/dmg/Cedar_<version>_*.dmg
```

Without the Apple secrets below the bundler signs ad hoc and skips notarization, which is fine
for your own Mac: open the app once with right-click → Open. `cargo tauri dev` runs the app
against the sidecars in `binaries/` without bundling.

The icons in `icons/`, `app-icon.png`, and `tray-icon.png` are rendered by `make-icons.py` (needs
Pillow) from the brand assets in `docs/assets/`: `cedar-app-icon.svg` with the margin a macOS icon
carries itself, and `cedar-mark-mono.svg` as a black-on-transparent template image for the menu
bar. Rerun it after the assets change. `Info.plist`
is merged into the bundle's: it declares the app a menu bar app and allows the loopback HTTP the
window uses.

## Releasing

Releases are built by [`.github/workflows/release.yml`](../.github/workflows/release.yml) when a
pull request from a branch named `release/<version>` is merged into `main`:

1. Set the version in two places: `[workspace.package] version` in the root `Cargo.toml` (cedar
   and cedar-chat) and `version` in `desktop/Cargo.toml` (the app, and the image tag it pulls).
2. Open a pull request from `release/<version>` to `main` and merge it.
3. The workflow checks the versions match the branch and the tag is free, runs the tests, builds
   and pushes `ghcr.io/hanneshapke/cedar-sandbox:<version>` and `cedar-browser:<version>` for
   amd64 and arm64 (and `:latest`), builds the Apple Silicon DMG, and creates the GitHub Release
   `v<version>` with it and a `checksums.txt`.

*Run workflow* in the Actions tab builds everything for a version from the current commit as
artifacts, without a tag or release, to try the pipeline. The images are pushed under that
version too.

One-time setup:

- **Apple secrets.** In the repository's Actions secrets: `APPLE_CERTIFICATE` (a Developer ID
  Application certificate exported as .p12 and base64-encoded: `base64 -i cert.p12 | pbcopy`),
  `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` (`Developer ID Application: Name
  (TEAMID)`, as `security find-identity -v -p codesigning` prints it), and for notarization the
  App Store Connect API key the iOS TestFlight upload uses: `ASC_KEY_ID`, `ASC_ISSUER_ID`, and
  `ASC_KEY_P8` (see [the iOS README](../iOS/README.md#testflight)). The dmg job fails up front if
  any is missing.
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
| No notification when a response finishes | Settings → Notifications in the chat should list *Mac notification* as set up and the default (the app sets that up on its first start, unless another method was already chosen). Then allow notifications for Cedar in System Settings → Notifications. A response you looked at within a few seconds of it ending isn't announced. |
| iPhone & iPad: "Tailscale is not installed" | Install it from tailscale.com or the Mac App Store. Running it from the menu bar is not enough if the CLI is somewhere else: choose *Install CLI* in Tailscale's settings to put `tailscale` in `/usr/local/bin`. |
| iPhone & iPad: "HTTPS is not enabled" or "no tailnet name" | Turn on HTTPS Certificates and MagicDNS in the Tailscale admin console's DNS page, then click *Try again*. |
| The iOS app can't connect | Use the `https://…ts.net` address from the setup page, without a port; check that the phone is logged in to the same tailnet, that the Mac is awake, and that `tailscale serve status` on the Mac lists the chat port. The first connection can take a few seconds while Tailscale gets the certificate. |
| Cedar stopped with an error | *Open Logs Folder*; `cedar.log` says why. After three quick crashes the app stops retrying until *Restart Services*. |
