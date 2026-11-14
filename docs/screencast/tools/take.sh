#!/usr/bin/env bash
# One take: fresh stack, record, encode.
#
#   take.sh <take-dir> [lang] [audio-dir]
#
# lang is en (default), de, fr or es. With audio-dir (the clips voice.sh made for that language)
# each scene is as long as its clip. Writes <take-dir>/cedar-intro.mp4 and <take-dir>/rec/timeline.json.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); TAKE=${1:?take dir}; LANG_=${2:-en}; AUDIO=${3:-}
CEDAR_DEMO_LANG=$LANG_ "$HERE/start.sh" "$TAKE/state" >/dev/null
node "$HERE/record.js" "$TAKE/rec" "$LANG_" $AUDIO
python3 "$HERE/encode.py" "$TAKE/rec" "$TAKE/cedar-intro.mp4"
ffprobe -v error -show_entries format=duration,size -of default=nw=1 "$TAKE/cedar-intro.mp4"
