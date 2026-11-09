# Running Cedar with Nemotron 3.5 Super VL on a DGX Spark

This guide sets up a private Cedar server on one DGX Spark, with vLLM serving NVIDIA Nemotron 3.5 Super VL. Super VL is a 120B-parameter model with 12B active parameters, up to 1M tokens of context, and support for text, images, video, reasoning, and tool calls.

[Cedar](https://getcedar.io) has two programs: `cedar`, the agent server, and `cedar-chat`, the web interface. Each run uses tools inside a Docker sandbox.

```
Your computer -> SSH tunnel or Tailscale -> cedar-chat :3000 -> cedar :8080 -> vLLM :8000
                                                                  |
                                                                  -> Docker sandbox
```

The performance figures below are from [Classmethod's DGX Spark test](https://dev.classmethod.jp/en/articles/dgx-spark-nemotron-3-5-super-vl-first-touch/), which used an earlier vLLM nightly. Your results on `v0.32.0` may differ.

## What you need

- DGX Spark with 128GB of unified memory
- About 90GB of free disk space for the weights and container image
- Docker with NVIDIA GPU support
- SSH access from your computer

Super VL effectively takes over the Spark. In Classmethod's test, resident memory was close to 100 GiB, so don't run another model beside it.

## 1. Download the model

Install `uv` if you don't already have it:

```
curl -LsSf https://astral.sh/uv/install.sh | sh
source "$HOME/.local/bin/env"
```

Download the NVFP4 weights and the LiLiCorr drafter, which the model card recommends for every DGX Spark deployment:

```
sudo install -d -o "$USER" -g "$USER" /opt/models

uvx --from huggingface_hub hf download \
  nvidia/NVIDIA-Nemotron-3.5-Super-VL-120B-A12B-NVFP4 \
  --local-dir /opt/models/super-vl-nvfp4

uvx --from huggingface_hub hf download \
  nvidia/NVIDIA-Nemotron-3.5-Super-VL-120B-A12B-NVFP4-LiLiCorr \
  --local-dir /opt/models/super-vl-lilicorr

du -sh /opt/models/super-vl-nvfp4 /opt/models/super-vl-lilicorr
```

Expect about 70 GiB and 1.2 GiB.

## 2. Pull the vLLM image

```
docker pull vllm/vllm-openai:v0.32.0
```

The model card also names a pinned nightly image, but that tag has since disappeared from Docker Hub. Use `v0.32.0`.

## 3. Run vLLM as a service

If another model already runs under `vllm.service`, follow [Switch from another model](#switch-from-another-model) instead of creating the unit below.

Create the start script. It runs Docker in the foreground because the systemd unit uses `Type=simple`:

```
sudo tee /usr/local/bin/vllm-supervl.sh >/dev/null <<'EOF'
#!/bin/bash
exec /usr/bin/docker run --name vllm --gpus all --ipc=host \
  -p 127.0.0.1:8000:8000 \
  -v /opt/models/super-vl-nvfp4:/model:ro \
  -v /opt/models/super-vl-lilicorr:/draft:ro \
  vllm/vllm-openai:v0.32.0 /model \
  --served-model-name nemotron-3.5-super-vl \
  --max-num-seqs 1 \
  --trust-remote-code \
  --gpu-memory-utilization 0.75 \
  --max-num-batched-tokens 8192 \
  --speculative_config.model /draft \
  --speculative_config.num_speculative_tokens 7 \
  --speculative_config.method dflash \
  --mamba-backend flashinfer \
  --mamba-ssm-cache-dtype float16 \
  --enable-mamba-cache-stochastic-rounding \
  --mamba-cache-philox-rounds 5 \
  --enable-mamba-shared-prefix-checkpoint \
  --video-pruning-rate 0.5 \
  --media-io-kwargs '{"video": {"fps": 2, "num_frames": 256}}' \
  --block-size 64 --prefix-match-unit 64 \
  --enable-auto-tool-choice --tool-call-parser qwen3_coder \
  --reasoning-parser nemotron_v3
EOF
sudo chmod 755 /usr/local/bin/vllm-supervl.sh
```

Create the service:

```
sudo tee /etc/systemd/system/vllm.service >/dev/null <<'EOF'
[Unit]
Description=vLLM - Nemotron 3.5 Super VL NVFP4
After=network-online.target docker.service
Wants=network-online.target
Requires=docker.service

[Service]
Type=simple
ExecStartPre=-/usr/bin/docker rm -f vllm
ExecStart=/usr/local/bin/vllm-supervl.sh
ExecStartPost=/bin/sh -c 'until curl -sf http://127.0.0.1:8000/health >/dev/null; do sleep 5; done'
ExecStop=/usr/bin/docker stop -t 30 vllm
Restart=always
RestartSec=15
TimeoutStartSec=1800
TimeoutStopSec=60

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable vllm
sudo systemctl start --no-block vllm
until docker inspect vllm >/dev/null 2>&1; do sleep 2; done
docker logs -f vllm
```

Loading took about 11 minutes in Classmethod's test. Press `Ctrl+C` to stop following the logs. This doesn't affect the server.

Keep vLLM on `127.0.0.1`, because Cedar runs on the same machine.

## 4. Check the model

Confirm that systemd is running the right script and that speculative decoding loaded:

```
systemctl cat vllm | grep -E '^(Description|ExecStart)='
docker inspect vllm --format '{{.Config.Image}} {{.State.StartedAt}}'
docker logs vllm 2>&1 | grep -E 'LiLiCorrDraftModel|SpeculativeConfig|Maximum concurrency'
curl -s http://127.0.0.1:8000/v1/models | jq -r '.data[].id'
```

The model ID should be `nemotron-3.5-super-vl`. The logs should show `LiLiCorrDraftModel` and `method='dflash'`.

Cedar acts through tool calls, so test those before installing it:

```
curl -s http://127.0.0.1:8000/v1/chat/completions -H 'Content-Type: application/json' -d '{
  "model": "nemotron-3.5-super-vl",
  "max_tokens": 4096,
  "messages": [{"role": "user", "content": "Run the command: echo hi"}],
  "tools": [{"type": "function", "function": {
    "name": "bash",
    "description": "Run a shell command",
    "parameters": {
      "type": "object",
      "properties": {"command": {"type": "string"}},
      "required": ["command"]
    }
  }}]
}' | jq '.choices[0] | {finish_reason, tool_calls: .message.tool_calls}'
```

You want `finish_reason` to be `tool_calls`, with a non-empty `tool_calls` array.

To confirm that the drafter is being used, check that this counter goes up after a request:

```
curl -s http://127.0.0.1:8000/metrics | grep spec_decode_num_drafts_total
```

## 5. Install Cedar

Confirm that you're on ARM and have a new enough glibc:

```
uname -m
ldd --version | head -1
```

`uname -m` should print `aarch64`, and glibc should be 2.35 or newer. Check the [releases page](https://github.com/hanneshapke/cedar/releases) for the latest version, then change `V` if needed:

```
V=0.3.6
F=Cedar-$V-linux-arm64
U=https://github.com/hanneshapke/cedar/releases/download
curl -fLO $U/v$V/$F.tar.gz
tar -xzf $F.tar.gz
sudo install $F/cedar* /usr/local/bin/
```

Build the image that Cedar's tools run in:

```
git clone --depth 1 --branch v$V https://github.com/hanneshapke/cedar.git ~/cedar
docker build -t cedar-sandbox ~/cedar/sandbox/
```

### Create the configuration

Create a service account. The `cedar` account is in the `docker` group, which is effectively root on this machine, so use it only to run Cedar.

```
sudo useradd --system --create-home --home-dir /var/lib/cedar \
  --shell /usr/sbin/nologin --groups docker cedar
```

Run this as one block, because later commands use the generated values:

```
CEDAR_KEY=$(openssl rand -hex 24)
CHAT_TOKEN=$(openssl rand -hex 24)
ADMIN_TOKEN=$(openssl rand -hex 24)
SECRET_KEY=$(openssl rand -hex 32)

sudo install -d -m 0755 /etc/cedar

sudo tee /etc/cedar/cedar.env >/dev/null <<EOF
CEDAR_API_KEYS=$CEDAR_KEY
CEDAR_ADMIN_KEYS=$CEDAR_KEY
CEDAR_SECRET_KEY=$SECRET_KEY
BIND_ADDR=127.0.0.1:8080
DATABASE_URL=sqlite:///var/lib/cedar/cedar.db
MAX_CONCURRENT_RUNS=1
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

sudo chmod 600 /etc/cedar/cedar.env /etc/cedar/chat.env
echo "Chat access token: $CHAT_TOKEN"
echo "Admin token: $ADMIN_TOKEN"
```

Save both tokens in your password manager.

`MAX_CONCURRENT_RUNS=1` matches vLLM's `--max-num-seqs 1`. Local replies are slow enough that each run may need the longer timeout.

### Run Cedar as a service

```
sudo tee /etc/systemd/system/cedar.service >/dev/null <<'EOF'
[Unit]
Description=cedar agent server
After=network-online.target docker.service vllm.service
Wants=network-online.target vllm.service
Requires=docker.service

[Service]
User=cedar
Group=cedar
EnvironmentFile=/etc/cedar/cedar.env
WorkingDirectory=/var/lib/cedar
ExecStart=/usr/local/bin/cedar
Restart=on-failure
RestartSec=5
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

### Register the model and test Cedar

```
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/readyz

CEDAR_KEY=$(sudo grep ^CEDAR_API_KEYS= /etc/cedar/cedar.env | cut -d= -f2)

curl -s -X PUT http://127.0.0.1:8080/v1/admin/models/nemotron-super-vl \
  -H "Authorization: Bearer $CEDAR_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"base_url": "http://127.0.0.1:8000/v1", "model": "nemotron-3.5-super-vl", "label": "Nemotron 3.5 Super VL", "max_tokens": 8192, "is_default": true}'
```

The first command should print `200`.

The `max_tokens` value is lower than Cedar's default of 16,384 because Cedar gives up on one model reply after 300 seconds. At the 38 tok/s Classmethod measured, 8,192 tokens take about 215 seconds. Reasoning counts toward this limit, so a reply can end before any answer appears.

Leave `reasoning_levels` out. Cedar sends those levels as `reasoning_effort`, while the model card controls Super VL's reasoning with `chat_template_kwargs`.

Start a test run:

```
curl -s http://127.0.0.1:8080/v1/runs -H "Authorization: Bearer $CEDAR_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"prompt": "Write hello.txt containing the word hi, then read it back."}' | tee /tmp/run.json

RUN=$(jq -r .id /tmp/run.json)
curl -N http://127.0.0.1:8080/v1/runs/$RUN/events -H "Authorization: Bearer $CEDAR_KEY"
```

Look for `tool.call`, `tool.result`, and `run.completed`.

### Images and PDF previews

The steps above register the model as text-only. vLLM's default limit is one image and one video per request, and that limit covers the whole conversation Cedar resends on each turn. If you register the model as `multimodal`, a second image in a conversation is refused, and so is a PDF preview of more than one page.

To try images, add these flags to `/usr/local/bin/vllm-supervl.sh`:

```
--limit-mm-per-prompt '{"image": 16, "video": 0}' \
--max-model-len 262144 \
```

Then restart vLLM, and add `"multimodal": true` to the model registration. The shorter context leaves memory for the larger image allowance, because a [forum report](https://forums.developer.nvidia.com/t/385969) found that raising the limit at the full 1M context stopped vLLM from starting. This combination hasn't been tested with Cedar.

## 6. Open Cedar from your computer

Create an SSH tunnel from your computer:

```
ssh -N -L 3000:127.0.0.1:3000 yourusername@SPARK_IP
```

Then open [http://localhost:3000](http://localhost:3000) and enter your chat access token.

If someone can reach Cedar's chat, they can run an agent that executes commands, so don't expose port 3000 to the internet. For phone access over HTTPS, follow the [Tailscale steps in the deployment guide](https://github.com/hanneshapke/cedar/blob/main/DEPLOYMENT.md#over-https-with-tailscale-the-ios-and-android-apps).

## 7. Maintain the server

Follow the logs:

```
journalctl -u vllm -u cedar -u cedar-chat -f
docker logs -f vllm
```

Restarting vLLM takes the model offline for the full load time:

```
sudo systemctl restart --no-block vllm
```

Restarting Cedar ends any active run, so do it when nothing is working.

### Switch from another model

If `vllm.service` already serves another model, a hand-started container is not enough. systemd can bring the old model back after a crash or reboot, and it deletes any container named `vllm` when it starts.

Create `/usr/local/bin/vllm-supervl.sh` from section 3, then update the existing unit:

```
sudo cp /etc/systemd/system/vllm.service /etc/systemd/system/vllm.service.previous
sudo sed -i \
  -e 's#^Description=.*#Description=vLLM - Nemotron 3.5 Super VL NVFP4#' \
  -e 's#^ExecStart=.*#ExecStart=/usr/local/bin/vllm-supervl.sh#' \
  /etc/systemd/system/vllm.service

sudo systemctl daemon-reload
sudo systemctl restart --no-block vllm
```

This assumes the unit uses the same `vllm` container name as section 3. Confirm the change with the checks in section 4. In Cedar, update the registered model name, or register the new model as the default.

To go back:

```
sudo cp /etc/systemd/system/vllm.service.previous /etc/systemd/system/vllm.service
sudo systemctl daemon-reload
sudo systemctl restart --no-block vllm
```

## Troubleshooting

| Problem | What to check |
|---|---|
| `docker pull` says `manifest unknown` | Use `vllm/vllm-openai:v0.32.0`. The nightly tag in the model card is gone. |
| Port 8000 is `already in use` | Another vLLM is still running. Run `sudo ss -ltnp \| grep ':8000'`, and stop it through systemd if systemd manages it. |
| A restart brings back the old model | Run `systemctl cat vllm \| grep ExecStart`. The unit is still pointing to the old script, or you restored the backup. |
| `docker logs` says `No such container: vllm` | systemd hasn't created it yet. Wait a few seconds, or check `journalctl -u vllm -n 50 --no-pager`. |
| systemd warns that the unit file changed on disk | Run `sudo systemctl daemon-reload`. |
| vLLM fails with a KV cache or memory error | Stop anything else using the GPU, then add `--max-model-len 262144`. |
| The Spark runs out of memory during inference | Lower `--gpu-memory-utilization` to `0.7`, add `--max-model-len 262144`, and keep other models off the machine. |
| The machine shuts off or runs hot on long prompts | Forum users reported this on inputs of 32K tokens or more. Watch `/sys/class/thermal` as well as `nvidia-smi`. |
| Runs show tool calls as plain text | Confirm that vLLM has `--enable-auto-tool-choice --tool-call-parser qwen3_coder`. |
| A reply ends with no answer | Reasoning used the whole `max_tokens` budget. Raise it carefully, keeping each reply under 300 seconds. |
| A run fails after a long wait with `sending API request` | One reply exceeded Cedar's 300-second limit. Lower `max_tokens`. |
| A run reaches the time limit | Raise `RUN_TIMEOUT_SECS`, or lower `max_tokens`. |
| A run with images or PDFs fails | Follow [Images and PDF previews](#images-and-pdf-previews). |
| Cedar says the model isn't found | Compare the registered name with `curl -s http://127.0.0.1:8000/v1/models`. |
| `/readyz` returns `503` | Run `id cedar` to confirm it's in the `docker` group, and `docker images` to confirm `cedar-sandbox` exists. |

## Sources

- [Cedar](https://getcedar.io)
- [Cedar README](https://github.com/hanneshapke/cedar)
- [Cedar DGX Spark deployment guide](https://github.com/hanneshapke/cedar/blob/main/DEPLOYMENT.md)
- [NVIDIA Nemotron 3.5 Super VL technical blog](https://developer.nvidia.com/blog/nvidia-nemotron-3-5-super-vl-powers-specialized-agents-with-multimodal-reasoning/)
- [Nemotron 3.5 Super VL NVFP4 model card](https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Super-VL-120B-A12B-NVFP4)
- [LiLiCorr drafter](https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Super-VL-120B-A12B-NVFP4-LiLiCorr)
- [Classmethod: Nemotron 3.5 Super VL on DGX Spark](https://dev.classmethod.jp/en/articles/dgx-spark-nemotron-3-5-super-vl-first-touch/)
- [NVIDIA Developer Forums: Super VL on DGX Spark](https://forums.developer.nvidia.com/t/385969)
