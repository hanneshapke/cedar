#!/usr/bin/env bash
# Build cedar and cedar-chat, then run vLLM, cedar, and the chat UI in one tmux session. If
# Tailscale is installed and logged in, the chat UI is also served over HTTPS on the tailnet
# (`tailscale serve`), which is what the iOS and Android apps need.
#
# This runs the stack on one machine you are logged in to. Nothing restarts it after a crash or a
# reboot; for a long-running server use the systemd setup in DEPLOYMENT.md.
#
# Before it starts, it checks Docker and (unless TAILSCALE_SERVE=off) Tailscale. On Linux it offers
# to fix what is missing, showing each command and running it only when you answer yes: install
# Docker Engine (get.docker.com), start it, let your user run it, install Tailscale
# (tailscale.com/install.sh) and log in. Without a terminal it installs nothing and says what to
# run. On a Mac it says what to install (or use the Cedar app, which sets Docker up itself).
#
#   scripts/cedar-stack.sh            build, start everything, attach
#   scripts/cedar-stack.sh --no-build skip cargo and docker builds
#   scripts/cedar-stack.sh --restart  kill the existing session first
#   scripts/cedar-stack.sh --no-vllm  don't start, check or wait for vLLM, and don't detect
#                                     LLM_MODEL from it (cedar starts at once; set LLM_MODEL in
#                                     .env if you need it)
#   scripts/cedar-stack.sh --down     kill the session and stop tailscale serve; vLLM keeps running
#   scripts/cedar-stack.sh --down --with-vllm
#                                     also stop vLLM (docker compose down)
#   scripts/cedar-stack.sh --down-all same as --down --with-vllm
#   scripts/cedar-stack.sh --pair     print the QR code the iOS and Android apps scan to connect
#                                     (its address on the tailnet and the access token); needs
#                                     qrencode
#
# Settings come from .env in the repo root (created on first run with fresh keys). Besides
# cedar's and cedar-chat's own variables it reads:
#   VLLM_COMPOSE_FILE  compose file that runs vLLM; if set, the script starts it and shows its logs
#   TMUX_SESSION       session name (default: cedar)
#   TAILSCALE_SERVE    auto (default: serve if tailscale is logged in), on (fail if it can't), off
#   TAILSCALE_HTTPS_PORT
#                      the tailnet HTTPS port to serve the chat UI on (default: 443)
#   INSTALL_DEPS       ask (default): offer to install or fix Docker and Tailscale; yes: do it
#                      without asking; no: only say what is missing
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ENV_FILE="$ROOT/.env"
SELF="$ROOT/scripts/cedar-stack.sh"

die() { echo "error: $*" >&2; exit 1; }

load_env() {
    [[ -f $ENV_FILE ]] || die "$ENV_FILE not found; run scripts/cedar-stack.sh first"
    set -a
    # shellcheck disable=SC1090
    . "$ENV_FILE"
    set +a
    : "${CEDAR_API_KEYS:?CEDAR_API_KEYS must be set in $ENV_FILE}"
    BIND_ADDR=${BIND_ADDR:-127.0.0.1:8080}
    LLM_BASE_URL=${LLM_BASE_URL:-http://localhost:8000/v1}
    CHAT_BIND_ADDR=${CHAT_BIND_ADDR:-127.0.0.1:3000}
    TMUX_SESSION=${TMUX_SESSION:-cedar}
    TAILSCALE_SERVE=${TAILSCALE_SERVE:-auto}
    TAILSCALE_HTTPS_PORT=${TAILSCALE_HTTPS_PORT:-443}
    case $TAILSCALE_SERVE in
        auto|on|off) ;;
        *) die "TAILSCALE_SERVE must be auto, on, or off (got '$TAILSCALE_SERVE')" ;;
    esac
    INSTALL_DEPS=${INSTALL_DEPS:-ask}
    case $INSTALL_DEPS in
        ask|yes|no) ;;
        *) die "INSTALL_DEPS must be ask, yes, or no (got '$INSTALL_DEPS')" ;;
    esac
}

# --- Docker and Tailscale ------------------------------------------------------------------

# ask <question> <y|n>: true if the answer is yes; the second argument is the answer to Enter.
# INSTALL_DEPS=yes or no answers instead, and without a terminal the answer is no.
ask() {
    case $INSTALL_DEPS in
        yes) echo "$1 (INSTALL_DEPS=yes)"; return 0 ;;
        no) return 1 ;;
    esac
    [[ -t 0 ]] || return 1
    local hint='[Y/n]' reply
    [[ $2 == n ]] && hint='[y/N]'
    read -r -p "$1 $hint " reply || return 1
    [[ ${reply:-$2} == [Yy]* ]]
}

# as_root <command...>: run it as root, through sudo unless the script already is root. The
# command is shown first.
as_root() {
    echo "+ $*" >&2
    if (( EUID == 0 )); then
        "$@"
    else
        command -v sudo >/dev/null || die "this needs root and sudo is not installed; run as root: $*"
        sudo "$@"
    fi
}

# run_installer <url>: download an install script and run it as root.
run_installer() {
    local script
    script=$(mktemp)
    echo "==> downloading $1"
    curl -fsSL "$1" -o "$script" || { rm -f "$script"; die "could not download $1"; }
    as_root sh "$script" || { rm -f "$script"; die "$1 failed (see above)"; }
    rm -f "$script"
}

# ensure_docker <no_vllm> <args...>: a docker CLI, a daemon that answers, and permission to use
# it; offers to install or fix each on Linux. After adding the user to the docker group it runs
# the script again under `sg docker`, so the new group counts without logging in again.
ensure_docker() {
    local no_vllm=$1
    shift
    local linux=0
    [[ $(uname -s) == Linux ]] && linux=1
    if ! command -v docker >/dev/null; then
        (( linux )) || die "docker is not installed. Install Docker Desktop (brew install --cask docker) or OrbStack and start it, or use the Cedar app, which sets Docker up itself."
        echo "Docker is not installed. Docker's script installs Docker Engine and the compose plugin:"
        echo "  curl -fsSL https://get.docker.com | sudo sh"
        ask "Install Docker Engine now?" y || die "docker is not installed; install it with the command above"
        run_installer https://get.docker.com
        command -v docker >/dev/null || die "docker is still not on PATH after installing it"
    fi

    local err me
    me=$(id -un)
    if ! err=$(docker info --format '{{.ServerVersion}}' 2>&1 >/dev/null); then
        if [[ $err == *"permission denied"* ]] && (( linux && EUID != 0 )); then
            [[ -n ${CEDAR_STACK_SG:-} ]] && die "docker still says permission denied in the docker group: $err"
            if ! id -nG "$me" | tr ' ' '\n' | grep -qx docker; then
                echo "Your user ($me) may not use Docker yet: it is not in the docker group."
                ask "Add $me to the docker group?" y || die "add yourself to the docker group (sudo usermod -aG docker $me), log out and in, and run this again"
                as_root usermod -aG docker "$me"
            fi
            echo "==> carrying on in the docker group (sg docker); new logins pick it up by themselves"
            CEDAR_STACK_SG=1 exec sg docker -c "$(printf '%q ' "$SELF" "$@")"
        elif (( linux )) && command -v systemctl >/dev/null; then
            echo "Docker is installed but its daemon is not answering: $err"
            ask "Start it (and start it at boot)?" y || die "start Docker: sudo systemctl enable --now docker"
            as_root systemctl enable --now docker
            local i
            for i in $(seq 20); do
                docker info >/dev/null 2>&1 && break
                (( i == 20 )) && die "Docker did not start; check: sudo systemctl status docker"
                sleep 1
            done
            # The daemon may answer now and still refuse this user.
            ensure_docker "$no_vllm" "$@"
            return
        elif (( linux )); then
            die "Docker is installed but not answering: $err. Start its daemon (dockerd) and run this again."
        else
            die "Docker is installed but not answering: $err. Start Docker Desktop (or OrbStack, Colima) and run this again."
        fi
    fi

    if [[ -n ${VLLM_COMPOSE_FILE:-} ]] && (( ! no_vllm )) && ! docker compose version >/dev/null 2>&1; then
        die "VLLM_COMPOSE_FILE is set but 'docker compose' is missing; install the compose plugin (sudo apt install docker-compose-plugin)"
    fi
}

# skip_tailscale: carry on without Tailscale, unless TAILSCALE_SERVE=on needs it.
skip_tailscale() {
    [[ $TAILSCALE_SERVE == on ]] && die "TAILSCALE_SERVE=on needs Tailscale installed and logged in"
    echo "Carrying on without Tailscale; the phone apps can't reach the chat. Set TAILSCALE_SERVE=off in $ENV_FILE to stop this question." >&2
}

# ensure_tailscale: unless TAILSCALE_SERVE=off, Tailscale installed and logged in, for the phone
# apps; offers to install it and log in on Linux. With TAILSCALE_SERVE=auto it is optional, so
# saying no carries on without it.
ensure_tailscale() {
    [[ $TAILSCALE_SERVE == off ]] && return 0
    local default=n
    [[ $TAILSCALE_SERVE == on ]] && default=y

    if ! command -v tailscale >/dev/null; then
        if [[ $(uname -s) != Linux ]]; then
            echo "Tailscale is not installed; get it from https://tailscale.com/download" >&2
            skip_tailscale
            return 0
        fi
        echo "Tailscale is not installed. It serves the chat to the iOS and Android apps over HTTPS, on your"
        echo "tailnet only. Tailscale's script installs it:"
        echo "  curl -fsSL https://tailscale.com/install.sh | sudo sh"
        ask "Install Tailscale now?" "$default" || { skip_tailscale; return 0; }
        run_installer https://tailscale.com/install.sh
        command -v tailscale >/dev/null || die "tailscale is still not on PATH after installing it"
    fi

    tailscale_ready && return 0
    echo "Tailscale is installed but this machine is not logged in to a tailnet."
    echo "  sudo tailscale up       (prints a link to log in with)"
    ask "Log in now?" "$default" || { skip_tailscale; return 0; }
    as_root tailscale up || die "tailscale up failed (see above)"
    # So `tailscale serve` (and --down) work without sudo.
    (( EUID == 0 )) || as_root tailscale set --operator="$(id -un)" ||
        echo "warning: could not let $(id -un) manage tailscale; tailscale serve may need sudo" >&2
    echo "Logged in. For the phone apps, also turn on MagicDNS and HTTPS Certificates at"
    echo "https://login.tailscale.com/admin/dns"
}

# --- tailscale serve -----------------------------------------------------------------------

# tailscale_ready: true if tailscale is installed and this machine is logged in to a tailnet.
tailscale_ready() {
    command -v tailscale >/dev/null &&
        [[ $(tailscale status --json 2>/dev/null | jq -r '.BackendState // empty') == Running ]]
}

# tailscale_url: the HTTPS address `tailscale serve` gives the chat UI on the tailnet.
tailscale_url() {
    local name
    name=$(tailscale status --json | jq -r '.Self.DNSName // empty')
    name=${name%.}
    [[ -n $name ]] || return 1
    if [[ $TAILSCALE_HTTPS_PORT == 443 ]]; then
        echo "https://$name"
    else
        echo "https://$name:$TAILSCALE_HTTPS_PORT"
    fi
}

# serve_start: proxy https://<machine>.<tailnet>.ts.net to cedar-chat. Prints the URL on success;
# prints nothing when Tailscale is off or unavailable in auto mode.
serve_start() {
    [[ $TAILSCALE_SERVE == off ]] && return 0
    if ! tailscale_ready; then
        [[ $TAILSCALE_SERVE == on ]] && die "TAILSCALE_SERVE=on, but tailscale is not installed or not logged in (run: sudo tailscale up)"
        return 0
    fi
    # tailscale serve can only proxy to this machine. A wildcard bind is reachable on loopback.
    local host=${CHAT_BIND_ADDR%:*} port=${CHAT_BIND_ADDR##*:}
    case $host in 0.0.0.0|'[::]'|'') host=127.0.0.1 ;; esac
    echo "==> tailscale serve --bg --https=$TAILSCALE_HTTPS_PORT http://$host:$port" >&2
    if ! tailscale serve --bg --https="$TAILSCALE_HTTPS_PORT" "http://$host:$port" >&2; then
        cat >&2 <<EOF
tailscale serve failed. Common causes:
  - your user may not manage tailscale:  sudo tailscale set --operator=\$USER
  - HTTPS certificates are off for the tailnet: enable MagicDNS and HTTPS Certificates
    at https://login.tailscale.com/admin/dns
  - tailscale is older than 1.52 (the --bg flag is newer): update it
EOF
        [[ $TAILSCALE_SERVE == on ]] && exit 1
        echo "Continuing without HTTPS on the tailnet." >&2
        return 0
    fi
    tailscale_url || true
}

# serve_stop: turn off what serve_start set up (only that HTTPS port).
serve_stop() {
    [[ $TAILSCALE_SERVE == off ]] && return 0
    tailscale_ready || return 0
    if tailscale serve --https="$TAILSCALE_HTTPS_PORT" off >/dev/null 2>&1; then
        echo "Stopped tailscale serve on HTTPS port $TAILSCALE_HTTPS_PORT."
    fi
}

# --- pairing the phone apps (iOS, Android) -----------------------------------------------

# pair: print the QR code the iOS and Android apps scan under Settings → Scan QR Code. It carries
# cedar://connect?server=<url>&token=<token>, the same link the Mac app shows (pairing/src/connect.rs;
# the iOS app reads it in ConnectionCode.swift, the Android app the same link). The address is CHAT_PUBLIC_URL when that is HTTPS,
# else the tailnet address `tailscale serve` gives the chat UI.
pair() {
    load_env
    for cmd in curl jq; do
        command -v "$cmd" >/dev/null || die "$cmd is not installed"
    done
    [[ -n ${CHAT_ACCESS_TOKEN:-} ]] || die "CHAT_ACCESS_TOKEN is not set in $ENV_FILE; the phone apps need it to connect"
    local server
    if [[ ${CHAT_PUBLIC_URL:-} == https://* ]]; then
        server=${CHAT_PUBLIC_URL%/}
    else
        [[ $TAILSCALE_SERVE == off ]] && die "TAILSCALE_SERVE=off and CHAT_PUBLIC_URL is not an https:// address; the phone apps connect over HTTPS only"
        tailscale_ready || die "tailscale is not installed or not logged in (run: sudo tailscale up); the phone apps connect over HTTPS only"
        server=$(tailscale_url) || die "tailscale has no name for this machine; turn on MagicDNS at https://login.tailscale.com/admin/dns"
    fi
    curl -fs -m 5 -o /dev/null "$server/" ||
        echo "warning: $server does not answer yet; start the stack (scripts/cedar-stack.sh) before you scan." >&2
    local link
    link="cedar://connect?server=$(jq -rn --arg v "$server" '$v|@uri')&token=$(jq -rn --arg v "$CHAT_ACCESS_TOKEN" '$v|@uri')"
    echo "In the Cedar app's Settings (iOS or Android), tap Scan QR Code and scan this. Or enter $server and the"
    echo "access token from CHAT_ACCESS_TOKEN in $ENV_FILE. Anyone who scans it can use your cedar."
    echo
    if command -v qrencode >/dev/null; then
        qrencode -t ansiutf8 -m 2 "$link"
    else
        echo "qrencode is not installed (sudo apt install qrencode), so here is the link instead:"
        echo "$link"
    fi
}

# wait_for <url> <what>: poll until the URL answers with a 2xx.
wait_for() {
    local i=0
    until curl -fs -m 3 -o /dev/null "$1"; do
        (( i++ % 10 == 0 )) && echo "waiting for $2 at $1 ..."
        sleep 3
    done
}

create_env() {
    local key token
    key=$(openssl rand -hex 24)
    token=$(openssl rand -hex 24)
    cat >"$ENV_FILE" <<EOF
# Local settings for scripts/cedar-stack.sh (gitignored). See .env.example and chat/README.md.

# cedar
CEDAR_API_KEYS=$key
BIND_ADDR=127.0.0.1:8080
LLM_BASE_URL=http://localhost:8000/v1
# LLM_MODEL=              # detected from vLLM if unset (not with --no-vllm)
MAX_CONCURRENT_RUNS=2
RUN_TIMEOUT_SECS=1800
RUST_LOG=info,sqlx=warn
# Give the sandbox network access so the agent can \`pip install\` packages. Set to "none" to keep
# it offline (the agent then only has what sandbox/Dockerfile installs).
SANDBOX_NETWORK=bridge
# Brave Search API key (https://api-dashboard.search.brave.com/). Turns on the web_search tool,
# which runs in cedar, not the sandbox. Without it the model has no way to search the web.
# BRAVE_API_KEY=

# cedar-chat (its CEDAR_API_KEY is the first of CEDAR_API_KEYS)
# Keep it on loopback: when Tailscale is logged in, the script serves it to your tailnet over HTTPS
# (https://<machine>.<tailnet>.ts.net), which the iOS and Android apps and other devices use.
CHAT_BIND_ADDR=127.0.0.1:3000
CHAT_ACCESS_TOKEN=$token
# auto: serve over Tailscale if it is logged in; on: fail if it can't; off: never.
TAILSCALE_SERVE=auto
# ask: offer to install or fix Docker and Tailscale; yes: do it without asking; no: only say so.
INSTALL_DEPS=ask
# TAILSCALE_HTTPS_PORT=443

# vLLM: the compose file that runs it. Leave empty if you start vLLM yourself.
VLLM_COMPOSE_FILE=
EOF
    chmod 600 "$ENV_FILE"
    echo "Created $ENV_FILE. Chat access token: $token"
}

# --- pane commands (run inside tmux) -------------------------------------------------------

pane_vllm() {
    load_env
    if [[ -n ${VLLM_COMPOSE_FILE:-} ]]; then
        docker compose -f "$VLLM_COMPOSE_FILE" logs -f --tail 50
    else
        echo "VLLM_COMPOSE_FILE is not set; expecting vLLM at $LLM_BASE_URL"
        wait_for "$LLM_BASE_URL/models" vLLM
        echo "vLLM is up."
    fi
}

pane_cedar() {
    load_env
    if (( ! ${NO_VLLM:-0} )); then
        wait_for "$LLM_BASE_URL/models" vLLM
        if [[ -z ${LLM_MODEL:-} ]]; then
            LLM_MODEL=$(curl -fsS "$LLM_BASE_URL/models" | jq -r '.data[0].id')
            export LLM_MODEL
            echo "LLM_MODEL=$LLM_MODEL (from vLLM)"
        fi
    fi
    cd "$ROOT"
    exec "$ROOT/target/release/cedar"
}

pane_chat() {
    load_env
    export CEDAR_URL=${CEDAR_URL:-http://$BIND_ADDR}
    export CEDAR_API_KEY=${CEDAR_API_KEY:-${CEDAR_API_KEYS%%,*}}
    wait_for "$CEDAR_URL/healthz" cedar
    cd "$ROOT"
    echo "chat UI: http://$CHAT_BIND_ADDR"
    [[ -n ${CHAT_PUBLIC_URL:-} ]] && echo "public URL: $CHAT_PUBLIC_URL"
    exec "$ROOT/target/release/cedar-chat"
}

# down <stop_vllm>: stop cedar and cedar-chat. vLLM keeps running unless stop_vllm is 1 and the
# script manages it.
down() {
    load_env
    if tmux kill-session -t "$TMUX_SESSION" 2>/dev/null; then
        echo "Killed tmux session '$TMUX_SESSION' (cedar, cedar-chat)."
    else
        echo "tmux session '$TMUX_SESSION' is not running."
    fi
    serve_stop
    if (( ! $1 )); then
        echo "vLLM keeps running (use --down --with-vllm to stop it)."
    elif [[ -n ${VLLM_COMPOSE_FILE:-} ]]; then
        echo "==> docker compose down ($VLLM_COMPOSE_FILE)"
        docker compose -f "$VLLM_COMPOSE_FILE" down
    else
        echo "VLLM_COMPOSE_FILE is not set; leaving vLLM at $LLM_BASE_URL alone."
    fi
}

# --- main ----------------------------------------------------------------------------------

main() {
    local build=1 restart=0 stop=0 with_vllm=0 no_vllm=0 pair_only=0
    for arg in "$@"; do
        case $arg in
            --no-build) build=0 ;;
            --restart) restart=1 ;;
            --no-vllm) no_vllm=1 ;;
            --down) stop=1 ;;
            --with-vllm) with_vllm=1 ;;
            --down-all) stop=1; with_vllm=1 ;;
            --pair) pair_only=1 ;;
            -h|--help) sed -n '2,/^[^#]/p' "$SELF" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
            *) die "unknown option: $arg" ;;
        esac
    done
    if (( pair_only )); then
        (( stop || with_vllm )) && die "--pair can't be combined with --down"
        pair
        exit 0
    fi
    if (( stop )); then
        down "$with_vllm"
        exit 0
    fi
    (( with_vllm )) && die "--with-vllm only applies to --down"

    for cmd in cargo tmux curl jq openssl; do
        command -v "$cmd" >/dev/null || die "$cmd is not installed"
    done
    # cedar-chat's Web Push encryption links OpenSSL; catch missing headers before a long build.
    if (( build )) && [[ $(uname -s) == Linux ]] && ! pkg-config --exists openssl 2>/dev/null; then
        die "OpenSSL headers not found (needed to build cedar-chat); install them: sudo apt install pkg-config libssl-dev"
    fi
    [[ -f $ENV_FILE ]] || create_env
    load_env
    ensure_docker "$no_vllm" "$@"
    ensure_tailscale

    local ts_url
    ts_url=$(serve_start)

    if tmux has-session -t "$TMUX_SESSION" 2>/dev/null; then
        if (( restart )); then
            tmux kill-session -t "$TMUX_SESSION"
        else
            echo "tmux session '$TMUX_SESSION' is already running (use --restart to replace it)."
            [[ -n $ts_url ]] && echo "Chat UI on your tailnet: $ts_url"
            [[ -t 1 && -z ${TMUX:-} ]] && exec tmux attach -t "$TMUX_SESSION"
            exit 0
        fi
    fi

    if (( build )); then
        echo "==> cargo build --release --workspace"
        (cd "$ROOT" && cargo build --release --workspace)
        echo "==> docker build -t cedar-sandbox sandbox/"
        docker build -q -t cedar-sandbox "$ROOT/sandbox"
    fi

    if [[ -n ${VLLM_COMPOSE_FILE:-} ]] && (( ! no_vllm )); then
        echo "==> docker compose up -d ($VLLM_COMPOSE_FILE)"
        docker compose -f "$VLLM_COMPOSE_FILE" up -d
    fi

    # One pane per process (no vLLM pane with --no-vllm). Each drops to a shell when its process
    # exits, so errors stay readable. The panes are fresh processes: NO_VLLM tells them the flag,
    # and CHAT_PUBLIC_URL the tailnet address (a value in .env wins, since the pane loads it after).
    local hold='; echo; echo "[exited with status $?]"; exec bash'
    local pane_env="NO_VLLM=$no_vllm"
    [[ -n $ts_url && -z ${CHAT_PUBLIC_URL:-} ]] && pane_env+=" CHAT_PUBLIC_URL=$ts_url"
    local -a titles=(cedar cedar-chat) cmds=(_cedar _chat)
    if (( ! no_vllm )); then
        titles=(vllm "${titles[@]}")
        cmds=(_vllm "${cmds[@]}")
    fi
    local i
    for i in "${!cmds[@]}"; do
        if (( i == 0 )); then
            tmux new-session -d -s "$TMUX_SESSION" -n stack -c "$ROOT" "$pane_env $SELF ${cmds[i]} $hold"
        else
            tmux split-window -v -t "$TMUX_SESSION:stack" -c "$ROOT" "$pane_env $SELF ${cmds[i]} $hold"
        fi
    done
    tmux select-layout -t "$TMUX_SESSION:stack" even-vertical
    tmux set-option -t "$TMUX_SESSION" pane-border-status top >/dev/null
    for i in "${!titles[@]}"; do
        tmux select-pane -t "$TMUX_SESSION:stack.$i" -T "${titles[i]}"
    done

    echo "Started tmux session '$TMUX_SESSION'. Chat UI: http://$CHAT_BIND_ADDR"
    if [[ -n $ts_url ]]; then
        echo "On your tailnet (iOS and Android apps, other devices): $ts_url"
        echo "To connect the iOS or Android app, run  scripts/cedar-stack.sh --pair  and scan the QR code."
    fi
    if [[ -t 1 && -z ${TMUX:-} ]]; then
        exec tmux attach -t "$TMUX_SESSION"
    elif [[ -n ${TMUX:-} ]]; then
        echo "You are inside tmux: switch with  tmux switch-client -t $TMUX_SESSION"
    else
        echo "Attach with  tmux attach -t $TMUX_SESSION"
    fi
}

case ${1:-} in
    _vllm) pane_vllm ;;
    _cedar) pane_cedar ;;
    _chat) pane_chat ;;
    *) main "$@" ;;
esac
