#!/usr/bin/env bash
# One take: fresh stack, record, encode. Usage: take.sh <take-dir>   (writes <take-dir>/cedar-intro.mp4)
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); TAKE=${1:?take dir}
"$HERE/start.sh" "$TAKE/state" >/dev/null
node "$HERE/record.js" "$TAKE/rec"
python3 "$HERE/encode.py" "$TAKE/rec" "$TAKE/cedar-intro.mp4"
ffprobe -v error -show_entries format=duration,size -of default=nw=1 "$TAKE/cedar-intro.mp4"
