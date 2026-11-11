# Running Cedar on a Jetson Orin Nano Developer Kit

This guide turns an 8GB Jetson Orin Nano Developer Kit into a private, remote Cedar server. You'll complete the first boot with a monitor, then run the Jetson over SSH with Ollama serving a local model.

[Cedar](https://getcedar.io) has two programs: `cedar`, the agent server, and `cedar-chat`, the web interface. Each run uses tools inside a Docker sandbox.

If you want to run much larger models with the same setup, see [Running Cedar on an AMD Ryzen AI Max (Strix Halo) mini PC](cedar-on-ryzen-ai-max-strix-halo.md). It follows these steps on a 128GB x86_64 machine with `gpt-oss:120b`.

```text
Your computer -> SSH tunnel or Tailscale -> cedar-chat :3000 -> cedar :8080 -> Ollama :11434
                                                                  |
                                                                  -> Docker sandbox
```

## What you need

- Jetson Orin Nano Developer Kit and its 19V power supply
- microSD card (64GB or larger)
- microSD card reader or adapter for your computer
- Ethernet connection to your router
- Monitor with a DisplayPort input, or a DisplayPort-to-HDMI cable for an HDMI monitor
- USB keyboard and mouse

You only need the monitor, keyboard, and mouse for the first boot. In this setup, the USB-C connection to a Mac did not work, so plan to complete setup with a monitor.

## 1. Flash the microSD card

1. Download the **Jetson Orin Nano Developer Kit SD Card Image** for JetPack 6.2.x from NVIDIA's [JetPack page](https://developer.nvidia.com/embedded/jetpack).
2. Install [balenaEtcher](https://etcher.balena.io).
3. Connect the microSD card to your computer.
4. In Etcher, select **Flash from file**, choose the `.zip`, and select your microSD card.
5. Click **Flash**. This erases everything on the card.
6. Eject the card when Etcher finishes. If macOS says the disk is unreadable, select **Ignore** or **Eject**, not **Initialize**.

Insert the card into the slot on the underside of the Jetson module.

## 2. Complete the first boot

1. Connect the Jetson to your router with Ethernet.
2. Connect the monitor to the Jetson's DisplayPort port, using a DisplayPort-to-HDMI cable for an HDMI monitor.
3. Connect the USB keyboard and mouse.
4. Turn on the monitor and select the correct input.
5. Connect the 19V power supply. The green LED next to the USB-C port should turn on.
6. Follow the NVIDIA and Ubuntu setup screens and confirm each question, including the license agreement and your username and password.

**You must finish these setup questions before you can connect over SSH.**

If you see a UEFI shell instead of the setup screens, the firmware is probably too old for JetPack 6. Follow NVIDIA's [JetPack 6.x update path](https://docs.nvidia.com/jetson/orin-nano-devkit/user-guide/latest/update_firmware.html), which uses JetPack 5.1.3 as a bridge.

## 3. Connect over SSH

On the Jetson, open a terminal and find its IP address:

```bash
hostname -I
```

You can also find it in your router's client list. From your computer, connect with:

```bash
ssh yourusername@JETSON_IP
```

Once SSH works, you can disconnect the monitor, keyboard, and mouse.

## 4. Update and secure the system

Check your release:

```bash
cat /etc/nv_tegra_release
```

`R36` means JetPack 6. `R35` means JetPack 5, so finish the firmware update path before moving to JetPack 6.

Install updates:

```bash
sudo apt update && sudo apt upgrade -y
sudo reboot
```

Reserve the Jetson's IP address in your router's DHCP settings, then make sure SSH starts at boot:

```bash
sudo systemctl enable ssh
```

From your computer, set up key-based login:

```bash
ssh-keygen -t ed25519
ssh-copy-id yourusername@JETSON_IP
```

Confirm that key-based login works before making any stricter SSH changes.

## 5. Set up remote access with Tailscale

Tailscale lets you reach the Jetson from anywhere without forwarding ports through your router.

```bash
curl -fsSL https://tailscale.com/install.sh | sh
sudo tailscale up
tailscale ip -4
```

Open the sign-in link. Then install Tailscale on your computer, sign in with the same account, and connect:

```bash
ssh yourusername@TAILSCALE_IP
```

Do not forward port 22 on your router.

## 6. Run without a desktop

Boot to the command line to free up memory:

```bash
sudo systemctl set-default multi-user.target
```

To get the desktop back later, run:

```bash
sudo systemctl set-default graphical.target
```

### Remove desktop apps

Check for snap apps:

```bash
snap list
```

If you have the same snaps as this setup, remove them in this order:

```bash
sudo snap remove --purge firefox
sudo snap remove --purge thunderbird
sudo snap remove --purge gnome-46-2404 gtk-common-themes mesa-2404
sudo snap remove --purge core24
sudo snap remove --purge bare
```

Preview the `apt` removal:

```bash
APPS=(firefox thunderbird 'libreoffice*' rhythmbox shotwell cheese totem remmina transmission-gtk aisleriot gnome-mahjongg gnome-mines gnome-sudoku simple-scan)
sudo apt-get -s purge "${APPS[@]}" | grep '^Remv' | grep -E 'nvidia|network-manager|openssh|tailscale|systemd|netplan'
```

The second command should print nothing. If it prints a package, stop and review it. If apt says it can't find a package, remove that name from the list and run the preview again.

When the preview is clear, remove the apps:

```bash
sudo apt-get purge -y "${APPS[@]}"
```

Preview the cleanup:

```bash
sudo apt-get -s autoremove --purge | grep '^Remv' | grep -E 'nvidia|network-manager|openssh|tailscale|systemd|netplan'
```

If that prints nothing, run:

```bash
sudo apt-get autoremove --purge -y
sudo reboot
```

Reconnect over both your local IP address and Tailscale.

### Install monitoring tools

```bash
sudo apt install -y tmux curl git jq openssl
curl -fsSL https://raw.githubusercontent.com/rbonghi/jetson_stats/master/scripts/install_jtop_torun_without_sudo.sh -o install_jtop.sh
less install_jtop.sh
sudo -v
bash install_jtop.sh
sudo usermod -aG jtop "$USER"
```

Log out and SSH back in, then check `jtop`:

```bash
systemctl is-active jtop.service
jtop
```

Use `tmux new -s work` for long jobs, so they keep running if SSH disconnects.

## 7. Set up Ollama

Install Ollama and enable its service:

```bash
curl -fsSL https://ollama.com/install.sh | sh
sudo systemctl enable --now ollama
```

Add settings for the 8GB board:

```bash
sudo mkdir -p /etc/systemd/system/ollama.service.d
sudo tee /etc/systemd/system/ollama.service.d/environment.conf << 'EOF'
[Service]
Environment="OLLAMA_FLASH_ATTENTION=1"
Environment="OLLAMA_KV_CACHE_TYPE=q8_0"
Environment="OLLAMA_KEEP_ALIVE=1h"
Environment="OLLAMA_CONTEXT_LENGTH=16384"
EOF
sudo systemctl daemon-reload
sudo systemctl restart ollama
```

Check that it's working:

```bash
curl http://localhost:11434/api/version
journalctl -u ollama -n 30 --no-pager
```

A `cuda_v12` "skipping CUDA device" message is harmless if a later `inference compute` line shows the Orin GPU through `cuda_v13`.

Ollama listens only on the Jetson by default. Leave it that way, because Cedar runs on the same board.

## 8. Choose a model and test tool calling

Cedar acts through tool calls, so test this before installing Cedar. Start with Qwen 3.5 4B:

```bash
ollama pull qwen3.5:4b
ollama run qwen3.5:4b "Reply with OK"
ollama ps
```

You want `100% GPU` in the `ollama ps` output. If you get a memory error or a CPU/GPU split, use `qwen3.5:2b` instead, including in the commands below.

Test the OpenAI-compatible endpoint that Cedar uses:

```bash
curl -s http://127.0.0.1:11434/v1/chat/completions -H 'Content-Type: application/json' -d '{
  "model": "qwen3.5:4b",
  "max_tokens": 256,
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

## 9. Install Cedar

Confirm that the Jetson uses the ARM build:

```bash
uname -m
ldd --version | head -1
```

`uname -m` should print `aarch64`, and the second command should show glibc 2.35 or newer. **Download the `linux-arm64` release, not `linux-x64`.**

```bash
V=0.3.6
F=Cedar-$V-linux-arm64
U=https://github.com/hanneshapke/cedar/releases/download
curl -fLO $U/v$V/$F.tar.gz
tar -xzf $F.tar.gz
sudo install $F/cedar* /usr/local/bin/
```

Check the [releases page](https://github.com/hanneshapke/cedar/releases) for a newer version, then change `V` if you want it.

### Install Docker and build the sandbox

Check whether Docker is installed:

```bash
sudo docker info --format '{{.ServerVersion}}'
```

If that doesn't print a version number, install Docker:

```bash
curl -fsSL https://get.docker.com | sh
sudo systemctl enable --now docker
```

Build the image that Cedar's tools run in:

```bash
git clone --depth 1 --branch v$V https://github.com/hanneshapke/cedar.git ~/cedar
sudo docker build -t cedar-sandbox ~/cedar/sandbox/
```

### Create the configuration

Create a service account and configuration files. The `cedar` account is in the `docker` group, which is effectively root on this machine, so use it only to run Cedar.

```bash
sudo useradd --system --create-home --home-dir /var/lib/cedar \
  --shell /usr/sbin/nologin --groups docker cedar
```

**Log out and SSH back in before continuing.**

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

Save both tokens in your password manager. You'll need the chat token to open Cedar and the admin token to change server settings.

`MAX_CONCURRENT_RUNS=1` and the longer timeout suit this board because the model, Cedar, and sandboxes all share the same 8GB of memory.

Cedar also offers many optional tools. To reduce prompt size on the Orin Nano, you can disable ones you don't need by adding lines such as these to `/etc/cedar/cedar.env`:

```text
MAPS=off
RECIPES=off
TRANSIT=off
STOCKS=off
PRODUCTS=off
```

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

Confirm Cedar is ready, then connect it to Ollama:

```bash
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/readyz

CEDAR_KEY=$(sudo grep ^CEDAR_API_KEYS= /etc/cedar/cedar.env | cut -d= -f2)

curl -s -X PUT http://127.0.0.1:8080/v1/admin/models/qwen \
  -H "Authorization: Bearer $CEDAR_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"base_url": "http://127.0.0.1:11434/v1", "model": "qwen3.5:4b", "label": "Qwen 3.5 4B", "max_tokens": 2048, "is_default": true}'
```

The first command should print `200`. The `max_tokens` value is lower than Cedar's default of 16,384 because Cedar gives up on a single model reply after 300 seconds, and a small model on this board can take longer than that to generate a very long reply.

Start a test run:

```bash
curl -s http://127.0.0.1:8080/v1/runs -H "Authorization: Bearer $CEDAR_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"prompt": "Write hello.txt containing the word hi, then read it back."}' | tee /tmp/run.json

RUN=$(jq -r .id /tmp/run.json)
curl -N http://127.0.0.1:8080/v1/runs/$RUN/events -H "Authorization: Bearer $CEDAR_KEY"
```

Look for `tool.call`, `tool.result`, and `run.completed`.

## 10. Open Cedar from your computer

Create an SSH tunnel from your computer. You can use your Tailscale IP address when you're away from home:

```bash
ssh -N -L 3000:127.0.0.1:3000 yourusername@JETSON_IP
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

## 11. Maintain Cedar

Follow the logs:

```bash
journalctl -u cedar -u cedar-chat -f
```

To update, download the new `linux-arm64` release, then:

```bash
V=NEW_VERSION
F=Cedar-$V-linux-arm64
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

## 12. Power and recovery

Use this to restart remotely:

```bash
sudo reboot
```

Use this only if someone can physically reach the board:

```bash
sudo shutdown now
```

The Jetson stays off after shutdown, even with the power connected. To turn it back on, unplug the power, wait about 10 seconds, and plug it back in.

If the board returns by itself after that power cycle, a smart plug can give you a way to recover it remotely. Cut power only when the board is off or frozen, because cutting power while it's running can corrupt the microSD card.

## Troubleshooting

| Problem | What to check |
|---|---|
| SSH doesn't work after flashing | Finish the setup questions on the monitor first. |
| The screen shows a UEFI shell | Update the firmware using NVIDIA's JetPack 6.x update path. |
| The monitor stays blank | Confirm the monitor input, connect it before powering on, then reseat the cable and microSD card. |
| `tar` or `cedar` fails with an architecture error | Confirm `uname -m` prints `aarch64` and that you downloaded `linux-arm64`. |
| `/readyz` returns `503` | Run `id cedar` to confirm it's in the `docker` group, and `sudo docker images` to confirm `cedar-sandbox` exists. |
| A message says no model is registered | Run the model registration command again, or use **Settings > Server** with your admin token. |
| Runs show tool calls as plain text | Repeat the tool-calling test in section 8, or switch models. |
| A run reaches the time limit | Raise `RUN_TIMEOUT_SECS`, or lower the model's maximum reply length. |
| The board slows down or freezes | Use `qwen3.5:2b`, keep `MAX_CONCURRENT_RUNS=1`, and check memory in `jtop`. |
| Ollama returns a memory error | Switch to `qwen3.5:2b` and keep other memory use low. |

## Sources

- [Cedar](https://getcedar.io)
- [Cedar Linux deployment guide](https://github.com/hanneshapke/cedar/blob/main/DEPLOYMENT.md)
- [Cedar README](https://github.com/hanneshapke/cedar)
- [Running Cedar on an AMD Ryzen AI Max (Strix Halo) mini PC](cedar-on-ryzen-ai-max-strix-halo.md)
- [Jetson Orin Nano Quick Start Guide](https://docs.nvidia.com/jetson/orin-nano-devkit/user-guide/latest/quick_start.html)
- [JetPack 6.x Update Path](https://docs.nvidia.com/jetson/orin-nano-devkit/user-guide/latest/update_firmware.html)
- [Jetson AI Lab: OpenClaw on Jetson](https://www.jetson-ai-lab.com/tutorials/openclaw/)
- [jetson-stats no-sudo install docs](https://raw.githubusercontent.com/rbonghi/jetson_stats/master/docs/nosudo.rst)
