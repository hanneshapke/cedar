# Cedar for macOS and Windows

A menu bar app (a tray app on Windows) that runs cedar and cedar-chat from a single download: a
DMG for the Mac, an MSI installer for Windows. It bundles both server binaries, starts them as
child processes, pulls the sandbox image, and shows the chat UI in its window. Two things stay
outside the app:

- **Docker.** Each agent run's tools execute in a container, so Docker must be installed and
  running: Docker Desktop, OrbStack, Colima, or Rancher Desktop on a Mac; Docker Desktop (with
  its WSL 2 backend) or Rancher Desktop on Windows. The sandbox is a Linux image either way. The
  app finds the `docker` command in the usual places and tells you when it can't.
- **The model server.** vLLM doesn't run on macOS or Windows, so the app asks for the address of
  any OpenAI-compatible server with tool calling: Ollama or LM Studio on the same computer, or a
  vLLM on another machine (a DGX Spark over Tailscale, say). It lists the server's models and you
  pick one.

```
Cedar (menu bar / tray) ──┬── cedar-chat :13000 ──▶ cedar :18080 ──▶ model server (yours)
                          │       ▲                      │
                          └── window (WKWebView / WebView2) ──▶ docker run … (one container per run)
```

## Installing

### macOS

1. Download `Cedar-<version>-apple-silicon.dmg` from the
   [releases page](https://github.com/hanneshapke/cedar/releases), open it, and drag Cedar to
   Applications. Releases are built for Apple Silicon only; on an Intel Mac, build it yourself
   (see below). The app is signed and notarized, so it opens without warnings.
2. Start Docker Desktop (or OrbStack, Colima).
3. Open Cedar. A tree appears in the menu bar and the setup window opens (see
   [Setting it up](#setting-it-up)).
4. While the window is open Cedar has a Dock icon and shows up in Command-Tab like any app. Close
   the window and it is a menu bar item again, with the servers still running; *Open Cedar* brings
   the window back, and *Quit Cedar* (or Command-Q while the window is up) stops everything.

### Windows

1. Install [Docker Desktop](https://www.docker.com/products/docker-desktop/) with the WSL 2
   backend (its default), or Rancher Desktop, and start it. Both need administrator rights to
   install, and so does Cedar's MSI.
2. Download `Cedar-<version>-x64.msi` from the
   [releases page](https://github.com/hanneshapke/cedar/releases) and run it. It installs for all
   users of the PC into `C:\Program Files\Cedar` and adds Cedar to the Start Menu. The installer
   is not code-signed yet, so Windows SmartScreen warns the first time ("Windows protected your
   PC"): choose *More info*, then *Run anyway*. Releases are built for x64; Windows on Arm runs
   them emulated.
3. Open Cedar from the Start Menu. Its icon appears in the tray (the notification area at the
   right of the taskbar; it may be under the ^ arrow) and the setup window opens (see
   [Setting it up](#setting-it-up)).
4. While the window is open Cedar has a taskbar button. Close the window and Cedar keeps running
   in the tray, with the servers still up. Click the tray icon to bring the window back;
   right-click it for the menu, where *Quit Cedar* stops everything. Starting Cedar again from the
   Start Menu shows the running one.

A newer MSI installs over the older one and keeps your settings and conversations; they live in
your user profile (see [Where things are](#where-things-are)), which neither an upgrade nor
uninstalling touches.

### Setting it up

The setup window shows three steps:

- **Docker**: found and running, or what to do.
- **Sandbox image**: pulled once per version from `ghcr.io/hanneshapke/cedar-sandbox`. Tick
  *Browser tool* to also pull `cedar-browser` (a few hundred MB more) and offer the `browser`
  tool.
- **Model server**: pick a preset or type the address, add an API key if the server needs one,
  click *List models*, choose one, and *Save*. The app registers it in cedar's model registry (as
  the entry `default`), which cedar keeps in its database.

The window switches to the chat once both servers answer and the model is registered.

*Open in Browser* opens the same chat in your default browser; it asks for an access token once,
which *Copy Access Token* puts on the clipboard. Only this computer can reach the servers: both
listen on 127.0.0.1. To use the chat from the iOS app, share it over Tailscale (below).

To change the model server later, choose *Model Server & Setup…* from the menu; the new choice
is registered with the running cedar, so nothing restarts. The chat's own Settings page can add
further models to the registry and manages the Brave Search key and the Google account: cedar
runs with `CEDAR_SECRET_KEY`, the chat's key is an admin key, and the app hands the chat page its
admin token, so Settings works without typing anything.

When a response finishes, the app can tell you with a **desktop notification**: in Notification
Center on a Mac, as a toast on Windows. Clicking it opens the conversation. The app sets this
method up in the chat's Settings → Notifications on its first start, unless another was chosen.

## Using it from an iPhone or iPad

The [iOS app](../iOS/README.md) connects over HTTPS only, so the setup page's **iPhone & iPad**
section (*iPhone & iPad Access…* in the menu) can serve the chat on your
[Tailscale](https://tailscale.com) network. Tick *Share with the iOS app over Tailscale* and
the app runs `tailscale serve --bg --https=443 http://127.0.0.1:<chat_port>`. Tailscale gets a
Let's Encrypt certificate for the computer's tailnet name and proxies
`https://<computer>.<tailnet>.ts.net` to the chat. Only devices on your tailnet can reach it, and
both servers stay on loopback.

1. Install Tailscale on the computer and log in.
   - **macOS**: from [tailscale.com](https://tailscale.com/download/mac) or the Mac App Store.
     The app finds the CLI that Tailscale's *Install CLI* puts in `/usr/local/bin`, Homebrew's,
     or the binary inside `/Applications/Tailscale.app`.
   - **Windows**: from [tailscale.com](https://tailscale.com/download/windows). The app finds
     `tailscale.exe` in `C:\Program Files\Tailscale` or on `PATH`.
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
`../pairing/src/connect.rs`, shared with the chat; the iOS app reads it in `ConnectionCode.swift`).
Once sharing is on, the chat's own **Settings → iPhone** page shows the same code, the address
and token, and what Tailscale still needs, so it can be paired from the chat window too. It is drawn only when asked
and hidden again with *Hide QR code*, since anyone who scans it can use your Cedar.

The setting is saved, so sharing comes back on every time Cedar starts. Cedar turns the serve
off when it quits, and turns it off for good when you untick the checkbox. The phone can reach
the chat only while the computer is awake and Cedar is running. Anyone on your tailnet can open
the page, so the access token is still what protects it;
[tailnet ACLs](https://tailscale.com/kb/1018/acls) can narrow who gets through. To use another
tailnet port than 443 (when this computer already serves something there), set
`tailscale_https_port` in `config.json`.

With the address set as `CHAT_PUBLIC_URL`, *Connect Google* sends Google back to
`https://<computer>.<tailnet>.ts.net/api/oauth/google/callback`. Add that redirect URI to your
Google OAuth client before you connect, or set your own `CHAT_PUBLIC_URL` in `chat_env`, which
wins.

For push notifications on the phone, set up **iPhone push** in the chat's Settings →
Notifications (see [iOS/README.md](../iOS/README.md#push-notifications)) and choose it as the
default method; the app chooses *Desktop notification* on its first start.

## Where things are

Tauri derives both folders from the bundle identifier, `io.github.hanneshapke.cedar`.

### macOS

| What | Where |
| --- | --- |
| Settings, including the generated keys | `~/Library/Application Support/io.github.hanneshapke.cedar/config.json` (mode 0600) |
| cedar's database (runs, conversations) | `~/Library/Application Support/io.github.hanneshapke.cedar/cedar.db` |
| cedar-chat's database (the sidebar, schedules) | `…/cedar-chat.db` |
| Logs | `~/Library/Logs/io.github.hanneshapke.cedar/cedar.log` and `cedar-chat.log` (*Open Logs Folder* in the menu) |
| The app and the two servers | `/Applications/Cedar.app` (the servers are in `Contents/MacOS/`) |
| Agent workspaces | Docker volumes named `cedar-ws-*`, removed after `WORKSPACE_TTL_SECS` |

### Windows

| What | Where |
| --- | --- |
| Settings, including the generated keys | `%APPDATA%\io.github.hanneshapke.cedar\config.json` |
| cedar's database (runs, conversations) | `%APPDATA%\io.github.hanneshapke.cedar\cedar.db` |
| cedar-chat's database (the sidebar, schedules) | `%APPDATA%\io.github.hanneshapke.cedar\cedar-chat.db` |
| Logs | `%LOCALAPPDATA%\io.github.hanneshapke.cedar\logs\cedar.log` and `cedar-chat.log` (*Open Logs Folder* in the menu) |
| The app and the two servers | `C:\Program Files\Cedar\` (`Cedar.exe`, `cedar.exe`, `cedar-chat.exe`) |
| Agent workspaces | Docker volumes named `cedar-ws-*`, removed after `WORKSPACE_TTL_SECS` |

`%APPDATA%` is your own profile folder (`C:\Users\<you>\AppData\Roaming`), which other
non-administrator users of the PC can't read. That is what protects the keys in `config.json` on
Windows; the app sets no extra ACL on the file, unlike the 0600 mode it gives it on a Mac.

### config.json

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
exit when the app goes away, so nothing is left behind after a crash or when Cedar is ended from
Task Manager), `BIND_ADDR` and `CHAT_BIND_ADDR` on loopback, and the keys from `secrets`. The model
server is not passed as environment: cedar takes models only through its admin API, which the app
calls once cedar is up (`PUT /v1/admin/models/default`).

## How it is built

`desktop/` is a [Tauri 2](https://v2.tauri.app) crate written in Rust only; the setup page is the
static HTML in `ui/`, and the chat page is whatever cedar-chat serves. It is not a member of the
workspace (Tauri needs GTK and WebKit on Linux, which the servers don't), so build it from this
directory. The `cedar` and `cedar-chat` binaries are bundled as sidecars: the bundler expects
them in `binaries/` with the target triple as a suffix (and `.exe` on Windows).

`tauri.conf.json` holds the settings for both platforms; `tauri.windows.conf.json` is merged over
it on Windows (an MSI instead of a DMG, the WebView2 installer bundled for PCs that lack it, and
the installer's upgrade code). The upgrade code is what makes a newer MSI replace an older one,
so it must never change.

### On a Mac

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

### On Windows

Needs the [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
(*Desktop development with C++*) and [Strawberry Perl](https://strawberryperl.com/), which builds
the OpenSSL that cedar-chat links statically (Git's own Perl can't). In PowerShell:

```powershell
# From the repository root.
$T = "x86_64-pc-windows-msvc"
# The C runtime linked in, so the servers run on PCs without the Visual C++ Redistributable.
$env:RUSTFLAGS = "-C target-feature=+crt-static"
cargo build --release -p cedar -p cedar-chat --target $T
Remove-Item Env:RUSTFLAGS
Copy-Item target/$T/release/cedar.exe desktop/binaries/cedar-$T.exe
Copy-Item target/$T/release/cedar-chat.exe desktop/binaries/cedar-chat-$T.exe

cargo install tauri-cli --version '^2' --locked     # once
cd desktop
cargo tauri build --target $T --bundles msi          # target/$T/release/bundle/msi/Cedar_<version>_x64_en-US.msi
```

The bundler downloads the WiX toolset it builds the MSI with on first use. `cargo tauri dev` runs
the app without an installer, but Windows shows toast notifications only for an app installed
with a Start Menu shortcut, so test those with the MSI.

### Icons

The icons in `icons/`, `app-icon.png`, `tray-icon.png`, and `tray-icon-color.png` are rendered
by `make-icons.py` (needs Pillow) from the brand assets in `docs/assets/`: `cedar-app-icon.svg`
with the margin a macOS icon carries itself (and full bleed for `icon.ico` and the Windows tray
icon), and `cedar-mark-mono.svg` as a black-on-transparent template image for the Mac's menu bar.
Rerun it after the assets change. `Info.plist` is merged into the Mac bundle's: it declares the
app a menu bar app and allows the loopback HTTP the window uses.

## Releasing

Releases are built by [`.github/workflows/release.yml`](../.github/workflows/release.yml) when a
pull request from a branch named `release/<version>` is merged into `main`:

1. Set the version in two places: `[workspace.package] version` in the root `Cargo.toml` (cedar
   and cedar-chat) and `version` in `desktop/Cargo.toml` (the app, and the image tag it pulls).
   It must be a plain `major.minor.patch` with major and minor up to 255 and patch up to 65535,
   with no pre-release suffix like `-beta`: Windows Installer takes nothing else, and the workflow
   refuses other versions up front.
2. Open a pull request from `release/<version>` to `main` and merge it.
3. The workflow checks the versions match the branch and the tag is free, runs the tests, builds
   and pushes `ghcr.io/hanneshapke/cedar-sandbox:<version>` and `cedar-browser:<version>` for
   amd64 and arm64 (and `:latest`), builds the Apple Silicon DMG and the x64 MSI, and creates
   the GitHub Release `v<version>` with them and a `checksums.txt`.

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
- **Windows signing.** Not set up yet: the MSI and `Cedar.exe` ship unsigned, so SmartScreen
  warns on first run (see [Installing](#windows)). Signing needs an Authenticode certificate or
  Azure Trusted Signing; Tauri signs during `cargo tauri build` once `bundle.windows` in
  `tauri.windows.conf.json` names one.
- **Package visibility.** The first push creates each GHCR package as private, linked to this
  repository. Make `cedar-sandbox` and `cedar-browser` public once (the package's settings →
  Danger Zone → Change visibility), or users' pulls fail with "denied".
- **arm64 runners.** The images job uses `ubuntu-24.04-arm`, which GitHub provides for public
  repositories. For a private repository, replace the matrix with a single `ubuntu-24.04` runner
  and add `docker/setup-qemu-action` (slower, but it works).

## Troubleshooting

### On either platform

| Symptom | What to check |
| --- | --- |
| "Docker is installed but not answering" | The daemon isn't running yet. Start it and click *Check again*. |
| The image download fails with "denied" | The GHCR package is private or this version's images haven't been published yet (see above). You can also build the image yourself and set `sandbox_image` in `config.json`. |
| *List models* fails | The address needs the API path (usually `/v1`). Ollama: `http://127.0.0.1:11434/v1`, LM Studio: `http://127.0.0.1:1234/v1`. A remote server must be reachable from this computer. |
| "Cedar could not register …" | cedar checks the model server itself before keeping the entry, so the server must be running when you save and when the app starts. Start it and click *Save* again; a model registered earlier keeps working meanwhile. |
| Runs finish instantly with tool calls printed as text | The model or server doesn't do tool calling. Pick a model that does; for vLLM see the [deployment guide](../DEPLOYMENT.md#2-make-sure-vllm-is-ready-for-cedar). |
| "port … is already in use" | Something else listens on 18080 or 13000. Change `cedar_port` or `chat_port` in `config.json`, then *Restart Services*. |
| No notification when a response finishes | Settings → Notifications in the chat should list *Desktop notification* as set up and the default (the app sets that up on its first start, unless another method was already chosen). A response you looked at within a few seconds of it ending isn't announced. |
| iPhone & iPad: "HTTPS is not enabled" or "no tailnet name" | Turn on HTTPS Certificates and MagicDNS in the Tailscale admin console's DNS page, then click *Try again*. |
| The iOS app can't connect | Use the `https://…ts.net` address from the setup page, without a port; check that the phone is logged in to the same tailnet, that the computer is awake, and that `tailscale serve status` on it lists the chat port. The first connection can take a few seconds while Tailscale gets the certificate. |
| Cedar stopped with an error | *Open Logs Folder*; `cedar.log` says why. After three quick crashes the app stops retrying until *Restart Services*. |

### macOS

| Symptom | What to check |
| --- | --- |
| "No docker command was found" | Docker Desktop, OrbStack, Colima, or Rancher Desktop isn't installed, or its CLI isn't in `/usr/local/bin`, `/opt/homebrew/bin`, `~/.orbstack/bin`, `~/.rd/bin`, or `/Applications/Docker.app/Contents/Resources/bin`. |
| No notification appears | Allow notifications for Cedar in System Settings → Notifications. |
| iPhone & iPad: "Tailscale is not installed" | Install it from tailscale.com or the Mac App Store. Running it from the menu bar is not enough if the CLI is somewhere else: choose *Install CLI* in Tailscale's settings to put `tailscale` in `/usr/local/bin`. |

### Windows

| Symptom | What to check |
| --- | --- |
| "Windows protected your PC" when installing | The MSI isn't code-signed yet. Choose *More info*, then *Run anyway*. |
| "No docker command was found" | Docker Desktop or Rancher Desktop isn't installed, or its CLI isn't in `C:\Program Files\Docker\Docker\resources\bin`, Rancher Desktop's `resources\resources\win32\bin`, or on `PATH`. If you installed Docker while Cedar was running, quit Cedar and start it again so it sees the new `PATH`. |
| The sandbox image won't start, or Docker says "image operating system "linux" cannot be used on this platform" | Docker Desktop is set to Windows containers. Right-click its tray icon and choose *Switch to Linux containers*; the WSL 2 backend is the default. |
| No notification appears | Allow notifications for Cedar in Settings → System → Notifications, and check that Do Not Disturb is off. Toasts only show for the installed app (from the MSI), not for `cargo tauri dev`. |
| Cedar's tray icon is missing | Windows may hide it under the ^ arrow in the notification area; drag it onto the taskbar, or turn it on in Settings → Personalization → Taskbar → Other system tray icons. |
| iPhone & iPad: "Tailscale is not installed" | Install it from tailscale.com/download/windows; the app looks for `C:\Program Files\Tailscale\tailscale.exe`. |
| `cedar.exe` or `cedar-chat.exe` still running after Cedar is gone | They stop within a few seconds once Cedar ends, however it ends; if one stays, its log says why. End it in Task Manager and start Cedar again. |
