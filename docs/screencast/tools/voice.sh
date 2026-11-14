#!/usr/bin/env bash
# Make the narration clips with ElevenLabs: one MP3 per scene text, named like the text, which
# is what mux.sh takes.
#
#   ELEVENLABS_API_KEY=... voice.sh [voice-id] [lang] [out-dir]
#
# lang is en (default), de, fr or es; the texts come from ../narration (en) or ../narration/<lang>.
# English has a default voice, so `voice.sh` and `voice.sh en` work without a voice ID; the other
# languages need one (`voice.sh <voice-id> de`).
# out-dir defaults to ../audio (en) or ../audio-<lang>. A clip that already exists is kept, so a
# run that failed half way can be repeated; delete a clip to make it again.
#
# Also read from the environment:
#   ELEVENLABS_MODEL   the model (default eleven_multilingual_v2)
#   ELEVENLABS_FORMAT  the output format (default mp3_44100_128)
#   ELEVENLABS_URL     the API's address (default https://api.elevenlabs.io)
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
DEFAULT_EN_VOICE=itMkRQx56fOF4CJe1D0C
USAGE="usage: ELEVENLABS_API_KEY=... voice.sh [voice-id] [en|de|fr|es] [out-dir]"
# A first argument that is a language means the voice ID was left out.
case ${1:-} in ''|en|de|fr|es) VOICE='' ;; *) VOICE=$1; shift ;; esac
LANG_=${1:-en}
case $LANG_ in en|de|fr|es) ;; *) echo "unknown language '$LANG_'; $USAGE" >&2; exit 1 ;; esac
if [[ -z $VOICE ]]; then
    [[ $LANG_ == en ]] || { echo "'$LANG_' has no default voice: give a voice ID; $USAGE" >&2; exit 1; }
    VOICE=$DEFAULT_EN_VOICE
fi
: "${ELEVENLABS_API_KEY:?set ELEVENLABS_API_KEY to your ElevenLabs API key}"
MODEL=${ELEVENLABS_MODEL:-eleven_multilingual_v2}
FORMAT=${ELEVENLABS_FORMAT:-mp3_44100_128}
URL=${ELEVENLABS_URL:-https://api.elevenlabs.io}
if [[ $LANG_ == en ]]; then TEXTS=$HERE/../narration; OUT=${2:-$HERE/../audio}
else TEXTS=$HERE/../narration/$LANG_; OUT=${2:-$HERE/../audio-$LANG_}; fi
[[ -d $TEXTS ]] || { echo "no narration texts for '$LANG_' ($TEXTS)" >&2; exit 1; }
mkdir -p "$OUT"
failed=0
for f in "$TEXTS"/[0-9][0-9]-*.txt; do
    name=$(basename "$f" .txt); clip=$OUT/$name.mp3
    [[ -s $clip ]] && { echo "kept   $name.mp3"; continue; }
    body=$(python3 -c 'import json, sys; print(json.dumps({"text": open(sys.argv[1]).read().strip(), "model_id": sys.argv[2]}))' "$f" "$MODEL")
    status=$(curl -sS -o "$clip.part" -w '%{http_code}' -X POST "$URL/v1/text-to-speech/$VOICE?output_format=$FORMAT" \
        -H "xi-api-key: $ELEVENLABS_API_KEY" -H 'content-type: application/json' -d "$body") || status=000
    if [[ $status == 200 && -s $clip.part ]]; then
        mv "$clip.part" "$clip"; echo "made   $name.mp3"
    else
        echo "failed $name (HTTP $status): $(head -c 300 "$clip.part" 2>/dev/null)" >&2
        rm -f "$clip.part"; failed=1
    fi
done
(( failed == 0 )) || { echo "some clips are missing; run it again once the cause is fixed" >&2; exit 1; }
echo "clips are in $OUT; next: $HERE/mux.sh $OUT"
