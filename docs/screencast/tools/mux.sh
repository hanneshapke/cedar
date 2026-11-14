#!/usr/bin/env bash
# Put the narration under the film: each clip starts 0.4 s into its scene, or right after the
# clip before it if that one runs long (a translation can take a second more than its scene).
#
#   mux.sh <audio-dir> [lang] [out.mp4]
#
# <audio-dir> holds one clip per scene, named like the texts in ../narration (or ../narration/de,
# /fr, /es): 01-title.mp3, 02-first-run.mp3, ... (mp3 or wav). Scenes without a clip stay silent.
# Without lang it takes ../cedar-intro.mp4 and ../timeline.json and writes
# ../cedar-intro-narrated.mp4; with de, fr or es it takes that language's take,
# ../cedar-intro.<lang>.mp4 and ../timeline.<lang>.json, and writes ../cedar-intro-narrated.<lang>.mp4.
#
# Background music goes under the voice, about 12 dB below it when alone and lower still while
# the voice speaks. MUSIC says which: unset or "on" for the piece music.py writes (made for the
# film's length), a path for a file of your own (it is cut and faded to the film's length), or
# "off" for none. MUSIC_DB is its level in dB (default -10).
#
# The language's captions (../captions.srt or ../captions.<lang>.srt, from make_script.py) go in
# as a subtitle track the viewer can switch on and off; SUBS=off leaves it out.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
AUDIO=${1:?audio dir}; shift
SUFFIX=""
case ${1:-} in en) shift ;; de|fr|es) SUFFIX=.$1; shift ;; esac
FILM=$HERE/../cedar-intro$SUFFIX.mp4; TIMELINE=$HERE/../timeline$SUFFIX.json
OUT=${1:-$HERE/../cedar-intro-narrated$SUFFIX.mp4}
[[ -f $FILM && -f $TIMELINE ]] || { echo "no take for this language: $FILM and $TIMELINE are needed (see take.sh)" >&2; exit 1; }
# The output must not be the film itself: ffmpeg would overwrite it while reading it.
if [[ -e $OUT && $OUT -ef $FILM ]]; then
    echo "the output is the silent film mux.sh reads ($OUT); leave the output out or name another file" >&2
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
for i, s in enumerate(json.load(open('$TIMELINE'))['scenes'], 1): print(i, s['id'], s['start'], sep='\t')")
(( n > 0 )) || { echo "no clips found in $AUDIO" >&2; exit 1; }
MUSIC=${MUSIC:-on}; MUSIC_DB=${MUSIC_DB:--10}
# The subtitle track: one more input, after the clips and the music.
CAPTIONS=$HERE/../captions$SUFFIX.srt
case ${SUFFIX#.} in de) CODE=deu ;; fr) CODE=fra ;; es) CODE=spa ;; *) CODE=eng ;; esac
subs_in=(); subs_out=(-metadata:s:a:0 "language=$CODE")
if [[ ${SUBS:-on} != off && -f $CAPTIONS ]]; then
    subs_in=(-i "$CAPTIONS")
    at=$((n + 1)); [[ $MUSIC == off ]] || at=$((n + 2))
    subs_out+=(-map "$at:s" -c:s mov_text -metadata:s:s:0 "language=$CODE" -disposition:s:0 default)
fi
if [[ $MUSIC == off ]]; then
    ffmpeg -loglevel error -y -i "$FILM" "${inputs[@]}" "${subs_in[@]}" \
        -filter_complex "${filter}${mix}amix=inputs=$n:normalize=0[a]" \
        -map 0:v -map "[a]" "${subs_out[@]}" -c:v copy -c:a aac -b:a 192k -movflags +faststart "$OUT"
else
    len=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$FILM")
    tmp=""
    # a file of your own is faded in and out here; the piece from music.py brings its own ending
    fades="afade=t=in:d=2,afade=t=out:st=$(python3 -c "print(max(0, $len - 4))"):d=4"
    if [[ $MUSIC == on ]]; then
        tmp=$(mktemp --suffix=.wav); MUSIC=$tmp; fades=anull
        python3 "$HERE/music.py" "$MUSIC" "$len"
    fi
    [[ -f $MUSIC ]] || { echo "no such music file: $MUSIC" >&2; exit 1; }
    # The voice goes to the mix and, as a side chain, to a compressor that turns the music down
    # while it speaks. The music is the last input.
    ffmpeg -loglevel error -y -i "$FILM" "${inputs[@]}" -stream_loop -1 -i "$MUSIC" "${subs_in[@]}" \
        -filter_complex "${filter}${mix}amix=inputs=$n:normalize=0,aformat=channel_layouts=stereo,apad=whole_dur=$len,asplit[voice][key];\
[$((n + 1)):a]atrim=0:$len,highpass=f=40,volume=${MUSIC_DB}dB,$fades[bed];\
[bed][key]sidechaincompress=threshold=0.015:ratio=3:attack=250:release=1500[ducked];\
[voice][ducked]amix=inputs=2:normalize=0,alimiter=limit=0.95[a]" \
        -map 0:v -map "[a]" "${subs_out[@]}" -c:v copy -c:a aac -b:a 192k -t "$len" -movflags +faststart "$OUT"
    [[ -z $tmp ]] || rm -f "$tmp"
fi
echo "wrote $OUT"
