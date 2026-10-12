# Deploying cedar and the chat UI on a DGX Spark

This guide sets up the whole stack on one NVIDIA DGX Spark and shows how to open the chat UI from
your laptop.

```
laptop browser ──SSH tunnel──▶ cedar-chat :3000 ──▶ cedar :8080 ──▶ vLLM :8000 (DeepSeek V4)
                                                       │
                                                       └──▶ one Docker container per run (the agent's tools)
```

| Service | What it is | Listens on | Started by |
| --- | --- | --- | --- |
| vLLM | Serves the model over an OpenAI-compatible API | `127.0.0.1:8000` | you (see [step 2](#2-make-sure-vllm-is-ready-for-cedar)) |
| `cedar` | The agent server: runs the loop and the sandboxes | `127.0.0.1:8080` | systemd |
| `cedar-chat` | The web UI | `127.0.0.1:3000` | systemd |

Only `cedar-chat` needs to be reachable from outside the Spark. Keep `cedar` and vLLM on
loopback.

If vLLM runs on another machine, skip step 2 and point `LLM_BASE_URL` at it in step 4.

## 1. Prepare the Spark

The Spark is an ARM64 (aarch64) machine running DGX OS. Everything here builds natively on it.

```sh
sudo apt update
sudo apt install -y build-essential git curl jq openssl

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

Whatever it prints is your `LLM_MODEL` in step 4. Then check that tool calling works, which is
the check that matters most:

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

# The image the agent's tools run in (a small Debian with python3, git, curl, jq)
docker build -t cedar-sandbox sandbox/
```

The chat UI's HTML, CSS, and JavaScript are compiled into the `cedar-chat` binary, so there is
nothing else to deploy.

The sandbox has no network, so the agent can't install packages. If your tasks need more tools,
add them to `sandbox/Dockerfile` and rebuild the image.

## 4. Configure

Create a service user, a state directory, and the two environment files. The user is in the
`docker` group, which is effectively root on this machine, so keep this account to running cedar.

```sh
sudo useradd --system --create-home --home-dir /var/lib/cedar \
  --shell /usr/sbin/nologin --groups docker cedar

CEDAR_KEY=$(openssl rand -hex 24)     # chat -> cedar
CHAT_TOKEN=$(openssl rand -hex 24)    # browser -> chat
MODEL=$(curl -s http://127.0.0.1:8000/v1/models | jq -r '.data[0].id')   # or type the name yourself
echo "model: ${MODEL:?vLLM is not reachable; set MODEL to the name it serves}"

sudo install -d -m 0755 /etc/cedar

sudo tee /etc/cedar/cedar.env >/dev/null <<EOF
CEDAR_API_KEYS=$CEDAR_KEY
BIND_ADDR=127.0.0.1:8080
DATABASE_URL=sqlite:///var/lib/cedar/cedar.db

LLM_BASE_URL=http://127.0.0.1:8000/v1
LLM_MODEL=$MODEL
# LLM_API_KEY=            # only if vLLM was started with --api-key

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
EOF

sudo chmod 600 /etc/cedar/cedar.env /etc/cedar/chat.env   # systemd reads them as root
echo "Chat access token (you type this in the browser): $CHAT_TOKEN"
```

Keep the printed token. The chat page asks for it on first load. The full list of settings is in
the [README](README.md#configuration) and [chat/README.md](chat/README.md#configuration).

Settings worth knowing on this hardware:

| Setting | Why |
| --- | --- |
| `RUN_TIMEOUT_SECS` | A whole run, across all its model turns, must finish within this (default 600). A local model is slower than a hosted one, so give it room. |
| `MAX_TOKENS` | Cap on one model reply (default 4096). cedar waits for the full reply and gives up on a single request after **300 seconds** (fixed in `src/llm.rs`). If your Spark generates slowly enough that 4096 tokens takes longer than that, lower `MAX_TOKENS`. |
| `MAX_CONCURRENT_RUNS` | How many agents run at once; the rest queue. |
| `CHAT_MODELS` | Optional model picker in the UI. Leave unset to use only `LLM_MODEL`. |

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
curl -s http://127.0.0.1:8080/v1/runs -H "Authorization: Bearer $CEDAR_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"prompt": "Write hello.txt containing the word hi, then read it back."}' | tee /tmp/run.json
RUN=$(jq -r .id /tmp/run.json)
curl -N http://127.0.0.1:8080/v1/runs/$RUN/events -H "Authorization: Bearer $CEDAR_KEY"
```

`/readyz` returns 200 only if the database and Docker (including the `cedar-sandbox` image) are
available. It does not check vLLM. The event stream should show `tool.call`, `tool.result`,
`assistant.text`, and finally `run.completed`. If it ends in `run.failed`, see
[Troubleshooting](#troubleshooting).

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

## 7. Open the UI from your laptop

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
| Run fails: `LLM API error 404` / "model not found" | `LLM_MODEL` doesn't match the name vLLM registered. Compare with `curl localhost:8000/v1/models`. Runs created earlier keep their old model name, so continuing one needs an explicit `model`. |
| Run fails immediately with `sending API request` | cedar can't reach vLLM. Check it is running and that `LLM_BASE_URL` includes `/v1`. |
| Runs finish instantly with tool calls printed as text | vLLM was started without `--enable-auto-tool-choice --tool-call-parser deepseek_v4`. Redo the check in step 2. |
| Run fails: `run exceeded the Ns time limit` | The whole run took longer than `RUN_TIMEOUT_SECS`. Raise it, or lower `MAX_TOKENS` so each model turn is shorter. |
| Run fails: `run did not finish within N turns` | The task needed more model turns than allowed. Raise `max_turns` in the request (capped by `MAX_TURNS_CAP`, 25 by default) or give the agent a smaller task. |
| Run fails: `sending API request` after a long wait | A single model reply took longer than the 300 s request limit (see `MAX_TOKENS` above), and cedar retried it. Lower `MAX_TOKENS`. |
| `/readyz` returns 503 | Docker isn't reachable by the `cedar` user (`id cedar` should list `docker`) or the `cedar-sandbox` image is missing. The log says which. |
| Chat page shows a 502 error | `cedar-chat` can't reach cedar, or `CEDAR_API_KEY` in `chat.env` doesn't match `CEDAR_API_KEYS` in `cedar.env`. |
| Chat page asks for a token and rejects it | Use the value of `CHAT_ACCESS_TOKEN` from `/etc/cedar/chat.env`, not the cedar key. |
| The whole machine slows down or freezes during a run | Memory. See the memory note in step 1: lower `MAX_CONCURRENT_RUNS`, or use a smaller model or quant. |
