#!/usr/bin/env bash
# Start the scripted model, cedar and cedar-chat with fresh databases. Usage: start.sh <run-dir>
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=${CEDAR_ROOT:-$(cd "$HERE/../../.." && pwd)}
RUN=${1:?run dir}
pkill -f mock_llm.py || true; pkill -x cedar || true; pkill -x cedar-chat || true
docker ps -aq --filter name=cedar | xargs -r docker rm -f >/dev/null 2>&1 || true
rm -rf "$RUN"; mkdir -p "$RUN"; rm -f "$HERE/requests.jsonl"
KEY=demo-key-0123456789abcdef
( cd "$HERE" && nohup python3 mock_llm.py 8000 >"$RUN/mock.log" 2>&1 & )
( cd "$RUN" && env -u HTTPS_PROXY -u https_proxy CEDAR_API_KEYS=$KEY CEDAR_ADMIN_KEYS=$KEY \
    CEDAR_SECRET_KEY=$(printf 'c%.0s' {1..64}) DATABASE_URL=sqlite://$RUN/cedar.db \
    ${CEDAR_ENV:-} nohup "$ROOT/target/release/cedar" >"$RUN/cedar.log" 2>&1 & )
( cd "$ROOT/chat" && env -u HTTPS_PROXY -u https_proxy CEDAR_API_KEY=$KEY CEDAR_URL=http://127.0.0.1:8080 \
    CHAT_DATABASE_URL=sqlite://$RUN/chat.db CHAT_UPDATE_CHECK=off \
    nohup "$ROOT/target/release/cedar-chat" >"$RUN/chat.log" 2>&1 & )
for i in $(seq 40); do curl -sf localhost:3000/ >/dev/null && curl -sf localhost:8080/healthz >/dev/null && break; sleep 0.5; done
curl -s localhost:8080/readyz; echo
