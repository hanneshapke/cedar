# Security review of cedar

_Reviewed: 31 October 2026, at commit `d590458` (version 0.3.4). Static review of the source tree only; nothing was run against a live deployment, and nothing in the repository was changed._

This document records the findings of a code-level security review of the cedar server, the cedar-chat server and its web UI, the agent tools, the Docker and OpenShell sandboxing, the desktop app, the mobile apps, the deployment scripts and the release pipeline. For each finding it suggests a remedy. **No remedy has been implemented**; this is input for prioritisation.

## Summary

cedar is in good shape for a project of its kind. The core server's authorization, SQL handling, secret encryption, webhook SSRF guard and workspace-path handling are implemented carefully and covered by tests, and the container flags match what the README's Safety section claims. The review found **no Critical or High severity issue**. It found a cluster of Medium issues that share two themes:

1. **The chat layer trusts every holder of the shared access token equally**, and an unattended (scheduled) run skips the approval gate entirely. Combined, a chat user who would be stopped by an approval prompt can get the same tool call executed without one.
2. **Network and resource boundaries are looser than the surrounding text suggests**: the quick-start script turns on a sandbox network that reaches the operator's whole LAN; a few host-side requests can leak an admin's third-party API key into the model's transcript on a failure; disk is unbounded; the release pipeline pins a signing-adjacent action to a mutable branch.

| ID | Severity | Finding |
|---|---|---|
| CHAT-1 | Medium | Scheduled runs bypass tool approvals, and any access-token holder may create them |
| CHAT-2 | Medium | Without `CHAT_ACCESS_TOKEN` the chat API is open to DNS rebinding and blind cross-site POSTs; no Host/Origin check |
| CHAT-3 | Medium | Non-admin users can change the global system prompt, grant "always allow", wipe shared memory and rewrite notification credentials |
| CHAT-4 | Medium | No HTTP security headers (CSP, frame-ancestors, nosniff, Referrer-Policy, HSTS) |
| CHAT-5 | Medium | Pairing hands phones the master token; no per-device credentials, no revocation, no brute-force protection |
| TOOL-1 | Medium | SerpApi and Best Buy keys travel in the query string and can appear in tool error text sent to the model and stored with the run |
| TOOL-2 | Medium | Marketplace integrity rests on an unsigned index; `http://` registries and absolute `base_url` are accepted |
| TOOL-3 | Medium | The browser's public-address check is a single host-side DNS lookup; the container resolves again |
| SBX-1 | Medium | Networked Docker sandboxes get the whole `bridge` network (LAN, docker0 gateway, cloud metadata); the quick-start script enables it |
| SBX-2 | Medium | No disk quota on workspace volumes, `/tmp` or the writable layer |
| SBX-3 | Medium | Start-up cleanup is instance-blind: two cedar processes on one Docker host destroy each other's sandboxes and volumes |
| CORE-1 | Medium | Attachments are base64-decoded and images fully decoded before the per-key run cap is checked; no decoder limits |
| CORE-2 | Medium | Continuations copy the full parent conversation, attachments included, so a chain grows quadratically |
| OPS-1 | Medium | Release jobs holding signing keys run `cargo-bins/cargo-binstall@main`; other actions are tag-pinned, not SHA-pinned |
| OPS-2 | Medium | Release artifacts are verified only against an unsigned `checksums.txt` from the same origin; the Linux updater runs as root on that basis |
| CHAT-6 | Low | SMTP and push settings give any token holder a port-scan / SSRF primitive from the chat host |
| CHAT-7 | Low | VAPID, SMTP, Pushover, APNs and Firebase secrets sit in plaintext SQLite with default file permissions |
| CHAT-8 | Low | Workspace download relay adds no `nosniff` or forced `attachment` of its own |
| CHAT-9 | Low | `trust-view.js` citation links accept any URL scheme |
| CHAT-10 | Low | Unbounded registrations and conversations; `/api/pairing` spawns a subprocess per request |
| TOOL-4 | Low | Chromium runs without its own sandbox inside the browser container |
| TOOL-5 | Low | Personal-tool secret names can target the `docker`/`openshell` client's own environment |
| TOOL-6 | Low | Gmail `create_draft` derives reply recipients after the address check |
| TOOL-7 | Low | Fetched and browsed text reaches the model undelimited; page-controlled fields flow to the UI |
| TOOL-8 | Low | Several upstream responses are read into memory without a size cap |
| TOOL-9 | Low | Small gaps in the private-address list (NAT64 local-use, Teredo, 6to4 relay) |
| SBX-4 | Low | Shared writable personal-tool environment volumes allow cross-conversation persistence |
| SBX-5 | Low | OpenShell approval prompts are shaped by the model; log parsing may accept forged denial lines |
| SBX-6 | Low | Sandbox and browser images are not reproducible (base image by tag, pip packages unpinned) |
| CORE-3 | Low | No minimum API-key strength; `change-me` is accepted; owner ids are unsalted truncated hashes of the key |
| CORE-4 | Low | `callback_url` stored raw allows log-line injection |
| CORE-5 | Low | Internal error chains from the agent loop reach API clients as `run.error` |
| CORE-6 | Low | An admin can read out a saved model API key by pointing the model entry at their own host |
| CORE-7 | Low | No cap on concurrent SSE streams or file-download containers per key |
| DESK-1 | Low | Desktop webview has no Content-Security-Policy; one `innerHTML` sink |
| DESK-2 | Low (unverified) | Updater anti-rollback depends on a plugin option whose support in the pinned version was not confirmed |
| OPS-3 | Low | Vendored `qrcode-8.2-py3-none-any.whl` at the repo root is unused and unverified |
| OPS-4 | Low | Documented systemd units carry no hardening directives |
| OPS-5 | Low | `cedar-stack.sh` installs Docker and Tailscale with download-and-run-as-root scripts |
| Various | Info | See the Informational section |

## Scope and method

Read in full: `src/` (all modules, including `src/tools/` and `src/sandbox/`), `chat/src/`, `chat/static/`, `pairing/src/`, `desktop/src/`, `desktop/ui/`, `desktop/tauri*.conf.json`, `desktop/capabilities/`, `sandbox/`, `browser/`, `scripts/`, `.github/workflows/`, `migrations/`, `registry/build_index.py`, `DEPLOYMENT.md`, `README.md` (Safety), `chat/README.md`, `PRIVACY.md`, `.env.example`, `.gitignore`. Skimmed: `android/` and `iOS/` for credential storage and transport security, `bench/`, `tests/` for what is asserted.

Method: manual reading with four parallel reviewers, each covering one area (core API, tools, sandbox/desktop/ops, chat), followed by spot verification of every Medium finding in the code by the coordinating reviewer. `cargo audit` is not installed in the review environment, so no advisory scan was run; the Cargo.lock pins were inspected by hand instead. Items marked _unverified_ were not confirmed and say what would confirm them.

Threat model used: (a) a web page, email or document the agent reads that carries a prompt injection, so the model acts against the operator; (b) a holder of a non-admin cedar API key, or of the chat access token, who is not fully trusted; (c) another device on the LAN or tailnet; (d) a website the operator visits while the chat UI runs locally; (e) a malicious community-tool author or compromised registry; (f) a compromised upstream (GitHub account, third-party action, PyPI package). The operator and the holder of an admin key are trusted, consistent with the README.

---

## Findings

Severity reflects a realistic attacker and impact for a self-hosted, often single-user deployment. Each finding names the attacker, the evidence, and a remedy.

### cedar-chat and the web UI

#### CHAT-1 · Medium · Scheduled runs bypass tool approvals, and any access-token holder may create them

- **Where:** `chat/src/api.rs:1349-1361` (`options()` sets `interactive: false` for schedules), `chat/src/api.rs:1900-1949` and `2019-2032` (schedule create and run-now, gated only by the access token), `src/agent.rs:584-586` (the approval gate returns immediately when `!run.interactive`).
- **Issue:** The approval system exists to stop risky tool calls (community tools, OpenShell host grants, anything in `TOOL_APPROVALS`) until a person says yes. A scheduled run is non-interactive, so cedar skips the gate and the call executes. Any holder of `CHAT_ACCESS_TOKEN`, which is every chat user and every paired phone, can create a schedule with the same prompt and tools as a chat message and press "Run now". The README documents that unattended runs call community tools without asking, but does not say that this is reachable on demand from the UI as an approval bypass.
- **Attacker:** a chat user or paired device the operator does not fully trust; a phished or stolen access token.
- **Remedy:** make approvals fail closed for non-interactive runs (a gated call in a non-interactive run is refused with an explanation to the model, rather than allowed). Gate schedule creation, editing and run-now behind `X-Admin-Token`, or at least when the schedule names tools outside a safe set. Document that schedules run with the approval gate off.

#### CHAT-2 · Medium · Without `CHAT_ACCESS_TOKEN` the API is open to DNS rebinding and blind cross-site POSTs

- **Where:** `chat/src/api.rs:416-429` (`require_token` is a no-op when no token is configured), `chat/src/main.rs:28-30` (only a warning, and only when bound off-loopback), `chat/src/config.rs:69` (token optional). No `Host` or `Origin` header validation exists in the router.
- **Issue:** The desktop app and `scripts/cedar-stack.sh` generate a token, but a plain `cargo run` or a hand-written unit does not have to. With no token: (a) a website the operator visits can rebind its DNS name to `127.0.0.1` and then make same-origin requests to `http://evil.example:3000/api/...`; the chat does not check `Host`, so the page reads every conversation, trace and memory and can start runs that execute shell commands with the chat's admin-capable cedar key. (b) Even without rebinding, body-less state-changing POSTs are "simple requests" a cross-site form can fire blind: `/api/schedules/{id}/run`, `/turns/{id}/cancel`, `/conversations/{id}/compact`, `/conversations/{id}/seen`, `/update/check`.
- **Attacker:** any website visited while the chat runs locally without a token.
- **Remedy:** generate a token on first start when none is configured and print it, making the no-token mode an explicit opt-in flag. Reject requests whose `Host` does not match the bind address or `CHAT_PUBLIC_URL`. Require a custom header or a JSON body on every state-changing route, and consider checking `Sec-Fetch-Site`.

#### CHAT-3 · Medium · Non-admin users can change the global system prompt, grant "always allow", wipe shared memory and rewrite notification credentials

- **Where:** `chat/src/api.rs:1407-1430` (`PUT /api/settings` with `system`, up to 20 KB), `chat/src/api.rs:1303-1327` and `chat/src/schedule.rs:179` (that prompt is injected into every conversation and every scheduled run), `chat/src/api.rs:2119-2134` (`decide_approval` with `allow_always`; grants are per cedar owner, so shared by all chat users), `chat/src/api.rs:1504-1507` (`DELETE /api/memory`), `chat/src/api.rs:803-904` (`PUT /api/notifications` overwrites SMTP, APNs, Firebase credentials), `chat/src/api.rs:499-501` (`GET /api/pairing` returns the access token in clear).
- **Issue:** All chat users share one cedar identity. The admin token exists but gates only integrations, model registration, tool install and the update. A co-user can plant standing instructions into everyone's runs, including unattended ones, which is exactly what the memory design works to prevent. Only the memory sharing is documented in `chat/README.md`.
- **Attacker:** a chat user or paired device the operator does not fully trust.
- **Remedy:** require the admin token for the global `system` prompt, `allow_always`, `DELETE /api/memory` and notification credential changes. Document plainly that all users of one chat server share one identity, conversations, settings, schedules and approval grants.

#### CHAT-4 · Medium · No HTTP security headers

- **Where:** `chat/src/api.rs:412-414` (`asset()` sets only `Content-Type` and `Cache-Control`); no header middleware in the router (`326-400`); no `<meta http-equiv>` in either `index.html`.
- **Issue:** The UI reads its token from `localStorage`, so a page that can reach the chat host (any tailnet peer's browser) can frame `https://<host>.ts.net/v4/` fully authenticated and overlay clicks on Delete, Approve, Allow always or Run now. The hand-written markdown renderer and the 40-plus `innerHTML` sites are correct today, but a CSP would be the backstop if one slips. Downloads relay cedar's guessed `Content-Type` without `nosniff`.
- **Attacker:** a malicious site reachable by the operator's browser; a future XSS regression.
- **Remedy:** add a response-header layer: `Content-Security-Policy: default-src 'self'; img-src 'self' blob: https:; connect-src 'self'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'`, `X-Frame-Options: DENY`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, and `Strict-Transport-Security` when `CHAT_PUBLIC_URL` is https. The map view loads tiles from a third party and will need an `img-src` entry.

#### CHAT-5 · Medium · Pairing hands phones the master token; no per-device credentials, revocation or brute-force protection

- **Where:** `pairing/src/connect.rs:12-18`, `chat/src/pairing.rs:71-78` (the QR encodes `cedar://connect?server=...&token=<CHAT_ACCESS_TOKEN>`), `chat/src/api.rs:416-429` and `1530-1539` (no rate limiting or lockout on token checks), `DEPLOYMENT.md:449-456` (plain-HTTP LAN binding documented as acceptable).
- **Issue:** Every device that ever scanned the QR, and every screenshot of the Settings → Phone page, holds a permanent credential. Revoking one device means rotating the token and re-pairing everything. Tokens from the scripts have good entropy, but an operator who types a short one has no protection against online guessing from the LAN or tailnet. The cedar backend key itself is never exposed to phones, which is right.
- **Attacker:** a lost phone; a tailnet or LAN peer.
- **Remedy:** issue per-device tokens from pairing (a short-lived one-time code exchanged for a device token stored hashed in SQLite, listed and revocable in Settings). Add per-IP exponential backoff on failed bearer and admin checks. Warn harder about plain HTTP off loopback.

#### CHAT-6 · Low · SMTP and push settings are an SSRF / port-scan primitive

- **Where:** `chat/src/api.rs:817-830` (any SMTP host, port, `security: none`), `chat/src/channels.rs:941-975` (`builder_dangerous`), `chat/src/api.rs:936-938` (connection error text returned to the caller), `chat/src/api.rs:956` (push endpoints may be `http://` and any host), `chat/src/channels.rs:1023-1050` (POSTed on every notification), `chat/src/store.rs:694-704` (subscriptions unbounded).
- **Remedy:** allow only `https://` push endpoints and deny private, loopback and link-local ranges after resolution; gate SMTP, APNs and Firebase changes behind the admin token; cap subscriptions per server.

#### CHAT-7 · Low · Notification secrets in plaintext SQLite with default permissions

- **Where:** `chat/src/channels.rs:221-291, 529-541`, `chat/src/store.rs:369-377, 672-684`; DB path defaults to the working directory (`chat/src/config.rs:68`).
- **Remedy:** create the database with mode 0600, document a dedicated state directory, and consider encrypting the secrets blob with a key from the environment, as cedar does with `CEDAR_SECRET_KEY`.

#### CHAT-8 · Low · Workspace download relay relies entirely on cedar

- **Where:** `chat/src/api.rs:2050-2061` relays `Content-Type`, `Content-Length`, `Content-Disposition` verbatim. Today cedar sets `attachment` (`src/api.rs:1393-1395`) and the front end shows files via `blob:` URLs with forced MIME types, so an agent-written `.html` does not render same-origin.
- **Remedy:** force `Content-Disposition: attachment` and add `X-Content-Type-Options: nosniff` in the relay regardless of upstream, so a future change in cedar cannot turn this into stored XSS.

#### CHAT-9 · Low · Citation links accept any URL scheme

- **Where:** `chat/static/trust-view.js:301` (`a.href = s.url || '#'`), unlike `sourcesElement` (`327-333`) and the markdown renderer, which allow only `https?:` and `mailto:`. Likely unexploitable today because `fetch_url` would have to succeed on a `javascript:` URL.
- **Remedy:** apply the same scheme test in `cite()`.

#### CHAT-10 · Low · Unbounded growth and amplification

- **Where:** no cap on turns, conversations, schedule runs, push subscriptions or device registrations (`chat/src/store.rs:694-704, 739-749, 784-796`); every finished turn fans out one push per subscription; `GET /api/pairing` runs `tailscale status` per request (`chat/src/pairing.rs:36-48`).
- **Remedy:** cap registrations (for example 20 per kind), rate-limit `/api/pairing`, add retention limits.

### Agent tools

#### TOOL-1 · Medium · SerpApi and Best Buy keys can leak through request error text

- **Where:** `src/tools/flights.rs:103` and `src/tools/bestbuy.rs:122` put the key in the query string; `flights.rs:108` and `bestbuy.rs:132` wrap the `send()` error with `.context(...)` and return it; `src/agent.rs:824-830` formats the error chain into the tool result, which is emitted as a `tool.result` event, stored with the run, shown in the UI and fed back to the model. reqwest's `Error` display includes the request URL with userinfo stripped but the query string intact (`error sending request for url (...)`). The authors know this: `src/mcp.rs` has `without_url` for exactly this reason and uses it for Google and MCP, but not here. The same pattern is in the key-check calls at `flights.rs:79-83` and `bestbuy.rs:95-104`, whose errors go to the admin.
- **Issue:** When serpapi.com or api.bestbuy.com is unreachable or slow enough to hit the request timeout, the admin's key appears in the transcript of whoever is chatting, is persisted in cedar's database, and is sent to the model server. The README promises these keys stay in the cedar process.
- **Attacker:** none needed beyond an upstream outage; a chat user who waits for one.
- **Remedy:** route every error from a request carrying a key through `without_url` (or `e.without_url()`). Add a test that forces a connect failure and asserts the key is absent from the tool error. Audit the other host-side tools for the same `.context` pattern.

#### TOOL-2 · Medium · Marketplace trust rests on the index it downloads

- **Where:** `src/marketplace.rs:402-424` (file hashes come from the same just-fetched index), `src/config.rs:655-660` (`http://` accepted for `TOOLS_REGISTRY_URL`), `src/marketplace.rs:404-408` (`base_url` may be absolute, so a listing can point the host at any URL; the client at `170-176` has no public-only resolver and follows redirects), `src/tools/custom.rs:190-203` (pip requirements may be unpinned ranges, which the release hash does not cover).
- **Issue:** A compromised registry branch or an on-path attacker on a plain-HTTP registry gets Python executed in a networked tool container with workspace access and the admin-saved secrets. The admin must click install and accept the permission list, but the attacker writes that list.
- **Attacker:** a registry maintainer account compromise; a network MITM on an `http://` registry; a tool author whose unpinned dependency is later hijacked on PyPI.
- **Remedy:** require `https` for the registry. Sign the index (minisign or Sigstore) and pin the public key in config. Restrict `base_url` to the index's origin or to relative paths, and run the download through the public-only resolver. Require `==` pins for pip requirements and consider `pip --require-hashes`.

#### TOOL-3 · Medium · The browser's public-address check is one host-side DNS lookup

- **Where:** `src/tools/browser.rs:76-80` calls `check_public_url` (`src/tools/fetch.rs:170-194`), which resolves with the cedar host's resolver and then passes the URL string into the container; the container resolves again through Docker's DNS. `browser/driver.py:330-337` filters only by scheme and resource type; redirects, meta refresh, `location=` and sub-resources are unchecked. The driver supports `CEDAR_BROWSER_PROXY` (`driver.py:365-366`) but `src/browser.rs:118-147` never sets it.
- **Issue:** The README says redirects and links are unchecked; the direct navigation check is weaker than it reads too, since a low-TTL or alternating DNS record defeats it. From inside the browser container the model reaches cloud metadata, this host, and the LAN.
- **Attacker:** a page the agent is steered to.
- **Remedy:** expose `CEDAR_BROWSER_PROXY` as a cedar setting and document running the browser through an egress-filtering proxy or a dedicated Docker network with iptables egress rules. In `route()`, check the response's server address and abort on private IPs. Block `data:` and `blob:` top-level navigations.

#### TOOL-4 · Low (unverified) · Chromium runs without its own sandbox

- **Where:** `browser/driver.py:363` launches with only `--disable-dev-shm-usage`; Playwright's default is `chromium_sandbox=False`. With `--cap-drop ALL` and `no-new-privileges` and no user namespaces, Chromium's own sandbox could not start anyway, so the container is the only layer between a renderer exploit and the container's filesystem and network.
- **Remedy:** document it; keep the browser image minimal with no secrets (as now); consider a Chromium-friendly seccomp profile plus `chromium_sandbox=True`.

#### TOOL-5 · Low · Personal-tool secret names can target the Docker or OpenShell client's own environment

- **Where:** secret values are placed in the `docker`/`openshell` process environment and passed by name (`src/sandbox/docker.rs:86-103, 372-375`; `src/sandbox/openshell.rs:371-373`). The reserved-name list in `src/tools/custom.rs:171-180` omits names the Go-based CLI honours: `GODEBUG`, `GOFLAGS`, `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`, `ALL_PROXY`, `SSH_AUTH_SOCK`, `KUBECONFIG`, `TZ`.
- **Attacker:** a tool author whose manifest asks for a secret named `HTTPS_PROXY` and whose README tells the admin what to paste; relevant when `DOCKER_HOST` is TCP.
- **Remedy:** pass secrets via `--env-file` from an unlinked tmpfs file or a pipe, or `env_clear()` with an explicit allowlist for the CLI; at minimum add `GO`, `*_PROXY`, `SSH_`, `KUBE`, `XDG_`, `TZ`, `TERM` to the reserved prefixes, in `registry/build_index.py:40-41` too.

#### TOOL-6 · Low · Gmail `create_draft` derives recipients after the address check

- **Where:** `src/tools/gmail.rs:279-283` checks `to`, `cc`, `bcc` from the model; `288-294` then pushes `address_of(Reply-To or From)` from the original message unchecked. A sender setting `Reply-To: a@x, b@y` (no angle brackets) controls the reply draft's recipient list. CRLF is still caught by `mime_message` (`482-485`), and no send tool exists.
- **Remedy:** run `check_address` on the derived address and refuse with an explanation when it fails.

#### TOOL-7 · Low · Untrusted tool output is not delimited; page-controlled fields flow to the UI

- **Where:** `fetch_url` (`src/tools/fetch.rs:430-452`), browser (`src/tools/browser.rs:142-160`), `read_document`, and personal-tool stdout (`src/tools/custom.rs:593-600`) hand raw text to the model. Only Gmail/Calendar (`src/tools/mod.rs:689-694`), community tools (`712-724`) and `search_products` label their output as data. Source titles are re-parsed from the first `Title:` line of page text when the page has no `<title>` (`src/sources.rs:40-47`, `fetch.rs:388-405`), so a page can set them. Browser `url`/`title`, Yahoo headlines, SerpApi logo URLs and Brave titles reach the UI unescaped server-side; the UI escapes them today.
- **Remedy:** wrap fetched and browsed text in clear delimiters with a one-line notice; carry the title in the result struct instead of re-parsing output.

#### TOOL-8 · Low · Unbounded response bodies from configured upstreams

- **Where:** `src/mcp.rs:303`, `src/tools/google.rs:88`, `stocks.rs:90, 121`, `flights.rs:85, 110`, `transit.rs:84`, `recipe.rs:117`, `open_meteo.rs:66`, `web_search.rs:81, 159` all call `.text()` or `.json()` on the whole body; truncation happens only afterwards. reqwest is built without compression, so there are no decompression bombs.
- **Remedy:** reuse the chunked, capped reader from `WebClient::get` (`fetch.rs:93-106`) through a small helper, around 8 MB.

#### TOOL-9 · Low · Small gaps in the private-address list

- **Where:** `src/tools/fetch.rs:224-256` misses local-use NAT64 `64:ff9b:1::/48`, Teredo `2001::/32`, IPv4-translated `::ffff:0:0:0/96`, and the 6to4 relay anycast `192.88.99.0/24`.
- **Remedy:** add them; they are cheap and the surrounding code already handles the common forms well.

### Sandboxing

#### SBX-1 · Medium · Networked Docker sandboxes reach the whole `bridge` network, and the quick-start enables it

- **Where:** `src/sandbox/docker.rs:23-25, 37-38`, `src/config.rs:111-117, 327-330, 618-620` (any policy that reaches anything gives the container all of `SANDBOX_NETWORK`, default `bridge`), `scripts/cedar-stack.sh:322-324` (writes `SANDBOX_NETWORK=bridge` into the generated `.env`), `docker.rs:106-114, 143-147` (personal tools with declared hosts get all of `TOOLS_NETWORK`).
- **Issue:** The README is accurate that Docker cannot limit a container to some hosts, but the documented quick-start runs networked. From that sandbox the model reaches the docker0 gateway (any host service bound to `0.0.0.0`, such as vLLM on :8000), RFC1918 neighbours, and `169.254.169.254` on cloud hosts. The model is told the policy is "packages" when in fact it is everything.
- **Attacker:** anyone who can steer the model (prompt injection via fetched content) or a low-trust API key holder.
- **Remedy:** create a dedicated Docker network for sandboxes and add `DOCKER-USER` iptables rules dropping RFC1918, link-local and metadata destinations (or `enable_icc=false` plus an egress filter); document this in `DEPLOYMENT.md`; have `cedar-stack.sh` default to `none` or warn prominently; point operators at the OpenShell runtime, which already enforces host allowlists correctly.

#### SBX-2 · Medium · No disk quotas

- **Where:** `src/sandbox/docker.rs:28-65`, `src/sandbox.rs:319-341` set memory, CPU and pids limits, but no `--storage-opt size=`, no size-limited `--tmpfs`, and the `cedar-ws-*` volume (`docker.rs:55-56`) has no limit. `sandbox/Dockerfile:50-51` puts `PYTHONUSERBASE` under `/workspace/.local`, so pip output lands on the unbounded volume, which outlives the run for `WORKSPACE_TTL_SECS`.
- **Attacker:** any authenticated run, or an injected model, fills the host disk and takes down Docker.
- **Remedy:** mount `/tmp` and `/home/agent` as size-limited tmpfs (`--tmpfs /tmp:rw,nosuid,nodev,size=256m`), make the rootfs `--read-only` (only `/tmp`, `/home/agent`, `/sandbox`, `/workspace` need to be writable), and enforce a per-workspace quota (XFS project quota, `--storage-opt`, or a periodic size check that fails the run and expires the workspace).

#### SBX-3 · Medium · Start-up cleanup is instance-blind

- **Where:** `src/sandbox/docker.rs:462-466, 514-553` (removes every container with the bare `cedar.run` label), `src/manager.rs:181-192` (`remove_stray_workspaces` deletes every `cedar.workspace` volume not in this instance's database), `src/sandbox/openshell.rs:933-947` (deletes every `cedar.kind` sandbox in the workspace; `tests/openshell.rs:321-333` works around this).
- **Issue:** Two cedar processes on one Docker host, or one OpenShell workspace (staging plus production, a test instance beside the desktop app, the desktop app beside a systemd install), destroy each other's live sandboxes and delete each other's workspace volumes at start.
- **Remedy:** label containers, volumes and OpenShell sandboxes with an instance id (hash of `DATABASE_URL` or a stored id) and filter cleanup and stray removal by it; never delete volumes that are not in the instance's database unless they carry the instance label. Document "one cedar per Docker host" meanwhile.

#### SBX-4 · Low · Shared writable personal-tool environment volumes

- **Where:** `src/sandbox.rs:233-237` (`cedar-tool-env-<name>`, one per tool, outlives runs), mounted read-write in every call (`docker.rs:155-156`, `openshell.rs:759-762`); `PYTHONUSERBASE` (`docker.rs:92`) makes anything dropped there importable in later calls, with that tool's secrets in the environment.
- **Attacker:** one conversation plants a `sitecustomize.py` or `.pth` through a tool that writes files derived from model input; every later call of that tool, by any key, imports it.
- **Remedy:** mount the environment volume read-only during calls and write only during `setup_tool`, or key it per API key or conversation. Document that personal tools are trusted code with a shared environment.

#### SBX-5 · Low · OpenShell approval prompts are model-shaped

- **Where:** `src/sandbox/openshell.rs:784-815`, `src/openshell.rs:311-386` (denials parsed from `openshell logs --source sandbox`), `src/policy.rs:256-282` (`allow()` writes `binary host:port` with `tls: skip`). `Endpoint::check` (`policy.rs:84-102`) correctly forbids wildcards, so a grant is never wider than one program to one host and port. _Unverified_: whether sandbox-sourced logs can contain lines the sandboxed process itself prints; the parser (`openshell.rs:330-345`) looks only for `" DENIED "` and `OCSF` substrings.
- **Remedy:** test by echoing a fake denial line inside a sandbox; if it appears in the parsed stream, parse only lines carrying OpenShell's own prefix and show the binary path prominently in the approval UI. Consider not defaulting `tls: skip`.

#### SBX-6 · Low · Sandbox and browser images are not reproducible

- **Where:** `sandbox/Dockerfile:10` (`debian:bookworm-slim` by tag), `:25` and `:32` (pip packages unpinned, no `--require-hashes`); `browser/Dockerfile:319, 331` (base by tag; `playwright==1.56.0` is pinned, its dependencies are not); `release.yml:229-233` rebuilds `:latest` each release.
- **Remedy:** pin `FROM ...@sha256:`, use a `requirements.txt` with hashes, and let Dependabot or Renovate bump them.

### cedar core server

#### CORE-1 · Medium · Attachments are decoded before the per-key run cap; no decoder limits

- **Where:** `src/api.rs:428` (`validate_attachments` base64-decodes up to 32 MiB), `:454` (`images::downscale_all`) runs before `:541` (`insert_run`, the only per-key cap). `src/images.rs:95-100` reads dimensions, then fully decodes any image whose longer side exceeds `max_side` with no explicit `image::Limits`. The body limit for `POST /v1/runs` is 48 MiB (`api.rs:49`); there is no server-wide connection or rate limit.
- **Attacker:** any non-admin key holder loops `POST /v1/runs` with ten oversized images; each request is refused with 429 only after the decode work.
- **Remedy:** check the active-run count (or a cheap per-key token bucket) before decoding; move `downscale_all` after `insert_run` or into the run task; set explicit `image::Limits` (max dimensions around 20k, `max_alloc` around 256 MiB) on the reader; add a concurrency or rate limit layer on `/v1/runs`.

#### CORE-2 · Medium · Continuations copy the whole parent conversation, attachments included

- **Where:** `src/store.rs:366` (`INSERT INTO messages ... SELECT ... FROM messages WHERE run_id = parent`); the opening message carries image blocks with full base64 (`src/attachments.rs:59`), so each continuation duplicates up to 20 MiB per image, and every LLM turn re-sends it (`src/llm.rs`, `media_part`). A chain of k runs stores O(k²) bytes.
- **Remedy:** store attachments once and reference them by `run_attachments` row id, materialising blocks at send time; or cap chain depth and copied bytes; or strip `source.data` to a reference when copying.

#### CORE-3 · Low · No minimum API-key strength; owner ids are unsalted hashes of the key

- **Where:** `src/config.rs:376` refuses only an empty list; `.env.example` ships `CEDAR_API_KEYS=change-me`; `src/auth.rs:33-35` derives the owner id as the first 8 bytes of SHA-256(key), stored in `runs.owner`, `memories.owner`, `workspaces.owner`, `approval_grants.owner`, `run_notifications.owner`.
- **Issue:** A leaked database lets anyone recover low-entropy keys offline by hashing candidates against `owner`. The admin-subset check (`config.rs:472-474`) is correct.
- **Remedy:** refuse keys shorter than about 16 characters and the literal placeholder, warn otherwise; derive owner ids with a keyed hash (HMAC with `CEDAR_SECRET_KEY` or a stored server salt).

#### CORE-4 · Low · `callback_url` stored raw allows log-line injection

- **Where:** `src/webhooks.rs:188-191` validates the parsed URL (which strips tab, CR, LF per WHATWG) but returns `raw.to_string()`; the raw value is later logged with Display at `webhooks.rs:119, 132, 134`.
- **Remedy:** return the normalised `url.to_string()`; log user-controlled fields with `?` (Debug) formatting.

#### CORE-5 · Low · Internal error chains reach API clients as `run.error`

- **Where:** `src/agent.rs:71-74` (`From<anyhow::Error>` formats `{e:#}`); `?` on store, MCP and attachment calls lets sqlx, Docker and MCP error chains (paths, hostnames) reach the run owner. `src/manager.rs:380-383` sanitises sandbox-creation errors correctly, so the intended policy is not applied uniformly.
- **Remedy:** log the detail and map infrastructure failures to a fixed message, as `manager.rs` does.

#### CORE-6 · Low · An admin can read out a saved model API key

- **Where:** `src/integrations.rs:586` (`server_models` with `id` set uses the saved key against a caller-supplied `base_url`), `:528-530` (`set_model` without `api_key` sends the saved key to the new `base_url`). The README says admins can replace credentials; this is slightly stronger, a read-out.
- **Remedy:** require an explicit `api_key` when `base_url`'s host differs from the registered entry's host, or document it.

#### CORE-7 · Low · No cap on concurrent SSE streams or file-download containers per key

- **Where:** `src/sse.rs:34, 87` (each stream subscribes and polls), `src/api.rs` (`stream_events`, `stream_notifications`, `download_file` have no per-key concurrency limit); each download spawns a `docker run` (`src/sandbox.rs:290-310`) and buffers up to 32 MiB.
- **Remedy:** a per-key semaphore on streams and file reads; stream download bodies instead of buffering.

### Desktop app

#### DESK-1 · Low · No Content-Security-Policy; one `innerHTML` sink

- **Where:** `desktop/tauri.conf.json` has no `app.security.csp`; `desktop/ui/app.js:479` sets `innerHTML` to the SVG from `share_qr` (app-generated by the `qrcode` crate). The remote chat origin gets no IPC (`desktop/capabilities/default.json:63-65`), which is correct.
- **Remedy:** set a CSP (`default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:`) and render the QR as an `<img src="data:image/svg+xml,...">`.

#### DESK-2 · Low (unverified) · Updater anti-rollback depends on a plugin option not confirmed for the pinned version

- **Where:** `desktop/tauri.conf.json:33-38` sets `requireSignedVersion: true` and `desktop/src/update.rs:458-459` relies on it; the pinned `tauri-plugin-updater 2.14.0` (`desktop/Cargo.lock`) could not be inspected offline. Unknown config keys are ignored silently, which would leave a replay of an older, genuinely signed bundle undetected. The rest of the updater is well done: pubkey baked at build, builds without the key refuse to update, download verified before servers stop.
- **Remedy:** confirm with `grep -rn require_signed_version` in the plugin source, or a test feeding the updater a signature for an older version; if unsupported, compare `update.version` against `CARGO_PKG_VERSION` before `download()` and refuse non-increasing versions.

### Operations, deployment and supply chain

#### OPS-1 · Medium · Release jobs holding signing keys run an action pinned to a mutable branch

- **Where:** `.github/workflows/release.yml:307, 423` and `release-cache.yml:67, 112` use `cargo-bins/cargo-binstall@main` in jobs whose `env` holds `TAURI_SIGNING_PRIVATE_KEY`, `APPLE_CERTIFICATE` and `ASC_KEY_P8`. Every other action is pinned by tag (`actions/checkout@v4`, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`, `docker/*`, `maxim-lobanov/setup-xcode@v1`, `dorny/paths-filter@v3`, `gradle/actions/setup-gradle@v4`); tags are mutable.
- **Attacker:** a compromise of the third-party action's repository signs malicious updates that every installed desktop app would accept.
- **Remedy:** pin every `uses:` to a full commit SHA and let Dependabot keep them current; install `tauri-cli` with `cargo install --locked tauri-cli --version x.y.z`; move signing into a separate job with `environment:` protection so only the final step sees the keys.

#### OPS-2 · Medium · Release artifacts are verified only against an unsigned `checksums.txt`

- **Where:** `scripts/cedar-update.sh:167-177` (tarball and checksums from the same release directory, SHA-256 compared, no signature); `release.yml:691-693` publishes `checksums.txt` unsigned; no `actions/attest-build-provenance`. The unprivileged `cedar` user triggers the root-run updater through a request file (`scripts/systemd/cedar-update.path:7`, `chat/src/update.rs:296-313`); the script sanitises the version and refuses downgrades without `--force`, which is good.
- **Attacker:** a compromise of the GitHub account or release pipeline, or of `CEDAR_UPDATE_BASE` in `/etc/cedar/update.env`, installs arbitrary root-run binaries on every server with the path unit enabled.
- **Remedy:** sign `checksums.txt` (minisign, or cosign keyless) in `release.yml` and verify in `cedar-update.sh`; or publish SLSA provenance and verify it. Keep the version regex and downgrade check.

#### OPS-3 · Low · Vendored `qrcode-8.2-py3-none-any.whl` is unused and unverified

- **Where:** repository root; committed in `dbe9154`; referenced by nothing (`grep -rn qrcode` finds only the Rust crate). No hash is recorded. Its SHA-256 is `16e64e0716c14960108e85d853062c9e8bba5ca8252c0b4d0231b9df4060ff4f`, not compared with PyPI offline.
- **Remedy:** `git rm` it and add `*.whl` to `.gitignore`; if a QR is needed for the landing page, generate it in CI from pinned, hashed dependencies.

#### OPS-4 · Low · Documented systemd units carry no hardening

- **Where:** `DEPLOYMENT.md:289-327`: `cedar.service` and `cedar-chat.service` set `User=` and `EnvironmentFile=` only. `scripts/systemd/cedar-update.service:10-11` does set `PrivateTmp` and `NoNewPrivileges`.
- **Remedy:** add `NoNewPrivileges=yes`, `ProtectSystem=strict`, `ProtectHome=yes`, `PrivateTmp=yes`, `ReadWritePaths=/var/lib/cedar`, `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6`, `CapabilityBoundingSet=`. The cedar user is in `docker` (effectively root) so this is partial for cedar, but cedar-chat can be hardened fully.

#### OPS-5 · Low · `cedar-stack.sh` installs Docker and Tailscale with download-and-run-as-root scripts

- **Where:** `scripts/cedar-stack.sh:99-107, 119-122, 185-189`. Interactive, HTTPS, the command is shown and the user says yes, but there is no pinning.
- **Remedy:** at least `curl --proto '=https' --tlsv1.2`; prefer distro packages (`apt install docker.io`) and Tailscale's signed apt repository.

### Informational

- **Windows Authenticode subject check is a loose regex.** `desktop/src/winsetup.rs:156` (`O="?Docker Inc`) and `tssetup.rs:522` match any organisation starting with those words. macOS checks the Team ID, which is the right anchor. Anchor the regex or compare the thumbprint.
- **`docker exec` cannot stop background processes.** `src/sandbox/docker.rs:67-84` runs `timeout -k 2 N sh -c cmd`, which kills `sh` only; `tests/docker.rs:113-126` expects the background process to survive. Bounded by pids and memory limits and by the output backstop that destroys the container. OpenShell uses the session-killing `sandbox/exec_wrapper.py`; Docker could use it too.
- **File tools accept absolute paths and `..`.** `src/sandbox.rs:430-438` rejects only empty and NUL; descriptions say "relative to /workspace". Everything runs inside the container via `docker exec` with `--` separators, so the host is safe and this only widens what the model may read inside its own container. Tighten the descriptions or restrict to `/workspace` and `/tmp`.
- **`TOOLS_DIR` entries follow symlinks** (`src/tools/custom.rs:445-448`); only host write access can abuse it.
- **MCP tool names can collide** (`a.b` and `a_b` both become `a_b`, first wins; `src/tools/mcp.rs:117-129`) and remote descriptions enter the prompt verbatim, truncated at 2000 characters. Fine while the server is Google's.
- **`WEBHOOK_RETRY_SECS=0` is accepted** (`src/config.rs:592-595`) and yields immediate retries, bounded by 12 attempts and a 10 s timeout. Require at least 1.
- **Deferred transactions in `upsert_model` and `delete_model`** (`src/store.rs:929, 1002`) can hit `SQLITE_BUSY_SNAPSHOT` under WAL; `insert_run` correctly uses `BEGIN IMMEDIATE`. Use it there too.
- **`run.started` discloses sandbox details** (network, policy, hosts, memory, cpus; `src/sandbox.rs:704-722`) to any key; intentional per README.
- **Notification channels carry conversation text in clear to third parties by design**: Pushover and email send title plus a 300-character excerpt, APNs carries the message; Web Push is encrypted end to end and Android carries only ids. Worth a note in Settings.
- **Map tiles come from a third-party tile server**, leaking plotted coordinates. Acceptable; document it.
- **Two `rsa` crate versions** (0.7.2 and 0.9.10) are present via transitive dependencies; `rsa < 0.9.6` has RUSTSEC-2023-0071 (timing side channel). _Unverified_ whether the 0.7.2 path is reachable. Add `cargo audit` (or `cargo deny`) to CI to settle this and catch future advisories.
- **`.env.example` ships `CEDAR_API_KEYS=change-me`** as a placeholder; see CORE-3.

---

## What is done well

These are worth keeping and worth knowing before changing anything above.

- **Core authorization.** Every `/v1` route is behind `require_auth`; all 22 admin handlers call `require_admin`; owner-scoped lookups return identical 404s for foreign ids (`src/api.rs:245-251`). Continuation is checked inside one `BEGIN IMMEDIATE` transaction (owner, finished, not already continued, workspace reservation), so there is no race on cross-owner reuse or double continuation (`src/store.rs:253-330`). Waits are keyed by run and id and consumed once, so approvals cannot be replayed across owners.
- **SQL.** All queries use bound parameters; `format!` splices only compile-time constant column lists, in both servers.
- **Secrets at rest.** XChaCha20-Poly1305 with a fresh 24-byte random nonce and the row name as associated data (`src/secrets.rs:48-59`); `callback_secret` is never in the run columns returned to clients; `/v1/models` omits `base_url`.
- **Webhook and fetch SSRF guard.** Redirects disabled for webhooks and re-checked for fetches; a custom `PublicOnly` resolver applied at connect time defeats DNS rebinding; the URL parser normalises decimal, hex, IPv4-mapped, NAT64 and 6to4 forms, with tests (`src/tools/fetch.rs:208-262, 582-595`). HMAC-SHA256 signing includes a timestamp and matches RFC 4231 vectors.
- **Container hardening matches the README.** `--cap-drop ALL`, `--security-opt no-new-privileges`, `--user 1000:1000`, memory equal to memory-swap, cpus, pids-limit, `--network none` unless the policy reaches something; default seccomp and AppArmor left in force; no privileged flags, docker socket or host bind mounts in run containers; the image and command always come last so nothing after them is parsed as a flag (asserted in `src/sandbox/docker.rs:672-673`). Integration tests assert the symlink-escape, non-root, offline and read-only `/etc` properties.
- **Nothing untrusted becomes a flag.** Commands go as one argv element to `sh -c`; paths as `$1` behind `--`; names are UUID-derived; tool names are `[a-z0-9_]`; pip requirements are character-checked and passed after `--`; `--mount` CSV quoting handles commas, quotes and Windows paths. Secrets reach containers by name only (`-e NAME`), values via the client's environment, so they never appear in `ps`; the desktop supervisor does the same for its children and writes `config.json` 0600 atomically.
- **Workspace downloads** go through a locked-down, read-only, offline one-off container that resolves symlinks inside `/workspace`; `check_workspace_path` rejects `..`, absolute, empty and NUL components.
- **Chat front end.** Header-based bearer auth with SHA-256 and constant-time compare, no cookies, no CORS layer, so cross-origin reads and JSON-body CSRF are blocked by the browser. The hand-written markdown renderer escapes all text and links only `https?:` and `mailto:`; card renderers accept only `https://` URLs and set text via `textContent`; files are shown from `blob:` URLs with MIME forced from the extension and SVG never opened in a tab; the service worker caches nothing. The cedar API key never leaves the server. OAuth state is 32 random bytes, single-use, 10-minute TTL; the callback has a fixed redirect target.
- **Marketplace extraction.** No archives are extracted: files are written by validated flat names, staged and atomically swapped, and permissions must match both the index and what the admin accepted.
- **Google tools.** REST paths are built with `path_segments_mut().extend()` so ids cannot change endpoints; only read and draft operations exist; `GMAIL_TOOLS`/`CALENDAR_TOOLS` are enforced.
- **Elevated installers (desktop).** HTTPS downloads copied into admin-only directories before verification, verified by Apple Team ID or Authenticode, scripts passed as argv (macOS) or hash-checked before execution (Windows), with parser tests. Both servers bind `127.0.0.1`; the chat is exposed only via Tailscale Serve (tailnet), never Funnel.
- **GitHub Actions.** No `pull_request_target`; release requires a merged PR from a same-repo `release/*` branch; untrusted event data passes through `env:` rather than being interpolated into `run:`; top-level `permissions:` blocks are present; macOS artifacts are signed, notarised and stapled.
- **Mobile apps.** Android forbids cleartext, `allowBackup=false`, backup and device-transfer rules exclude everything, token encrypted with a Keystore AES key; iOS stores the token in the Keychain with `kSecAttrAccessibleAfterFirstUnlock`; no custom trust managers.
- **Memory.** Writes refused in non-interactive runs; one-line normalisation prevents forged `[id]` lines; the list is framed as the user's facts, not instructions.

## Checked and found acceptable

Listed so the next reviewer can skip them: admin key not in `CEDAR_API_KEYS` fails start-up; `max_turns` clamped to the cap; prompt, system, attachment, name, callback and secret lengths enforced; list limits clamped; run timeout excludes approval waits, each bounded by its own timeout; event log bounded by turns times output cap; trace capped at 10k events; `Content-Disposition` built from sanitised ASCII plus RFC 5987 encoding; no `unsafe` in `src/`; no `unwrap` or `expect` on client-controlled data in the handlers of either server; PDF rendering runs inside the sandbox with timeout and output caps; `normalize_llm_url` rejects userinfo and non-http(s); SQLite in WAL with a 5 s busy timeout and foreign keys on; static assets served from `include_bytes!` with fixed routes (no traversal); `encode_path` percent-encodes per segment; `tailscale status` runs with fixed arguments and a 5 s timeout; update check is an HTTPS GET of GitHub JSON with ETag, drafts and prereleases ignored; `exec_wrapper.py` runs as the sandbox user, double-forks, caps both streams and kills the whole session on timeout, and the host re-caps output; pause/resume inspects state by the fixed `cedar-run-<uuid>` name and refuses anything but running or paused; TTL enforced by a janitor; `chown_tool_env`/`copy_volume` run offline with only CHOWN, FOWNER, DAC_OVERRIDE; `cedar-update.sh` validates the version, follows no symlinks, extracts only the two named members with `--no-same-owner`, cross-checks `--version`, and rolls back on failed health; `.gitignore` covers `.env`, both databases, keystores and `google-services.json`, and `git ls-files` shows no committed secrets; reqwest is built without compression, so no decompression bombs; Debug impls redact Google credentials; no tracing call logs a key or token.

## Suggested order of work

1. **Close the approval bypass and tighten the chat's privilege split** (CHAT-1, CHAT-3): fail closed on gated calls in non-interactive runs; admin-gate schedules, the global system prompt, `allow_always`, memory wipe and notification credentials. Small changes, large effect.
2. **Make the chat safe by default** (CHAT-2, CHAT-4): generate a token when none is configured, check `Host`, add the security-header layer.
3. **Stop key leakage through error text** (TOOL-1): one helper applied in a handful of places, plus a test.
4. **Fix the quick-start network posture** (SBX-1, TOOL-3): default `SANDBOX_NETWORK=none` in `cedar-stack.sh` or document a filtered sandbox network; expose the browser proxy setting.
5. **Harden the release pipeline** (OPS-1, OPS-2): SHA-pin actions, pin `tauri-cli`, sign `checksums.txt`, verify in the updater; add `cargo audit` to CI.
6. **Resource limits** (CORE-1, CORE-2, SBX-2, CORE-7): decoder limits and ordering, attachment deduplication, tmpfs and quota, per-key stream caps.
7. **Instance isolation** (SBX-3): instance labels on containers, volumes and OpenShell sandboxes.
8. **Marketplace trust** (TOOL-2): https-only, signed index, origin-restricted `base_url`, pinned requirements.
9. **Per-device pairing credentials and rate limiting** (CHAT-5): a larger design change; worth planning once the above are in.
10. The Low and Informational items as convenient, several of which are one-line fixes (CHAT-9, TOOL-6, TOOL-9, CORE-4, OPS-3).

## Limitations of this review

Static reading only; no dynamic testing, fuzzing or deployment was performed. `cargo audit` was not available; dependency advisories were checked by hand against the lock file only. Items marked _unverified_ (DESK-2, TOOL-4, SBX-5, the `rsa` crate path) need the confirmation steps named with them. Reviewers were coordinated in parallel per area, so interactions between areas were examined mainly through the cross-cutting themes in the summary; a follow-up that walks a single prompt-injected run end to end through chat, cedar, a networked sandbox and the browser would be a useful complement.
