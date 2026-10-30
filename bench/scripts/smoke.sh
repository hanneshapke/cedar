#!/usr/bin/env bash
# End-to-end check of the harness without a GPU: the fixture server, a scripted fake model, real
# cedar processes (built from this checkout) pointed at both, and the runner over the tasks the
# fake model knows. Needs Docker and the cedar-sandbox image (see the top-level README's Quick start).
#
#     bench/scripts/smoke.sh            # builds cedar --release first if needed
set -euo pipefail
cd "$(dirname "$0")/../.."
BENCH=bench
WORK=$(mktemp -d)
FX_PORT=${FX_PORT:-8765} LLM_PORT=${LLM_PORT:-8001} CEDAR_PORT=${CEDAR_PORT:-8090}
FX=http://127.0.0.1:$FX_PORT
TASKS=('web/bridge-history' 'weather/umbrella-saturday' 'stocks/compare-quartz'
       'email_calendar/schedule-with-dana' 'email_calendar/ambiguous-sam'
       'email_calendar/theo-advisor-slot' 'weather/rosa-market-canopy')
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$WORK"' EXIT

[ -x target/release/cedar ] || cargo build --release

export PYTHONPATH=$BENCH
python3 -m cedar_bench serve --port "$FX_PORT" >"$WORK/fixtures.log" 2>&1 &
python3 $BENCH/scripts/fake_llm.py --port "$LLM_PORT" >"$WORK/llm.log" 2>&1 &

KEY=$(python3 -c 'import secrets; print(secrets.token_hex(24))')
# cedar looks up the calendar time zone once per process, so each persona time zone gets its own
# cedar, all pointed at the same fixture server.
ZONES=(America/Los_Angeles Europe/Berlin America/Chicago)
{
  echo "cedar:"
  for i in "${!ZONES[@]}"; do
    echo "  - {url: \"http://127.0.0.1:$((CEDAR_PORT + i))\", key_env: SMOKE_KEY, timezone: ${ZONES[$i]}}"
  done
  echo "fixtures_url: \"$FX\""
  echo "repeats: 1"
  echo "trial_timeout_secs: 180"
  echo "models:"
  echo "  - id: scripted"
  echo "    register: {base_url: \"http://127.0.0.1:$LLM_PORT/v1\", model: scripted,"
  echo "               price: {input_usd_per_m: 1.0, output_usd_per_m: 2.0}}"
} >"$WORK/models.yaml"

for i in "${!ZONES[@]}"; do
  (
    set -a
    eval "$(python3 -m cedar_bench env --fixtures-url "$FX" --dotenv)"
    CEDAR_API_KEYS=$KEY CEDAR_ADMIN_KEYS=$KEY BIND_ADDR=127.0.0.1:$((CEDAR_PORT + i))
    DATABASE_URL=sqlite://$WORK/cedar-$i.db
    set +a
    exec target/release/cedar
  ) >"$WORK/cedar-$i.log" 2>&1 &
done
for i in "${!ZONES[@]}"; do
  url=http://127.0.0.1:$((CEDAR_PORT + i))/readyz
  for _ in $(seq 60); do
    curl -sf "$url" >/dev/null && break
    sleep 1
  done
  curl -sf "$url" >/dev/null || { cat "$WORK/cedar-$i.log"; exit 1; }
done

SMOKE_KEY=$KEY python3 -m cedar_bench run --config "$WORK/models.yaml" --out "$WORK/results" \
  --keep-trace --filter "${TASKS[@]}" || { tail -50 "$WORK"/cedar-*.log; exit 1; }

python3 - "$WORK/results/results.jsonl" "${#TASKS[@]}" <<'PY'
import json, sys
trials = [json.loads(line) for line in open(sys.argv[1])]
failed = [t for t in trials if not t["passed"]]
for t in failed:
    print(f"--- {t['task']}: {t['error'] or ''}")
    for e in t["events"]:
        if e["type"] in ("tool.call", "tool.result", "run.failed"):
            print(f"  {e['type']}: {json.dumps(e['data'])[:600]}")
if len(trials) != int(sys.argv[2]):
    print(f"smoke: {len(trials)} of {sys.argv[2]} trials ran (see the skips above)")
    sys.exit(1)
if failed:
    print(f"smoke: {len(failed)} trial(s) failed")
    sys.exit(1)
print("smoke: all trials passed")
PY
