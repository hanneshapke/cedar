#!/usr/bin/env bash
# Build cedar and cedar-chat, then run vLLM, cedar, and the chat UI in one tmux session. If
# Tailscale is installed and logged in, the chat UI is also served over HTTPS on the tailnet
# (`tailscale serve`), which is what the iOS app needs.
#
# This runs the stack on one machine you are logged in to. Nothing restarts it after a crash or a
# reboot; for a long-running server use the systemd setup in DEPLOYMENT.md.
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
#   scripts/cedar-stack.sh --pair     print the QR code the iOS app scans to connect (its address
#                                     on the tailnet and the access token); needs qrencode
#
# Settings come from .env in the repo root (created on first run with fresh keys). Besides
# cedar's and cedar-chat's own variables it reads:
#   VLLM_COMPOSE_FILE  compose file that runs vLLM; if set, the script starts it and shows its logs
#   TMUX_SESSION       session name (default: cedar)
#   TAILSCALE_SERVE    auto (default: serve if tailscale is logged in), on (fail if it can't), off
#   TAILSCALE_HTTPS_PORT
#                      the tailnet HTTPS port to serve the chat UI on (default: 443)
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

# --- pairing the iOS app ------------------------------------------------------------------

# pair: print the QR code the iOS app scans under Settings → Scan QR Code. It carries
# cedar://connect?server=<url>&token=<token>, the same link the Mac app shows (pairing/src/connect.rs;
# the iOS app reads it in ConnectionCode.swift). The address is CHAT_PUBLIC_URL when that is HTTPS,
# else the tailnet address `tailscale serve` gives the chat UI.
pair() {
    load_env
    for cmd in curl jq; do
        command -v "$cmd" >/dev/null || die "$cmd is not installed"
    done
    [[ -n ${CHAT_ACCESS_TOKEN:-} ]] || die "CHAT_ACCESS_TOKEN is not set in $ENV_FILE; the iOS app needs it to connect"
    local server
    if [[ ${CHAT_PUBLIC_URL:-} == https://* ]]; then
        server=${CHAT_PUBLIC_URL%/}
    else
        [[ $TAILSCALE_SERVE == off ]] && die "TAILSCALE_SERVE=off and CHAT_PUBLIC_URL is not an https:// address; the iOS app connects over HTTPS only"
        tailscale_ready || die "tailscale is not installed or not logged in (run: sudo tailscale up); the iOS app connects over HTTPS only"
        server=$(tailscale_url) || die "tailscale has no name for this machine; turn on MagicDNS at https://login.tailscale.com/admin/dns"
    fi
    curl -fs -m 5 -o /dev/null "$server/" ||
        echo "warning: $server does not answer yet; start the stack (scripts/cedar-stack.sh) before you scan." >&2
    local link
    link="cedar://connect?server=$(jq -rn --arg v "$server" '$v|@uri')&token=$(jq -rn --arg v "$CHAT_ACCESS_TOKEN" '$v|@uri')"
    echo "In the iOS app's Settings, tap Scan QR Code and scan this. Or enter $server and the"
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
# (https://<machine>.<tailnet>.ts.net), which the iOS app and other devices use.
CHAT_BIND_ADDR=127.0.0.1:3000
CHAT_ACCESS_TOKEN=$token
# auto: serve over Tailscale if it is logged in; on: fail if it can't; off: never.
TAILSCALE_SERVE=auto
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

    for cmd in cargo docker tmux curl jq openssl; do
        command -v "$cmd" >/dev/null || die "$cmd is not installed"
    done
    # cedar-chat's Web Push encryption links OpenSSL; catch missing headers before a long build.
    if (( build )) && [[ $(uname -s) == Linux ]] && ! pkg-config --exists openssl 2>/dev/null; then
        die "OpenSSL headers not found (needed to build cedar-chat); install them: sudo apt install pkg-config libssl-dev"
    fi
    [[ -f $ENV_FILE ]] || create_env
    load_env

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
        echo "On your tailnet (iOS app, other devices): $ts_url"
        echo "To connect the iOS app, run  scripts/cedar-stack.sh --pair  and scan the QR code."
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
