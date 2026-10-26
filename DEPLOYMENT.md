# Deploying cedar and the chat UI on a DGX Spark

This guide sets up the whole stack on one NVIDIA DGX Spark and shows how to open the chat UI from
your laptop, and over HTTPS from your phone with Tailscale (the iOS and Android apps need HTTPS).

For running cedar and the chat on a Mac or a Windows PC instead, with a model server elsewhere,
see the [desktop app](desktop/README.md): one DMG or MSI, no systemd.

```
laptop browser ──SSH tunnel───────────┐
phones / any device ───Tailscale HTTPS┴─▶ cedar-chat :3000 ──▶ cedar :8080 ──▶ vLLM :8000 (DeepSeek V4)
                                                                     │
                                                                     └──▶ one Docker container per conversation (the agent's tools)
```

| Service | What it is | Listens on | Started by |
| --- | --- | --- | --- |
| vLLM | Serves the model over an OpenAI-compatible API | `127.0.0.1:8000` | you (see [step 2](#2-make-sure-vllm-is-ready-for-cedar)) |
| `cedar` | The agent server: runs the loop and the sandboxes | `127.0.0.1:8080` | systemd |
| `cedar-chat` | The web UI | `127.0.0.1:3000` | systemd |

Only `cedar-chat` needs to be reachable from outside the Spark. Keep `cedar` and vLLM on
loopback.

If vLLM runs on another machine, skip step 2 and register its address instead of the local one in step 5.

To try the stack without installing anything, `scripts/cedar-stack.sh` builds it and runs vLLM,
cedar, and cedar-chat in a tmux session from the checkout, with settings in `.env` (see
`scripts/cedar-stack.sh --help`). Nothing restarts that session after a crash or a reboot, so for
a server that should stay up, follow the steps below.

## 1. Prepare the Spark

The Spark is an ARM64 (aarch64) machine running DGX OS. Everything here builds natively on it.

```sh
sudo apt update
# pkg-config and libssl-dev: cedar-chat's Web Push encryption links OpenSSL
sudo apt install -y build-essential pkg-config libssl-dev git curl jq openssl

# Rust (cedar needs a recent stable toolchain)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
rustc --version
```

cedar runs each agent's tools in Docker containers, so Docker must work for your user:

```sh
docker info --format '{{.ServerVersion}}'    # prints a version number
```

If that says "permission denied", add yourself to the `docker` group, then log out and back in:

```sh
sudo usermod -aG docker "$USER"
```

Get the code:

```sh
git clone git@github.com:hanneshapke/cedar.git ~/cedar
cd ~/cedar
```

### Memory: the model and the sandboxes share it

The Spark has 128 GB of **unified** memory, shared by the CPU and GPU. The model, cedar, and every
sandbox container draw from the same pool, and running out can freeze the whole machine, not just
fail one process.

- The official FP8 DeepSeek V4 Flash weights need two or more Sparks
  ([2 Sparks, TP=2](https://forums.developer.nvidia.com/t/deepseek-v4-flash-official-fp8-running-across-2x-dgx-spark-tp-2-mtp-200k-ctx-recipe-numbers/370309),
  [4 Sparks, TP=4](https://forums.developer.nvidia.com/t/deepseek-v4-flash-official-fp8-running-across-2x-dgx-spark-tp-2-mtp-200k-ctx-recipe-numbers/370309/174)).
- A single Spark only fits a heavily quantized build, for example the community
  [2-bit hybrid quant ported to vLLM for GB10](https://forums.developer.nvidia.com/t/deepseekv4-flash-hybrid-quant-1x-dgx-spark-antirezs-optimized-128-gb-mlx-recipe-ported-to-vllm-for-gb10/369584),
  which its author reports at about 110 GiB resident while serving. That leaves little headroom.
  These are community reports, not something this repo tests.

With a model that large on one Spark, run few agents at once (`MAX_CONCURRENT_RUNS=2` or even `1`
in step 4). Each sandbox is capped at `SANDBOX_MEMORY` (512 MB by default), so concurrent runs add
up to `MAX_CONCURRENT_RUNS × SANDBOX_MEMORY` on top of the model.

## 2. Make sure vLLM is ready for cedar

How you start vLLM on the Spark is up to you. For GB10 that usually means NVIDIA's
[vLLM playbook](https://build.nvidia.com/spark/vllm) and its NGC container, or a community GB10
image. cedar needs three things from whatever you run:

1. **An OpenAI-compatible endpoint**, by default `http://127.0.0.1:8000/v1`.
2. **Tool calling enabled.** The agent acts only through tool calls, so vLLM must be started with
   the DeepSeek V4 parser flags (from the
   [vLLM recipe](https://recipes.vllm.ai/deepseek-ai/DeepSeek-V4-Flash)):

   ```
   --tokenizer-mode deepseek_v4 --tool-call-parser deepseek_v4 --enable-auto-tool-choice --reasoning-parser deepseek_v4
   ```

   Without them, tool calls come back as plain text and runs finish without doing anything.
3. **A known model name.** cedar must send the name vLLM registered, which is the model path or
   the `--served-model-name` value.

Check the first and third:

```sh
curl -s http://127.0.0.1:8000/v1/models | jq -r '.data[].id'
```

Whatever it prints is the model name you register in step 5. Then check that tool calling works,
which is the check that matters most:

```sh
MODEL=$(curl -s http://127.0.0.1:8000/v1/models | jq -r '.data[0].id')
curl -s http://127.0.0.1:8000/v1/chat/completions -H 'Content-Type: application/json' -d '{
  "model": "'"$MODEL"'",
  "max_tokens": 256,
  "messages": [{"role": "user", "content": "Run the command: echo hi"}],
  "tools": [{"type": "function", "function": {
    "name": "bash", "description": "Run a shell command",
    "parameters": {"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}}}]
}' | jq '.choices[0] | {finish_reason, tool_calls: .message.tool_calls}'
```

You want `finish_reason: "tool_calls"` and a non-empty `tool_calls` array. If `tool_calls` is
`null` and the reply is text, vLLM is running without the tool-call flags (or the build you use
doesn't support them for this model). Fix that before going on.

## 3. Build cedar, the chat UI, and the sandbox image

```sh
cd ~/cedar
cargo build --release --workspace
sudo install -m 0755 target/release/cedar target/release/cedar-chat /usr/local/bin/

# The image the agent's tools run in (Debian with python3, pip, git, curl, jq, and the PDF renderer)
docker build -t cedar-sandbox sandbox/
```

The chat UI's HTML, CSS, and JavaScript are compiled into the `cedar-chat` binary, so there is
nothing else to deploy.

To skip the build, take the binaries from a [release](https://github.com/hanneshapke/cedar/releases)
instead: `Cedar-<version>-linux-arm64.tar.gz` on the Spark (`-linux-x64` on an x86_64 server).
They need glibc 2.35 or newer and no OpenSSL library.

```sh
V=0.1.4   # the release you want
curl -fLO https://github.com/hanneshapke/cedar/releases/download/v$V/Cedar-$V-linux-arm64.tar.gz
tar -xzf Cedar-$V-linux-arm64.tar.gz
sudo install -m 0755 Cedar-$V-linux-arm64/cedar Cedar-$V-linux-arm64/cedar-chat /usr/local/bin/
```

By default the sandbox has no network, so the agent can only use what the image ships with; add
tools to `sandbox/Dockerfile` and rebuild the image if your tasks need more. To let the agent
`pip install` packages itself, set `SANDBOX_NETWORK=bridge` in cedar's environment file (step 4).
That gives the container outbound access to whatever the Docker bridge network can reach, so
only do it on a machine where that is acceptable. Installed packages live in the run's workspace
and survive follow-up messages until the workspace expires. Web search is separate: the optional
`web_search` tool (see `BRAVE_API_KEY` in step 4) runs in the cedar process, so the host needs
outbound HTTPS to `api.search.brave.com`. So do `fetch_url` and `download_file`, which are on by
default: the host needs outbound HTTP(S) to the sites the agent reads. They refuse private and
loopback addresses, so the agent can't use them to reach vLLM or anything else on your network;
set `WEB_FETCH=off` to remove them. The `weather_forecast` tool is also on by default and needs
outbound HTTPS to `api.open-meteo.com` and `geocoding-api.open-meteo.com`; set `WEATHER=off` to
remove it. So does `show_map` (which only needs the geocoding host); set `MAPS=off` to remove it.
The map itself is drawn by the chat page in the user's browser, which loads the tiles from
`tile.openstreetmap.org`, so the people using the chat need access to that host, not the server.

## 4. Configure

Create a service user, a state directory, and the two environment files. The user is in the
`docker` group, which is effectively root on this machine, so keep this account to running cedar.

```sh
sudo useradd --system --create-home --home-dir /var/lib/cedar \
  --shell /usr/sbin/nologin --groups docker cedar

CEDAR_KEY=$(openssl rand -hex 24)     # chat -> cedar
CHAT_TOKEN=$(openssl rand -hex 24)    # browser -> chat
ADMIN_TOKEN=$(openssl rand -hex 24)   # unlocks Settings -> Server in the browser
SECRET_KEY=$(openssl rand -hex 32)    # encrypts the API keys saved there; keep it with your secrets
MODEL=$(curl -s http://127.0.0.1:8000/v1/models | jq -r '.data[0].id')   # or type the name yourself
echo "model: ${MODEL:?vLLM is not reachable; set MODEL to the name it serves}"

sudo install -d -m 0755 /etc/cedar

sudo tee /etc/cedar/cedar.env >/dev/null <<EOF
CEDAR_API_KEYS=$CEDAR_KEY
CEDAR_ADMIN_KEYS=$CEDAR_KEY           # lets the chat UI register models and save keys
CEDAR_SECRET_KEY=$SECRET_KEY
BIND_ADDR=127.0.0.1:8080
DATABASE_URL=sqlite:///var/lib/cedar/cedar.db

# The model server is registered through the chat UI (step 5), not here.
# BRAVE_API_KEY=          # enables the web_search tool (key from api-dashboard.search.brave.com)

# Tuned for one Spark running a large model (see "Memory" above)
MAX_CONCURRENT_RUNS=2
RUN_TIMEOUT_SECS=1800
RUST_LOG=info,sqlx=warn
EOF

sudo tee /etc/cedar/chat.env >/dev/null <<EOF
CEDAR_URL=http://127.0.0.1:8080
CEDAR_API_KEY=$CEDAR_KEY
CHAT_BIND_ADDR=127.0.0.1:3000
CHAT_DATABASE_URL=sqlite:///var/lib/cedar/cedar-chat.db
CHAT_ACCESS_TOKEN=$CHAT_TOKEN
CHAT_ADMIN_TOKEN=$ADMIN_TOKEN
EOF

sudo chmod 600 /etc/cedar/cedar.env /etc/cedar/chat.env   # systemd reads them as root
echo "Chat access token (you type this in the browser): $CHAT_TOKEN"
echo "Admin token (unlocks the model settings in the browser): $ADMIN_TOKEN"
```

Keep the printed tokens. The chat page asks for the access token on first load; the admin token
unlocks Settings → Server, where the vLLM server is registered (step 5 does it
with curl first). The full list of settings is in
the [README](README.md#configuration) and [chat/README.md](chat/README.md#configuration).

Settings worth knowing on this hardware:

| Setting | Why |
| --- | --- |
| `RUN_TIMEOUT_SECS` | A whole run, across all its model turns, must finish within this (default 600). A local model is slower than a hosted one, so give it room. |
| `MAX_CONCURRENT_RUNS` | How many agents run at once; the rest queue. |
| `BRAVE_API_KEY` | Optional. Turns on the `web_search` tool. Search queries are sent to Brave; the key never enters a sandbox container. |
| `WEB_FETCH` | `on` by default. `off` removes `fetch_url` and `download_file`, for a host that should make no outbound requests on the agent's behalf. |
| `WEATHER` | `on` by default. `off` removes `weather_forecast`. It uses the public Open-Meteo API, which is free for non-commercial use only; for commercial use set `WEATHER_BASE_URL` and `WEATHER_GEOCODING_BASE_URL` to a self-hosted or commercial Open-Meteo endpoint. Place names and coordinates the agent asks about are sent to that service. |
| `GMAIL` / `CALENDAR` | `off` by default. `on` offers the Gmail or Calendar tools for one Google account, which also needs `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`, and `GOOGLE_REFRESH_TOKEN` (see "Gmail and Calendar" in the README). `GOOGLE_BACKEND` picks Google's REST APIs (`api`, the default, for any account) or its MCP servers (`mcp`, only for Cloud projects in the Workspace Developer Preview). Mail and calendar contents the agent reads are stored with the run and sent to the model. The default tools can't send, delete, or change anything. |
| `MAPS` | `on` by default. `off` removes `show_map`. It finds places by name with the same Open-Meteo geocoding endpoint (`WEATHER_GEOCODING_BASE_URL`), so the same non-commercial terms apply. |

## 5. Try it by hand first

Run each service in the foreground to confirm everything is wired up before using systemd. Use
two terminals on the Spark.

```sh
# terminal 1
sudo -u cedar env $(sudo cat /etc/cedar/cedar.env | grep -v '^#' | xargs) /usr/local/bin/cedar
```

```sh
# terminal 2
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/readyz     # 200

CEDAR_KEY=$(sudo grep ^CEDAR_API_KEYS= /etc/cedar/cedar.env | cut -d= -f2)
MODEL=$(curl -s http://127.0.0.1:8000/v1/models | jq -r '.data[0].id')

# Register vLLM as the model "deepseek" (cedar lists the server's models first to check the
# address). DeepSeek V4 takes these reasoning levels; leave the list out for a model that has none.
curl -s -X PUT http://127.0.0.1:8080/v1/admin/models/deepseek -H "Authorization: Bearer $CEDAR_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"base_url": "http://127.0.0.1:8000/v1", "model": "'"$MODEL"'", "label": "DeepSeek V4",
       "reasoning_levels": ["none", "low", "high", "max"]}'

curl -s http://127.0.0.1:8080/v1/runs -H "Authorization: Bearer $CEDAR_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"prompt": "Write hello.txt containing the word hi, then read it back."}' | tee /tmp/run.json
RUN=$(jq -r .id /tmp/run.json)
curl -N http://127.0.0.1:8080/v1/runs/$RUN/events -H "Authorization: Bearer $CEDAR_KEY"
```

`/readyz` returns 200 only if the database and Docker (including the `cedar-sandbox` image) are
available. It does not check vLLM, but it says how many models are registered (`"models": 1`
after the `PUT`). The registration is kept in cedar's database, so it survives restarts; the chat
UI shows and edits it under Settings → Server. The event stream should show
`tool.call`, `tool.result`, `assistant.text`, and finally `run.completed`. If it ends in
`run.failed`, see [Troubleshooting](#troubleshooting).

**Reply length.** Each model caps one reply at its `max_tokens`, 16384 unless set (add
`"max_tokens": 8192` to the `PUT` above, or change **Max tokens per reply** under Settings →
Server). A model that reasons before it answers spends this on its thinking too, so a reply can
end before any text; chat then says it was cut off at the limit. Raise it if you see that. cedar
waits for the full reply and gives up on a single request after **300 seconds** (fixed in
`src/llm.rs`). If your Spark generates slowly enough that the limit takes longer than that, lower
it. The old `MAX_TOKENS` setting is ignored, with a warning in the log.

Stop cedar with Ctrl-C and move on to systemd.

## 6. Run both services under systemd

```sh
sudo tee /etc/systemd/system/cedar.service >/dev/null <<'EOF'
[Unit]
Description=cedar agent server
After=network-online.target docker.service
Wants=network-online.target
Requires=docker.service

[Service]
User=cedar
Group=cedar
EnvironmentFile=/etc/cedar/cedar.env
WorkingDirectory=/var/lib/cedar
ExecStart=/usr/local/bin/cedar
Restart=on-failure
RestartSec=5
# On SIGTERM cedar cancels active runs and gives them up to 10 s to record their state.
TimeoutStopSec=30

[Install]
WantedBy=multi-user.target
EOF

sudo tee /etc/systemd/system/cedar-chat.service >/dev/null <<'EOF'
[Unit]
Description=cedar chat UI
After=network-online.target cedar.service
Wants=cedar.service

[Service]
User=cedar
Group=cedar
EnvironmentFile=/etc/cedar/chat.env
WorkingDirectory=/var/lib/cedar
ExecStart=/usr/local/bin/cedar-chat
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable --now cedar cedar-chat
systemctl status cedar cedar-chat --no-pager
```

cedar does not check the LLM at startup, so it comes up fine even if vLLM is still loading. Runs
started before vLLM is ready fail after a few retries. If vLLM is also a systemd unit, order cedar
after it:

```sh
sudo systemctl edit cedar      # add:  [Unit]  After=vllm.service  Wants=vllm.service
```

## 7. Open the UI from your laptop and phone

`cedar-chat` listens on the Spark's loopback only. The safest way in is an SSH tunnel. Run this on
your laptop:

```sh
ssh -N -L 3000:127.0.0.1:3000 <your-user>@<spark-hostname-or-ip>
```

Then open <http://localhost:3000> and enter the chat access token from step 4.

To skip the tunnel, bind to the network instead: set `CHAT_BIND_ADDR=0.0.0.0:3000` in
`/etc/cedar/chat.env` and restart `cedar-chat`. Only do this if:

- `CHAT_ACCESS_TOKEN` is set (anyone who can reach the port can otherwise run an agent that executes
  commands), and
- you are on a network you trust, or you put a TLS-terminating proxy in front. cedar-chat speaks
  plain HTTP, so on a shared network the token travels unencrypted. If the proxy is nginx, add
  `proxy_buffering off` or live replies will stall, because the UI streams them.

Leave `cedar` on `127.0.0.1:8080`. Only `cedar-chat` talks to it.

<a id="over-https-with-tailscale-the-ios-app"></a>

### Over HTTPS with Tailscale (the iOS and Android apps)

The iOS and Android apps only connect to servers they can reach over HTTPS. The simplest way to give
`cedar-chat` a trusted certificate without exposing it to the internet is
[Tailscale](https://tailscale.com): `tailscale serve` terminates TLS with a Let's Encrypt
certificate for the Spark's tailnet name and proxies to `cedar-chat` on loopback. Only devices on
your tailnet can reach it, and `CHAT_BIND_ADDR` stays `127.0.0.1:3000`.

1. Install Tailscale on the Spark and log in (it needs version 1.52 or newer for `--bg`):

   ```sh
   curl -fsSL https://tailscale.com/install.sh | sh
   sudo tailscale up
   ```

2. In the [admin console's DNS page](https://login.tailscale.com/admin/dns), turn on **MagicDNS**
   and **HTTPS Certificates**. Without them `tailscale serve` can't get a certificate (it prints
   a link to the setting and waits).

3. Serve `cedar-chat` on the tailnet. `--bg` makes the setting persistent: it survives reboots and
   Tailscale restarts, so this is a one-time step.

   ```sh
   sudo tailscale serve --bg --https=443 http://127.0.0.1:3000
   tailscale serve status        # prints https://<spark>.<tailnet>.ts.net
   ```

4. Tell `cedar-chat` its public address, so links it sends (push notifications, webhooks,
   "Connect Google") point at it, and restart it:

   ```sh
   URL=https://$(tailscale status --json | jq -r '.Self.DNSName | rtrimstr(".")')
   echo "CHAT_PUBLIC_URL=$URL" | sudo tee -a /etc/cedar/chat.env
   sudo systemctl restart cedar-chat
   ```

5. Install Tailscale on the iPhone or Android phone and log in to the same tailnet. In the cedar
   app's Settings (iOS, or the Android app from GitHub Releases), tap **Scan QR Code** and scan
   the code this prints (it needs `sudo apt install qrencode`):

   ```sh
   TOKEN=$(sudo sed -n 's/^CHAT_ACCESS_TOKEN=//p' /etc/cedar/chat.env)
   qrencode -t ansiutf8 "cedar://connect?server=$(jq -rn --arg v "$URL" '$v|@uri')&token=$TOKEN"
   ```

   The app fills in the address and token and tests the connection. Or enter
   `https://<spark>.<tailnet>.ts.net` (no port) and the chat access token yourself, then tap
   **Test Connection**. The same URL works in any browser on the tailnet. Anyone who scans the
   code can use your cedar, so don't leave it on screen.

To stop serving: `sudo tailscale serve --https=443 off`.

`tailscale serve` passes streamed replies through without buffering, so live replies work as
they do on loopback. The access token is still required: anyone on your tailnet can reach the
page. To limit that, restrict the Spark with [tailnet ACLs](https://tailscale.com/kb/1018/acls).

If you run the stack with `scripts/cedar-stack.sh` instead of systemd, it does steps 3 and 4 for
you whenever Tailscale is logged in: it serves the port from `CHAT_BIND_ADDR`, passes the address
to `cedar-chat` as `CHAT_PUBLIC_URL`, prints it, and turns serving off again on `--down`. Set `TAILSCALE_SERVE=off`
in `.env` to skip it, `TAILSCALE_SERVE=on` to fail when it can't serve, or `TAILSCALE_HTTPS_PORT`
to use another tailnet port than 443. If it fails with "access denied", let your user manage
Tailscale once with `sudo tailscale set --operator=$USER`. For step 5, `scripts/cedar-stack.sh --pair`
prints the QR code (the tailnet address, or `CHAT_PUBLIC_URL` when it is HTTPS, and
`CHAT_ACCESS_TOKEN` from `.env`); install `qrencode` for it, or it prints the link instead.

## Day-to-day

```sh
journalctl -u cedar -u cedar-chat -f           # logs
sudo systemctl restart cedar cedar-chat        # restart
```

**Update:**

```sh
cd ~/cedar && git pull
cargo build --release --workspace
docker build -t cedar-sandbox sandbox/
sudo install -m 0755 target/release/cedar target/release/cedar-chat /usr/local/bin/
sudo systemctl restart cedar cedar-chat
```

Restarting `cedar` ends any run in progress (it is marked `failed`), so do it when nothing is
working. Finished conversations are kept.

### Updates

The chat checks GitHub for a newer release every six hours and shows a banner when there is one
(`CHAT_UPDATE_CHECK=off` in `chat.env` turns that off). Without more setup its *How to update*
shows the commands to run. To have its **Update** button install the release, set up the
`cedar-update` unit once. It runs as root, because the binaries and the services are root's,
and only when the chat asks: an admin's *Update* writes the requested version to
`/var/lib/cedar/update/request`, a systemd path unit starts `cedar-update`, which downloads that
release's tarball and `checksums.txt` from GitHub, checks the tarball against it, installs
`cedar` and `cedar-chat` (keeping the old ones as `*.prev`), restarts both services, and puts the
old binaries back if cedar doesn't come up healthy. The banner shows each step. From the
unpacked release tarball (or `scripts/` in a checkout, where the script is `cedar-update.sh`):

```sh
sudo install -m 0755 cedar-update /usr/local/sbin/cedar-update
sudo install -m 0644 cedar-update.path cedar-update.service /etc/systemd/system/
sudo install -d -o cedar -g cedar -m 0750 /var/lib/cedar/update
echo "CHAT_UPDATE_DIR=/var/lib/cedar/update" | sudo tee -a /etc/cedar/chat.env
sudo systemctl daemon-reload
sudo systemctl enable --now cedar-update.path
sudo systemctl restart cedar-chat
```

The chat only passes a version on; the unit checks it is a release version newer than the one
installed and builds the download address itself. It needs `curl` and outbound HTTPS to
`github.com`. Starting an update from the page needs the admin token (`CHAT_ADMIN_TOKEN`).
The same works from a shell:

```sh
sudo cedar-update --check    # installed and latest version
sudo cedar-update            # install the latest release
sudo cedar-update 0.1.9      # install that one (--force to go back to an older one)
journalctl -u cedar-update   # what the last run did
```

The sandbox image is left alone, since step 3 builds it from `sandbox/`. To use the published
one instead, put `CEDAR_UPDATE_PULL_IMAGE=1` in `/etc/cedar/update.env`: the unit then pulls
`ghcr.io/hanneshapke/cedar-sandbox:<version>` and tags it `cedar-sandbox` (or
`CEDAR_SANDBOX_IMAGE`) before restarting.

**Backups.** State is two SQLite files in `/var/lib/cedar/`: `cedar.db` (runs, conversations,
events) and `cedar-chat.db` (the chat sidebar). Both use WAL, so copy them with SQLite rather
than `cp`:

```sh
sudo apt install -y sqlite3
sudo -u cedar sqlite3 /var/lib/cedar/cedar-chat.db ".backup '/var/lib/cedar/cedar-chat.bak'"
```

Agent workspaces are Docker volumes named `cedar-ws-*`. cedar deletes them after
`WORKSPACE_TTL_SECS` (24 h by default), so don't treat them as storage.

## Troubleshooting

| Symptom | Likely cause and fix |
| --- | --- |
| Message refused: "no model is registered" | Nobody has registered the vLLM server yet: the `PUT` in step 5, or Settings → Server → Add a model in the browser (with the admin token from step 4). |
| Run fails: `LLM API error 404` / "model not found" | The model name in the registered entry doesn't match the name vLLM registered. Compare with `curl localhost:8000/v1/models` and edit the entry (Settings shows what the server listed). |
| Run fails immediately with `sending API request` | cedar can't reach vLLM. Check it is running and that the entry's address includes `/v1`. |
| Runs finish instantly with tool calls printed as text | vLLM was started without `--enable-auto-tool-choice --tool-call-parser deepseek_v4`. Redo the check in step 2. |
| Run fails: `run exceeded the Ns time limit` | The whole run took longer than `RUN_TIMEOUT_SECS`. Raise it, or lower the model's **Max tokens per reply** so each model turn is shorter. |
| Run fails: `run did not finish within N turns` | The task needed more model turns than allowed. Raise `max_turns` in the request (capped by `MAX_TURNS_CAP`, 25 by default) or give the agent a smaller task. |
| Run fails: `sending API request` after a long wait | A single model reply took longer than the 300 s request limit (see "Reply length" above), and cedar retried it. Lower the model's **Max tokens per reply**. |
| `/readyz` returns 503 | Docker isn't reachable by the `cedar` user (`id cedar` should list `docker`) or the `cedar-sandbox` image is missing. The log says which. |
| Chat page shows a 502 error | `cedar-chat` can't reach cedar, or `CEDAR_API_KEY` in `chat.env` doesn't match `CEDAR_API_KEYS` in `cedar.env`. |
| Chat page asks for a token and rejects it | Use the value of `CHAT_ACCESS_TOKEN` from `/etc/cedar/chat.env`, not the cedar key. |
| The iOS or Android app can't connect | It needs HTTPS: use the `https://<spark>.<tailnet>.ts.net` address from step 7, not `http://…:3000`, and check that the phone is logged in to Tailscale (`tailscale serve status` on the Spark shows what is served). |
| `tailscale serve` waits or says HTTPS is not enabled | Turn on MagicDNS and HTTPS Certificates in the Tailscale admin console (step 7). |
| The whole machine slows down or freezes during a run | Memory. See the memory note in step 1: lower `MAX_CONCURRENT_RUNS`, or use a smaller model or quant. |
