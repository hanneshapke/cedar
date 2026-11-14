#!/usr/bin/env bash
# Put the narration under the film: each clip starts 0.4 s into its scene, or right after the
# clip before it if that one runs long (a translation can take a second more than its scene).
#
#   mux.sh <audio-dir> [out.mp4]
#
# <audio-dir> holds one clip per scene, named like the texts in ../narration (or ../narration/de,
# /fr, /es): 01-title.mp3, 02-first-run.mp3, ... (mp3 or wav). Scenes without a clip stay silent.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
AUDIO=${1:?audio dir}; OUT=${2:-$HERE/../cedar-intro-narrated.mp4}
FILM=$HERE/../cedar-intro.mp4
# The output must not be the film itself: ffmpeg would overwrite it while reading it.
if [[ -e $OUT && $OUT -ef $FILM ]]; then
    echo "the output is the silent film mux.sh reads ($OUT); leave the second argument out or name another file" >&2
    exit 1
fi
inputs=(); filter=""; mix=""; n=0; free=0
while IFS=$'\t' read -r idx id start; do
    f=$(ls "$AUDIO"/"$(printf %02d "$idx")-$id".{mp3,wav} 2>/dev/null | head -1 || true)
    [[ -n $f ]] || { echo "no clip for scene $idx ($id), left silent" >&2; continue; }
    n=$((n + 1)); inputs+=(-i "$f")
    len=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$f")
    at=$(python3 -c "print(max($start + 0.4, $free))")
    free=$(python3 -c "print($at + $len + 0.15)")
    python3 -c "import sys; sys.exit(0 if $at > $start + 0.45 else 1)" && echo "scene $idx ($id) starts $(python3 -c "print(round($at - $start - 0.4, 1))") s late: the clip before it runs long" >&2
    ms=$(python3 -c "print(int($at * 1000))")
    filter+="[$n:a]adelay=$ms:all=1[a$n];"; mix+="[a$n]"
done < <(python3 -c "
import json
for i, s in enumerate(json.load(open('$HERE/../timeline.json'))['scenes'], 1): print(i, s['id'], s['start'], sep='\t')")
(( n > 0 )) || { echo "no clips found in $AUDIO" >&2; exit 1; }
ffmpeg -loglevel error -y -i "$FILM" "${inputs[@]}" \
    -filter_complex "${filter}${mix}amix=inputs=$n:normalize=0[a]" \
    -map 0:v -map "[a]" -c:v copy -c:a aac -b:a 192k -movflags +faststart "$OUT"
echo "wrote $OUT"
