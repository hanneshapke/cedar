#!/usr/bin/env bash
# End-to-end check of the harness without a GPU: the fixture server, a scripted fake model, a real
# cedar (built from this checkout) pointed at both, and the runner over the tasks the fake model
# knows. Needs Docker and the cedar-sandbox image (see the top-level README's Quick start).
#
#     bench/scripts/smoke.sh            # builds cedar --release first if needed
set -euo pipefail
cd "$(dirname "$0")/../.."
BENCH=bench
WORK=$(mktemp -d)
FX_PORT=${FX_PORT:-8765} LLM_PORT=${LLM_PORT:-8001} CEDAR_PORT=${CEDAR_PORT:-8090}
FX=http://127.0.0.1:$FX_PORT
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$WORK"' EXIT

[ -x target/release/cedar ] || cargo build --release

export PYTHONPATH=$BENCH
python3 -m cedar_bench serve --port "$FX_PORT" >"$WORK/fixtures.log" 2>&1 &
python3 $BENCH/scripts/fake_llm.py --port "$LLM_PORT" >"$WORK/llm.log" 2>&1 &

KEY=$(python3 -c 'import secrets; print(secrets.token_hex(24))')
(
  set -a
  eval "$(python3 -m cedar_bench env --fixtures-url "$FX" --dotenv)"
  CEDAR_API_KEYS=$KEY CEDAR_ADMIN_KEYS=$KEY BIND_ADDR=127.0.0.1:$CEDAR_PORT
  DATABASE_URL=sqlite://$WORK/cedar.db
  set +a
  exec target/release/cedar
) >"$WORK/cedar.log" 2>&1 &

for _ in $(seq 60); do
  curl -sf "http://127.0.0.1:$CEDAR_PORT/readyz" >/dev/null && break
  sleep 1
done
curl -sf "http://127.0.0.1:$CEDAR_PORT/readyz" >/dev/null || { cat "$WORK/cedar.log"; exit 1; }

cat >"$WORK/models.yaml" <<EOF
cedar: {url: "http://127.0.0.1:$CEDAR_PORT", key_env: SMOKE_KEY}
fixtures_url: "$FX"
repeats: 1
trial_timeout_secs: 180
models:
  - id: scripted
    register: {base_url: "http://127.0.0.1:$LLM_PORT/v1", model: scripted,
               price: {input_usd_per_m: 1.0, output_usd_per_m: 2.0}}
EOF
SMOKE_KEY=$KEY python3 -m cedar_bench run --config "$WORK/models.yaml" --out "$WORK/results" --keep-trace \
  --filter 'web/bridge-history' 'weather/umbrella-saturday' 'stocks/compare-quartz' \
           'email_calendar/schedule-with-dana' 'email_calendar/ambiguous-sam' \
  || { tail -50 "$WORK/cedar.log"; exit 1; }
if grep -q '"passed": false' "$WORK/results/results.jsonl"; then
  echo "smoke: some trials failed; their tool calls and results:"
  python3 - "$WORK/results/results.jsonl" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    t = json.loads(line)
    if t["passed"]:
        continue
    print(f"--- {t['task']}: {t['error'] or ''}")
    for e in t["events"]:
        if e["type"] in ("tool.call", "tool.result", "run.failed"):
            print(f"  {e['type']}: {json.dumps(e['data'])[:600]}")
PY
  exit 1
fi
echo "smoke: all trials passed"
