# Running Cedar on an AMD Ryzen AI Max (Strix Halo) mini PC

This guide turns a mini PC with an AMD Ryzen AI Max+ 395 ("Strix Halo") and 128GB of unified memory into a private, remote Cedar server. Ubuntu Server runs headless, Ollama serves `gpt-oss:120b` on the Radeon 8060S integrated GPU, and you use Cedar over SSH or Tailscale.

It follows the same pattern as the [Jetson Orin Nano developer story](cedar-on-jetson-orin-nano.md), but with roughly 16 times the memory, so you can run a 120B-parameter model instead of a 4B one.

[Cedar](https://getcedar.io) has two programs: `cedar`, the agent server, and `cedar-chat`, the web interface. Each run uses tools inside a Docker sandbox.

```text
Your computer -> SSH tunnel or Tailscale -> cedar-chat :3000 -> cedar :8080 -> Ollama :11434
                                                                  |
                                                                  -> Docker sandbox
```

## What you need

- A mini PC or desktop with a Ryzen AI Max+ 395 (or another Ryzen AI Max) and 128GB of memory, such as the Framework Desktop, HP Z2 Mini G1a, or GMKtec EVO-X2
- A USB stick (8GB or larger) for the Ubuntu installer
- Ethernet connection to your router
- Monitor, keyboard, and mouse for the installation
- About 100GB of free disk space for the model, Docker images, and Cedar

A 64GB machine works with the same steps. Use a smaller model, such as `qwen3.6:35b-a3b`, and give the GPU less memory in section 3.

## Why mixture-of-experts models

The Ryzen AI Max shares one pool of LPDDR5X memory between the CPU and GPU. That lets it hold large models, but its memory bandwidth (about 256 GB/s) is lower than a DGX Spark's or a Mac Max's. Generation speed depends mostly on how many parameters the model reads per token, so mixture-of-experts (MoE) models run several times faster here than dense models of the same size.

`gpt-oss:120b` has 117B parameters but only about 5B active per token, so it fits comfortably and stays responsive. Cedar's [install guide](https://getcedar.io/install.html?os=linux&arch=x64&mem=80&model=own#model-title) lists the alternatives.

## 1. Prepare the firmware

Open the firmware setup (often F2 or Del while the machine starts) and check these settings. Names differ by manufacturer.

| Setting | Value | Why |
|---|---|---|
| UMA frame buffer size / iGPU memory | The smallest option (512MB to 2GB) | Linux gives the GPU memory dynamically in section 3, so a large fixed carve-out only takes memory away from the system. |
| Secure Boot | Your choice | Ubuntu boots with it on. Turning it off avoids signing prompts if you later install out-of-tree drivers. |
| Restore on AC power loss | Power on | The machine comes back by itself after a power cut, which is useful for a headless server. |

Save and exit.

## 2. Install Ubuntu Server

1. Download **Ubuntu Server 24.04 LTS** (24.04.3 or newer) from [ubuntu.com](https://ubuntu.com/download/server), or a newer LTS if one is available.
2. Write it to the USB stick with [balenaEtcher](https://etcher.balena.io).
3. Boot from the USB stick (often F7, F11, or F12 for the boot menu).
4. Follow the installer. Use the whole disk, connect Ethernet, and select **Install OpenSSH server**.
5. Remove the USB stick and reboot when the installer finishes.

Ubuntu Server has no desktop, so there are no apps to remove like on the Jetson.

Log in on the console and check the kernel:

```bash
uname -r
```

Strix Halo needs a recent kernel. If the version is older than 6.14, install the hardware enablement kernel and reboot:

```bash
sudo apt update
sudo apt install -y linux-generic-hwe-24.04
sudo reboot
```

## 3. Connect over SSH and secure the system

Find the IP address on the console:

```bash
hostname -I
```

From your computer, connect and set up key-based login:

```bash
ssh yourusername@SERVER_IP
ssh-keygen -t ed25519
ssh-copy-id yourusername@SERVER_IP
```

Reserve the server's IP address in your router's DHCP settings, then install updates and basic tools:

```bash
sudo apt update && sudo apt upgrade -y
sudo apt install -y tmux curl git jq openssl nvtop
```

Once SSH works, you can disconnect the monitor, keyboard, and mouse.

### Set up Tailscale

```bash
curl -fsSL https://tailscale.com/install.sh | sh
sudo tailscale up
tailscale ip -4
```

Open the sign-in link, install Tailscale on your computer with the same account, and connect with `ssh yourusername@TAILSCALE_IP`. Do not forward port 22 on your router.

## 4. Give the GPU its memory

By default, Linux lets the integrated GPU use only about half of the system memory. `gpt-oss:120b` needs about 65GB for its weights plus room for context, so raise the limit to 96GB. That leaves about 32GB for the system, Docker sandboxes, and Cedar.

The limit is set with two kernel parameters, counted in 4KB pages (96GB = 96 × 262,144 = 25,165,824 pages):

```bash
sudo sed -i 's/^GRUB_CMDLINE_LINUX_DEFAULT="\(.*\)"/GRUB_CMDLINE_LINUX_DEFAULT="\1 ttm.pages_limit=25165824 ttm.page_pool_size=25165824"/' /etc/default/grub
grep ^GRUB_CMDLINE_LINUX_DEFAULT /etc/default/grub
sudo update-grub
sudo reboot
```

After the reboot, check the GPU memory:

```bash
sudo dmesg | grep -i 'amdgpu.*memory'
```

You want a line like `amdgpu: 98304M of GTT memory ready.` On a 64GB machine, use 48GB instead (12,582,912 pages).

## 5. Set up Ollama

Install Ollama. The install script detects the AMD GPU and downloads Ollama's ROCm libraries:

```bash
curl -fsSL https://ollama.com/install.sh | sh
sudo systemctl enable --now ollama
```

Add settings for this machine:

```bash
sudo mkdir -p /etc/systemd/system/ollama.service.d
sudo tee /etc/systemd/system/ollama.service.d/environment.conf << 'EOF'
[Service]
Environment="OLLAMA_FLASH_ATTENTION=1"
Environment="OLLAMA_KV_CACHE_TYPE=q8_0"
Environment="OLLAMA_KEEP_ALIVE=24h"
Environment="OLLAMA_CONTEXT_LENGTH=65536"
EOF
sudo systemctl daemon-reload
sudo systemctl restart ollama
```

Check that Ollama found the GPU:

```bash
curl http://localhost:11434/api/version
journalctl -u ollama -n 50 --no-pager | grep -i 'inference compute'
```

You want an `inference compute` line with `library=ROCm` and `gfx1151`, with about 96 GiB of total memory. If it says `library=cpu`, see [Troubleshooting](#troubleshooting).

Ollama listens only on this machine by default. Leave it that way, because Cedar runs on the same machine.

## 6. Choose a model and test tool calling

Download `gpt-oss:120b` (about 65GB) in `tmux`, so the download keeps going if SSH disconnects:

```bash
tmux new -s work
ollama pull gpt-oss:120b
ollama run gpt-oss:120b "Reply with OK"
ollama ps
```

You want `100% GPU` in the `ollama ps` output. In another SSH window, `nvtop` shows the GPU's load and memory while the model answers.

Cedar acts through tool calls, so test that before installing Cedar:

```bash
curl -s http://127.0.0.1:11434/v1/chat/completions -H 'Content-Type: application/json' -d '{
  "model": "gpt-oss:120b",
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

### Other models

| Model | Size | Use it for |
|---|---|---|
| `gpt-oss:120b` | about 65GB | Text. The default for this guide. |
| `qwen3.6:35b-a3b` | about 23GB | Text and images. Faster, and the best choice on a 64GB machine. |
| `qwen3.5:122b` | about 81GB | Text and images on 128GB. Raise the GPU limit in section 4 to about 108GB (28,311,552 pages) and lower `OLLAMA_CONTEXT_LENGTH` to 32768. |

Avoid large dense models, such as 70B models. They fit, but generate only a few tokens per second on this memory.

## 7. Install Cedar

Confirm that the machine uses the x86_64 build:

```bash
uname -m
ldd --version | head -1
```

`uname -m` should print `x86_64`, and the second command should show glibc 2.35 or newer. **Download the `linux-x64` release, not `linux-arm64` as on the Jetson.**

```bash
V=0.6.3
F=Cedar-$V-linux-x64
U=https://github.com/hanneshapke/cedar/releases/download
curl -fLO $U/v$V/$F.tar.gz
tar -xzf $F.tar.gz
sudo install $F/cedar* /usr/local/bin/
```

Check the [releases page](https://github.com/hanneshapke/cedar/releases) for a newer version, then change `V` if you want it.

### Install Docker and build the sandbox

```bash
curl -fsSL https://get.docker.com | sh
sudo systemctl enable --now docker
git clone --depth 1 --branch v$V https://github.com/hanneshapke/cedar.git ~/cedar
sudo docker build -t cedar-sandbox ~/cedar/sandbox/
```

### Create the configuration

Create a service account. The `cedar` account is in the `docker` group, which is effectively root on this machine, so use it only to run Cedar.

```bash
sudo useradd --system --create-home --home-dir /var/lib/cedar \
  --shell /usr/sbin/nologin --groups docker cedar
```

Then generate the keys and create the configuration files. Run this as one block, because later commands in the block use the generated values:

```bash
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

sudo chmod 600 /etc/cedar/cedar.env /etc/cedar/chat.env
echo "Chat access token: $CHAT_TOKEN"
echo "Admin token: $ADMIN_TOKEN"
```

Save both tokens in your password manager. You'll need the chat token to open Cedar and the admin token to change server settings.

`MAX_CONCURRENT_RUNS=2` lets two runs work at once. Ollama answers one request at a time by default, so the second run waits briefly between turns while the first one's sandbox does its work. Unlike the Jetson, you don't need to turn off Cedar's optional tools to save prompt space.

### Run Cedar as a service

```bash
sudo tee /etc/systemd/system/cedar.service >/dev/null <<'EOF'
[Unit]
Description=cedar agent server
After=network-online.target docker.service ollama.service
Wants=network-online.target ollama.service
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

```bash
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/readyz

CEDAR_KEY=$(sudo grep ^CEDAR_API_KEYS= /etc/cedar/cedar.env | cut -d= -f2)

curl -s -X PUT http://127.0.0.1:8080/v1/admin/models/gpt-oss \
  -H "Authorization: Bearer $CEDAR_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"base_url": "http://127.0.0.1:11434/v1", "model": "gpt-oss:120b", "label": "gpt-oss 120B", "reasoning_levels": ["low", "medium", "high"], "is_default": true}'
```

The first command should print `200`. gpt-oss supports three reasoning levels, which you can then choose per run in the chat. Cedar's default reply limit of 16,384 tokens works here, so you don't need to lower `max_tokens` as on the Jetson.

Start a test run:

```bash
curl -s http://127.0.0.1:8080/v1/runs -H "Authorization: Bearer $CEDAR_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"prompt": "Write hello.py that prints the first 10 primes, run it, and show the output."}' | tee /tmp/run.json

RUN=$(jq -r .id /tmp/run.json)
curl -N http://127.0.0.1:8080/v1/runs/$RUN/events -H "Authorization: Bearer $CEDAR_KEY"
```

Look for `tool.call`, `tool.result`, and `run.completed`.

## 8. Open Cedar from your computer

Create an SSH tunnel from your computer. Use your Tailscale IP address when you're away from home:

```bash
ssh -N -L 3000:127.0.0.1:3000 yourusername@SERVER_IP
```

Then open [http://localhost:3000](http://localhost:3000) and enter your chat access token.

Keep `cedar` and Ollama bound to `127.0.0.1`. If someone can reach Cedar's chat, they can run an agent that executes commands, so don't expose port 3000 to the internet.

### Use Cedar on your phone

Cedar's phone apps require HTTPS. In the [Tailscale admin console](https://login.tailscale.com/admin/dns), turn on **MagicDNS** and **HTTPS Certificates**, then run:

```bash
sudo tailscale serve --bg --https=443 http://127.0.0.1:3000
URL=https://$(tailscale status --json | jq -r '.Self.DNSName | rtrimstr(".")')
echo "CHAT_PUBLIC_URL=$URL" | sudo tee -a /etc/cedar/chat.env
sudo systemctl restart cedar-chat
```

Install Tailscale on your phone with the same account, then pair the Cedar app under **Settings > Phone** in the chat.

## 9. Maintain Cedar

Follow the logs:

```bash
journalctl -u cedar -u cedar-chat -f
```

To update Cedar:

```bash
V=NEW_VERSION
F=Cedar-$V-linux-x64
U=https://github.com/hanneshapke/cedar/releases/download
curl -fLO $U/v$V/$F.tar.gz
tar -xzf $F.tar.gz
sudo install $F/cedar* /usr/local/bin/
git -C ~/cedar fetch --depth 1 origin tag v$V
git -C ~/cedar checkout v$V
sudo docker build -t cedar-sandbox ~/cedar/sandbox/
sudo systemctl restart cedar cedar-chat
```

Restarting Cedar ends any active run, so do this when nothing is working.

To update Ollama, run its install script again. It keeps your models and the settings in `environment.conf`. After a kernel update, check `dmesg` again to confirm the GPU still has its 96GB.

## 10. Power and recovery

Restart remotely with `sudo reboot`. With **Restore on AC power loss** set in section 1, a smart plug gives you a way to recover a frozen machine remotely: turn it off, wait about 10 seconds, and turn it back on. Cut power only when the machine is frozen, not while it's running normally.

## Troubleshooting

| Problem | What to check |
|---|---|
| Ollama's log shows `library=cpu` | Run `ls -l /dev/kfd /dev/dri` and `id ollama`; the `ollama` user must be in the `render` and `video` groups. If the GPU still isn't found, add `Environment="OLLAMA_VULKAN=1"` to `environment.conf` to use the Vulkan backend instead of ROCm, then restart Ollama. |
| `dmesg` shows less GTT memory than you set | Check that `cat /proc/cmdline` includes both `ttm.` parameters, and that `update-grub` ran before the reboot. |
| `ollama ps` shows a CPU/GPU split | The model and its context don't fit in the GPU's memory. Lower `OLLAMA_CONTEXT_LENGTH`, raise the limit in section 4, or use a smaller model. |
| The machine freezes or the GPU resets under load | Install the latest firmware from the manufacturer and the newest HWE kernel. Early Strix Halo firmware and kernels had GPU stability bugs. |
| `tar` or `cedar` fails with an architecture error | Confirm `uname -m` prints `x86_64` and that you downloaded `linux-x64`. |
| `/readyz` returns `503` | Run `id cedar` to confirm it's in the `docker` group, and `sudo docker images` to confirm `cedar-sandbox` exists. |
| A message says no model is registered | Run the model registration command again, or use **Settings > Server** with your admin token. |
| Runs show tool calls as plain text | Repeat the tool-calling test in section 6, or switch models. |
| Replies are slow | Use an MoE model. Dense models of the same size run several times slower on this memory. |

## Sources

- [Cedar](https://getcedar.io)
- [Cedar install guide: your model](https://getcedar.io/install.html#model-title)
- [Cedar Linux deployment guide](https://github.com/hanneshapke/cedar/blob/main/DEPLOYMENT.md)
- [Running Cedar on a Jetson Orin Nano](cedar-on-jetson-orin-nano.md)
- [Ollama on Linux](https://docs.ollama.com/linux)
- [Ollama GPU support](https://docs.ollama.com/gpu)
- [gpt-oss on Ollama](https://ollama.com/library/gpt-oss)
