#!/usr/bin/env bash
# Build cedar and cedar-chat, then run vLLM, cedar, and the chat UI in one tmux session.
#
#   scripts/dev-up.sh            build, start everything, attach
#   scripts/dev-up.sh --no-build skip cargo and docker builds
#   scripts/dev-up.sh --restart  kill the existing session first
#   scripts/dev-up.sh --down     kill the session; vLLM keeps running
#   scripts/dev-up.sh --down --with-vllm
#                                also stop vLLM (docker compose down)
#   scripts/dev-up.sh --down-all same as --down --with-vllm
#
# Settings come from .env in the repo root (created on first run with fresh keys). Besides
# cedar's and cedar-chat's own variables it reads:
#   VLLM_COMPOSE_FILE  compose file that runs vLLM; if set, the script starts it and shows its logs
#   TMUX_SESSION       session name (default: cedar)
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ENV_FILE="$ROOT/.env"
SELF="$ROOT/scripts/dev-up.sh"

die() { echo "error: $*" >&2; exit 1; }

load_env() {
    [[ -f $ENV_FILE ]] || die "$ENV_FILE not found; run scripts/dev-up.sh first"
    set -a
    # shellcheck disable=SC1090
    . "$ENV_FILE"
    set +a
    : "${CEDAR_API_KEYS:?CEDAR_API_KEYS must be set in $ENV_FILE}"
    BIND_ADDR=${BIND_ADDR:-127.0.0.1:8080}
    LLM_BASE_URL=${LLM_BASE_URL:-http://localhost:8000/v1}
    CHAT_BIND_ADDR=${CHAT_BIND_ADDR:-127.0.0.1:3000}
    TMUX_SESSION=${TMUX_SESSION:-cedar}
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
# Local settings for scripts/dev-up.sh (gitignored). See .env.example and chat/README.md.

# cedar
CEDAR_API_KEYS=$key
BIND_ADDR=127.0.0.1:8080
LLM_BASE_URL=http://localhost:8000/v1
# LLM_MODEL=              # detected from vLLM if unset
MAX_CONCURRENT_RUNS=2
RUN_TIMEOUT_SECS=1800
RUST_LOG=info,sqlx=warn
# Give the sandbox network access so the agent can \`pip install\` packages. Set to "none" to keep
# it offline (the agent then only has what sandbox/Dockerfile installs).
SANDBOX_NETWORK=bridge

# cedar-chat (its CEDAR_API_KEY is the first of CEDAR_API_KEYS)
# Set this to your Tailscale IP (e.g. 100.x.y.z:3000) to open the UI from other devices.
CHAT_BIND_ADDR=127.0.0.1:3000
CHAT_ACCESS_TOKEN=$token

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
    wait_for "$LLM_BASE_URL/models" vLLM
    if [[ -z ${LLM_MODEL:-} ]]; then
        LLM_MODEL=$(curl -fsS "$LLM_BASE_URL/models" | jq -r '.data[0].id')
        export LLM_MODEL
        echo "LLM_MODEL=$LLM_MODEL (from vLLM)"
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
    local build=1 restart=0 stop=0 with_vllm=0
    for arg in "$@"; do
        case $arg in
            --no-build) build=0 ;;
            --restart) restart=1 ;;
            --down) stop=1 ;;
            --with-vllm) with_vllm=1 ;;
            --down-all) stop=1; with_vllm=1 ;;
            -h|--help) sed -n '2,15p' "$SELF" | sed 's/^# \{0,1\}//'; exit 0 ;;
            *) die "unknown option: $arg" ;;
        esac
    done
    if (( stop )); then
        down "$with_vllm"
        exit 0
    fi
    (( with_vllm )) && die "--with-vllm only applies to --down"

    for cmd in cargo docker tmux curl jq openssl; do
        command -v "$cmd" >/dev/null || die "$cmd is not installed"
    done
    [[ -f $ENV_FILE ]] || create_env
    load_env

    if tmux has-session -t "$TMUX_SESSION" 2>/dev/null; then
        if (( restart )); then
            tmux kill-session -t "$TMUX_SESSION"
        else
            echo "tmux session '$TMUX_SESSION' is already running (use --restart to replace it)."
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

    if [[ -n ${VLLM_COMPOSE_FILE:-} ]]; then
        echo "==> docker compose up -d ($VLLM_COMPOSE_FILE)"
        docker compose -f "$VLLM_COMPOSE_FILE" up -d
    fi

    # Each pane drops to a shell when its process exits, so errors stay readable.
    local hold='; echo; echo "[exited with status $?]"; exec bash'
    tmux new-session -d -s "$TMUX_SESSION" -n stack -c "$ROOT" "$SELF _vllm $hold"
    tmux split-window -v -t "$TMUX_SESSION:stack" -c "$ROOT" "$SELF _cedar $hold"
    tmux split-window -v -t "$TMUX_SESSION:stack" -c "$ROOT" "$SELF _chat $hold"
    tmux select-layout -t "$TMUX_SESSION:stack" even-vertical
    tmux set-option -t "$TMUX_SESSION" pane-border-status top >/dev/null
    tmux select-pane -t "$TMUX_SESSION:stack.0" -T vllm
    tmux select-pane -t "$TMUX_SESSION:stack.1" -T cedar
    tmux select-pane -t "$TMUX_SESSION:stack.2" -T cedar-chat

    echo "Started tmux session '$TMUX_SESSION'. Chat UI: http://$CHAT_BIND_ADDR"
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
