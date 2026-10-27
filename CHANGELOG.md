# Changelog

## Unreleased

- Registry: `drug_prices` 1.0.1. A drug no open source lists (NADAC has no pharmacy cost for it and Cost Plus Drugs doesn't sell it) now gets an answer with coupon links instead of the error *no source lists this drug. Try again in a little while*, which made the model retry for nothing. A Cost Plus quote whose `brand_name` names the reference brand of a generic is no longer dropped when RxNorm has a generic.

## 0.1.10

- **Install tools from the registry in the Mac and Windows app.** The app now runs cedar with personal tools in `~/.cedar/tools` and the [community tool registry](registry/README.md). Under the tool list in Settings → Tools, *Tool registry* links to the registry on GitHub and lists its tools, with what each one asks for; *Install* copies a tool's files into `~/.cedar/tools/<name>`, checking each against the hash the registry pins, and cedar loads it right away, with no restart. Fix: the *Install* button works again; it read the clicked button only after its confirmation, when the event no longer had it (#101).
- Release: `scripts/bump-version.sh` sets the version everywhere a release checks it (both `Cargo.toml`s and lock files, the iOS app's `MARKETING_VERSION` and the landing site's `docs/landing/version.js`), and `scripts/check-version.sh` checks them. A pull request from a `release/<version>` branch fails (`release-check.yml`) until they all match (#104).
- Website: the landing page and install guide show the version from `docs/landing/version.js`, and the site is deployed with a new release only once its GitHub Release is published, so it never offers downloads that aren't there yet (#104, #105).

## 0.1.9

- **Cedar for macOS sets Docker up itself**, like the Windows app. When no Docker is found, the setup page offers *Set up Docker for me*: it checks that the Mac can run virtual machines, downloads Docker Desktop for its chip, and after one administrator password checks that the app is signed by Docker and installs it with Docker's own `install --accept-license`, so its first start asks for nothing. Cedar then starts it and carries on. A Docker that isn't answering is now started on a Mac too: Docker Desktop, OrbStack or Rancher Desktop, whichever the `docker` command belongs to (#96).
- **Cedar installs Tailscale too**, on macOS and Windows. When Tailscale isn't installed, the phone-sharing section offers *Set up Tailscale for me*: it downloads Tailscale's own installer (the standalone Mac app, or the MSI for the PC's processor), checks that it is signed by Tailscale, installs it after the same one permission prompt, and opens it. The page then lists what is left: logging in, and turning on MagicDNS and HTTPS Certificates (#96).
- **`scripts/cedar-stack.sh` offers to install Docker and Tailscale.** Before it starts, it checks for Docker (CLI, a daemon that answers, permission to use it, and the compose plugin when it runs vLLM) and, unless `TAILSCALE_SERVE=off`, Tailscale logged in. On Linux it offers to fix each one: install Docker Engine with get.docker.com, start it, add you to the `docker` group (and carry on in it with `sg docker`), install Tailscale with tailscale.com/install.sh, and `tailscale up`. Every command is shown, and it runs only when you answer yes; `INSTALL_DEPS=yes|no` in `.env` answers for you, and without a terminal nothing is installed. On a Mac it says what to install. `--pair` runs the Tailscale check too (it needs Tailscale unless `CHAT_PUBLIC_URL` is HTTPS); `--down` skips both (#96).
- **A model guide.** The website's new model page (`docs/landing/models.html`) suggests models for Cedar from a few answers: text or text and images, on this computer, another machine of yours or a token factory, and the hardware (M4/M5 Mac, AMD Ryzen AI Max, DGX Spark) with its memory. It names a top pick and alternatives (Qwen3.6, Qwen3.5 122B, Gemma 4, gpt-oss, Mistral Small 4, DeepSeek V4 Flash; DeepSeek V4.1 Flash, GLM 5.3 and Kimi K2.6 at Doubleword or Baseten), how to get each, and how to set it up in Cedar (#96).
- Website: the install guide lets Cedar set up Docker on a Mac and Tailscale on a Mac or PC, and no longer asks for Tailscale's *Install CLI*, which Cedar never needed. The landing page's setup steps follow the new order (pick a model, install Cedar, let Cedar set up the rest), and its *What's new* section is gone (#96).
- [PRIVACY.md](PRIVACY.md) says what the Docker and Tailscale setups download, and from where.
- **The iPhone app without TestFlight.** Each release also builds the iOS app as an unsigned IPA, attached to the GitHub Release and listed in a SideStore source (`docs/sidestore`), so it can be sideloaded with SideStore or AltStore (#98). The landing and install pages offer both ways to install it: TestFlight or privately (#99).
- **Registry: `drug_prices`**, the cheapest US price for a prescription: generic vs brand, Cost Plus Drugs, the NADAC pharmacy cost and your copay (#100).
- Release: the Mac and Windows builds sign their update bundles again; the Tauri CLI now gets the updater's public key it checks them against. 0.1.8 never got published because of this, so its changes ship with 0.1.9 (#102).

## 0.1.8

- **Update banner.** When a newer release is out, the chat shows a banner with its release notes and an *Update* button. The Mac and Windows app checks the release's signed `latest.json` every six hours (tauri-plugin-updater) and installs the update itself: it downloads the new app or MSI, checks its signature against the key built into the app, stops the servers and restarts on the new version; the tray menu offers it too, and the setup page has an *Updates* section to check now or turn the checks off. On Linux the chat looks up the latest GitHub Release (`GET /api/update`; `CHAT_UPDATE_CHECK=off` turns it off) and, with the new root `cedar-update` unit set up (DEPLOYMENT.md, "Updates"), an admin's *Update* installs it: the tarball is checked against `checksums.txt`, both binaries are replaced and restarted, and the old ones come back if cedar doesn't start. Without it the banner shows the commands. `cedar --version` and `cedar-chat --version` print the version. (#94)
- **An install guide that picks the steps for you.** The website's install page (`docs/landing/install.html`) asks which computer you have, whether Docker is already there, where the model runs and whether you want the phone app, and shows only the steps for that setup, ending with the downloads (#88–#91).
- **Privacy policy.** [PRIVACY.md](PRIVACY.md) spells out that Cedar collects no data: no account, no analytics or telemetry, and no Cedar cloud your data passes through. It also says what the new update check sends to GitHub (#93).

## 0.1.7

- **Sandboxes are paused when a conversation goes idle.** A completed run now keeps its sandbox container for the conversation's next turn instead of removing it, so processes it started and packages it installed outside `/workspace` are still there. After `sandbox_idle_minutes` without a new turn (10 by default, 0–1440) the container is paused with `docker pause`, and the next message resumes it. Set it under Settings → Server → *Pause when idle*, or with `PUT /v1/admin/settings`. Kept containers are removed with their workspace and when cedar stops. `PUT /v1/admin/settings` now leaves out fields unchanged instead of resetting them (#85).
- **Cedar decides when to plan.** A new plan mode, `tool` (*When Cedar thinks it helps* in Settings → Plans), offers the model a `propose_plan` tool instead of planning every longer prompt: Cedar proposes steps only for requests that take several steps or tools, or that would be costly to get wrong, and waits for your go-ahead partway through the response, where you can still change them. Such a plan is marked as Cedar's in the chat, the desktop apps and the iOS and Android apps, and the phone apps get a Plans setting of their own for the messages you send from them (#84).
- **A personal assistant by default.** The default system prompt (used when `SYSTEM_PROMPT` isn't set) now frames Cedar as a personal AI assistant instead of a coding agent: it answers first, asks clarifying questions when the answer would change what it does, writes a short plan before complex tasks and carries it out (following one already agreed with `PLAN`), checks before anything it can't undo, and keeps your information private (#83).

## 0.1.6

- Mac and Windows app: links the chat opens in a new tab (an answer's sources, map attributions, recipe credits) now open in the default browser instead of doing nothing (#81).

## 0.1.5

- **Cedar for Windows sets Docker up itself.** When no Docker is found, the setup page offers *Set up Docker for me*: it checks the PC (Windows build, virtualization in the firmware), downloads Docker Desktop, and after one permission prompt turns on WSL 2, installs Docker Desktop with the WSL 2 backend, and lets your account use it. When a restart or sign-out is needed, Cedar offers it and opens again afterwards to finish. A Docker Desktop that isn't running is now started by Cedar (#79).
- Landing page: Linux downloads, and the Android app next to iOS (#78).
- Fix: a port race in the webhook tests that failed the 0.1.4 release (#77).

## 0.1.4

- **Every platform in one release.** Each release now also publishes Linux tarballs of `cedar` and `cedar-chat` for x64 and arm64 (`Cedar-<version>-linux-<arch>.tar.gz`, glibc 2.35 and newer), which replace building them for the systemd setup in DEPLOYMENT.md, and uploads the iOS app to TestFlight under the same version.
- **Cedar for Android.** A native Kotlin/Compose port of the iOS app in `android/`: Chats, Starred and Scheduled, streamed turns with tool calls, plans, approvals and questions, attachments, agent images, cards (maps on OpenStreetMap, charts, recipes, routes, stocks, products), and QR pairing. Releases attach a signed `Cedar-<version>-android.apk`; `android/build-apk.sh` builds one yourself (#60).
- **Android push.** A new *Android push* notification method in the chat, through Firebase Cloud Messaging or UnifiedPush (ntfy, without Google). Pushes carry only ids; the app fetches the text from your server (#74).
- Settings → iPhone is now Settings → Phone, with the steps for Android too.
- Mac and Windows app: links on the setup page open in the default browser (#75).
- iOS: the app's version follows the release (0.1.4), and TestFlight build numbers are UTC timestamps, so manual and release uploads never collide.

## 0.1.3

- **Starred conversations.** Star a chat or a scheduled run from its header; the starred ones are listed under Starred at the top of the sidebar. Stars are kept by the chat server, so the list is the same on the web, in the Mac and Windows app and in the iOS app (#72).
- **Settings → iPhone.** The chat's HTTPS address, access token and the QR code the iOS app scans, with a check of what Tailscale on this computer still needs (#71).
- iOS: switch the sidebar between Chats, Starred and Scheduled; scheduled agents list their runs, with Run now, Pause and Resume (#69, #72). Fix: opening a scheduled run no longer shows an empty chat marked "cancelled" (#69).
- Landing page: Cedar for Windows, download buttons that follow the visitor's OS (#66), and a Cedar vs. comparison picker (#68).

## 0.1.2

- **Fix: Cedar for Windows starts its servers on PCs without the Visual C++ Redistributable.** In 0.1.1, cedar.exe and cedar-chat.exe loaded `VCRUNTIME140.dll`, which Windows doesn't ship, so they failed with "VCRUNTIME140.dll was not found". The C runtime is now linked in, and the release checks that no executable needs the DLL.
- **Fix: the Windows window no longer goes blank.** Opening Cedar from the tray (or starting it again) while the servers weren't running navigated the window to `about:blank` instead of the setup page.

## 0.1.1

- **Fix: the Mac app's chat now starts.** In 0.1.0, cedar-chat loaded OpenSSL from Homebrew, which macOS refused (or which wasn't there), so the chat didn't come up. OpenSSL is now built into it (#63).
- **Cedar for Windows:** a tray app with an MSI installer for x64, alongside the Mac app. It is not code-signed yet, so SmartScreen warns on first run (#62).
- The app's source moved from `macos/` to `desktop/` (#61).
- iOS: attach photos and PDFs to a message (#35), and answer the agent's clarifying questions in a form (#53).
- Linux: pair the iOS app by QR code from `cedar-stack.sh --pair` (#54).
- Registry: new tools `dep_audit`, `arxiv_search` and `air_quality` (#57).

## 0.1.0

First release: the Cedar Mac app (Apple Silicon), the sandbox, browser images, Python tools, iOS app connection.
