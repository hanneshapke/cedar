#!/usr/bin/env bash
# Starts and stops what a benchmark run needs: the fixture server and one cedar per persona time
# zone, all pointed at the fixtures, sharing one fresh API key. Logs, pids, databases and the key
# live in bench/.stack/ (git-ignored).
#
#     bench/scripts/stack.sh start          # stops a previous stack first
#     bench/scripts/stack.sh run --repeats 3 # the runner, with the stack's key (any run flags)
#     bench/scripts/stack.sh status | stop | logs | env
#
# Python runs through uv with bench/pyproject.toml, so its dependencies install themselves.
# Ports: FX_PORT (8765) and CEDAR_PORTS ("8080 8081 8082", matching models.example.yaml).
# Works with macOS's bash 3.2.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
STATE=$ROOT/bench/.stack
FX_PORT=${FX_PORT:-8765}
CEDAR_PORTS=${CEDAR_PORTS:-"8080 8081 8082"}
command -v uv >/dev/null || { echo "stack.sh needs uv: https://docs.astral.sh/uv/" >&2; exit 1; }
py() { uv run --quiet --project "$ROOT/bench" python "$@"; }
CEDAR_BIN=${CEDAR_BIN:-$ROOT/target/release/cedar}

alive() { [ -f "$1" ] && kill -0 "$(cat "$1")" 2>/dev/null; }

listener() {  # the pid listening on a port, if any
  lsof -nP -iTCP:"$1" -sTCP:LISTEN -t 2>/dev/null | head -1 || true
}

ours() {  # whether a pid is this checkout's cedar or a cedar-bench fixture server
  local cmd
  cmd=$(ps -o command= -p "$1" 2>/dev/null || true)
  case "$cmd" in
    "$CEDAR_BIN"*|*"-m cedar_bench serve"*) return 0 ;;
  esac
  return 1
}

stop() {
  local f pid port pids=""
  for f in "$STATE"/*.pid; do
    [ -e "$f" ] || continue
    alive "$f" && pids="$pids $(cat "$f")"
    rm -f "$f"
  done
  # Also ours on the bench ports that this script didn't start (started by hand, or by an
  # earlier stack whose pid files are gone). Anything else on those ports is left alone.
  for port in $FX_PORT $CEDAR_PORTS; do
    pid=$(listener "$port")
    if [ -n "$pid" ] && ours "$pid"; then
      case " $pids " in *" $pid "*) ;; *) echo "stopping pid $pid on port $port (started outside stack.sh)"; pids="$pids $pid" ;; esac
    fi
  done
  [ -n "$pids" ] || return 0
  kill $pids 2>/dev/null || true
  for _ in 1 2 3 4 5 6 7 8 9 10; do  # cedar cancels active runs before it exits
    alive_any=""
    for pid in $pids; do kill -0 "$pid" 2>/dev/null && alive_any=1; done
    [ -z "$alive_any" ] && return 0
    sleep 1
  done
  echo "still running after 10s, killing:$pids" >&2
  kill -9 $pids 2>/dev/null || true
}

wait_ready() {  # name pidfile url
  local i
  for i in $(seq 90); do
    if ! alive "$2"; then
      echo "$1 exited while starting; its log:" >&2
      tail -20 "$STATE/$1.log" >&2
      stop
      exit 1
    fi
    if curl -sf "$3" >/dev/null; then echo "$1 ready"; return; fi
    sleep 1
  done
  echo "$1 is running but not ready after 90s ($3):" >&2
  curl -s "$3" >&2 || true
  echo >&2
  echo "(a failed sandbox check usually means Docker isn't running or the cedar-sandbox image is missing)" >&2
  stop
  exit 1
}

start() {
  [ -x "$CEDAR_BIN" ] || { echo "no cedar binary at $CEDAR_BIN: run cargo build --release" >&2; exit 1; }
  stop
  local port pid busy=""
  for port in $FX_PORT $CEDAR_PORTS; do
    pid=$(listener "$port")
    if [ -n "$pid" ]; then
      echo "port $port is used by pid $pid: $(ps -o command= -p "$pid" 2>/dev/null | cut -c1-100)" >&2
      busy="$busy $pid"
    fi
  done
  if [ -n "$busy" ]; then
    echo "These aren't cedar-bench's, so they were left alone. Stop them (kill$busy), or pick" >&2
    echo "other ports with FX_PORT / CEDAR_PORTS (and the same ports in models.yaml)." >&2
    exit 1
  fi
  mkdir -p "$STATE"
  rm -f "$STATE"/*.db "$STATE"/*.db-* "$STATE"/*.log
  KEY=$(py -c 'import secrets; print(secrets.token_hex(24))')
  (umask 077; echo "$KEY" >"$STATE/key")
  # Encrypts the model API keys cedar saves when the runner registers models (each start uses fresh
  # databases, so a fresh secret is fine).
  local secret
  secret=$(py -c 'import secrets; print(secrets.token_hex(32))')

  # uv sets up the environment, but the server runs on its interpreter directly, so the pid we
  # keep is the server's own and `stop` really stops it.
  local python
  python=$(py -c 'import sys; print(sys.executable)')
  "$python" -m cedar_bench serve --port "$FX_PORT" >"$STATE/fixtures.log" 2>&1 &
  echo $! >"$STATE/fixtures.pid"
  wait_ready fixtures "$STATE/fixtures.pid" "http://127.0.0.1:$FX_PORT/_admin/health"

  local env_lines
  env_lines=$(py -m cedar_bench env --fixtures-url "http://127.0.0.1:$FX_PORT" --dotenv)
  for port in $CEDAR_PORTS; do
    (
      set -a
      eval "$env_lines"
      CEDAR_API_KEYS=$KEY CEDAR_ADMIN_KEYS=$KEY CEDAR_SECRET_KEY=$secret BIND_ADDR=127.0.0.1:$port
      DATABASE_URL=sqlite://$STATE/cedar-$port.db
      set +a
      exec "$CEDAR_BIN"
    ) >"$STATE/cedar-$port.log" 2>&1 &
    echo $! >"$STATE/cedar-$port.pid"
  done
  for port in $CEDAR_PORTS; do
    wait_ready "cedar-$port" "$STATE/cedar-$port.pid" "http://127.0.0.1:$port/readyz"
  done
  echo
  echo "Next:  $0 run --repeats 3"
}

status() {
  local name f
  for f in "$STATE"/*.pid; do
    [ -e "$f" ] || { echo "nothing started"; return; }
    name=$(basename "$f" .pid)
    if alive "$f"; then echo "$name: running (pid $(cat "$f"))"; else echo "$name: stopped"; fi
  done
}

case "${1:-}" in
  start) start ;;
  stop) stop; echo "stopped" ;;
  status) status ;;
  env)
    [ -f "$STATE/key" ] || { echo "no key: run $0 start first" >&2; exit 1; }
    echo "export CEDAR_BENCH_KEY=$(cat "$STATE/key")"
    echo "unset CEDAR_BENCH_ADMIN_KEY"
    ;;
  logs) tail -n 30 "$STATE"/*.log ;;
  run)
    shift
    [ -f "$STATE/key" ] || { echo "no key: run $0 start first" >&2; exit 1; }
    CEDAR_BENCH_KEY=$(cat "$STATE/key") CEDAR_BENCH_ADMIN_KEY= py -m cedar_bench run "$@"
    ;;
  *) echo "usage: $0 start|stop|status|run [run flags]|env|logs" >&2; exit 2 ;;
esac
