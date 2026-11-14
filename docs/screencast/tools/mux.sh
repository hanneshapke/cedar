#!/usr/bin/env bash
# Put the narration under the film: each clip starts 0.4 s into its scene.
#
#   mux.sh <audio-dir> [out.mp4]
#
# <audio-dir> holds one clip per scene, named like the texts in ../narration:
# 01-title.mp3, 02-first-run.mp3, ... (mp3 or wav). Scenes without a clip stay silent.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
AUDIO=${1:?audio dir}; OUT=${2:-$HERE/../cedar-intro-narrated.mp4}
inputs=(); filter=""; mix=""; n=0
while IFS=$'\t' read -r idx id start; do
    f=$(ls "$AUDIO"/"$(printf %02d "$idx")-$id".{mp3,wav} 2>/dev/null | head -1 || true)
    [[ -n $f ]] || { echo "no clip for scene $idx ($id), left silent" >&2; continue; }
    n=$((n + 1)); inputs+=(-i "$f")
    ms=$(python3 -c "print(int(($start + 0.4) * 1000))")
    filter+="[$n:a]adelay=$ms:all=1[a$n];"; mix+="[a$n]"
done < <(python3 -c "
import json
for i, s in enumerate(json.load(open('$HERE/../timeline.json'))['scenes'], 1): print(i, s['id'], s['start'], sep='\t')")
(( n > 0 )) || { echo "no clips found in $AUDIO" >&2; exit 1; }
ffmpeg -loglevel error -y -i "$HERE/../cedar-intro.mp4" "${inputs[@]}" \
    -filter_complex "${filter}${mix}amix=inputs=$n:normalize=0[a]" \
    -map 0:v -map "[a]" -c:v copy -c:a aac -b:a 192k -movflags +faststart "$OUT"
echo "wrote $OUT"
