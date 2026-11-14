#!/usr/bin/env python3
"""Write the script, the ElevenLabs narration and the captions from narration.json and a take's
timeline.json.

    make_script.py [timeline.json]     (default: ../timeline.json)

Writes, next to the timeline: script.md, narration-elevenlabs.txt, narration/NN-<scene>.txt (one
clip each) and captions.srt; and for each narration.<lang>.json here, narration-elevenlabs.<lang>.txt,
narration/<lang>/NN-<scene>.txt and captions.<lang>.srt.
"""
import glob, json, os, re, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
timeline_path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "..", "timeline.json")
OUT = os.path.dirname(os.path.abspath(timeline_path))
scenes = json.load(open(os.path.join(HERE, "narration.json")))
timeline = json.load(open(timeline_path))
times = {s["id"]: s for s in timeline["scenes"]}
LEAD = 0.4  # seconds into a scene before its narration starts

# How the text is written for the voice: names a text-to-speech model would misread.
SPOKEN = [("vLLM", "V L L M"), ("llama.cpp", "llama C P P"), ("your OK", "your okay")]


def spoken(text):
    for a, b in SPOKEN:
        text = text.replace(a, b)
    return text


def clip_seconds(lang, index, scene_id):
    """How long the scene's voice clip is, if voice.sh has made it (../audio or ../audio-<lang>)."""
    folder = os.path.join(OUT, "audio" if lang == "en" else f"audio-{lang}")
    for ext in ("mp3", "wav"):
        clip = os.path.join(folder, f"{index:02d}-{scene_id}.{ext}")
        if os.path.exists(clip):
            return float(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of",
                                         "csv=p=0", clip], capture_output=True, text=True, check=True).stdout)
    return None


MAX_CAPTION = 76   # characters: two lines of 38 or so


def pieces(text):
    """The narration cut into captions: sentences, and long ones again near their middle, at a
    comma or a colon if one is there, else between two words."""
    def cut(part):
        if len(part) <= MAX_CAPTION:
            return [part]
        middle = [m.end() for m in re.finditer(r"[,;:] ", part) if 0.3 < m.end() / len(part) < 0.7]
        at = min(middle or [m.end() for m in re.finditer(r" ", part)], key=lambda c: abs(c - len(part) / 2))
        return cut(part[:at].strip()) + cut(part[at:].strip())
    # a sentence ends at . ! or ? before a space, so "llama.cpp" stays whole
    return [p for sentence in re.split(r"(?<=[.!?])\s+", text.strip()) for p in cut(sentence)]


def two_lines(text):
    """A caption over 42 characters, broken into two lines of about the same length."""
    if len(text) <= 42:
        return text
    cut = min((m.start() for m in re.finditer(r" ", text)), key=lambda c: abs(c - len(text) / 2))
    return text[:cut] + "\n" + text[cut + 1:]


def captions(text, start, span, number):
    """SRT entries for one scene's narration, spoken from `start` for `span` seconds; each piece
    gets time by its share of the characters."""
    entries, at = [], start
    for piece in pieces(text):
        d = span * len(piece) / len(text)
        number += 1
        entries += [str(number), f"{clock(at, True)} --> {clock(at + d, True)}", two_lines(piece), ""]
        at += d
    return entries, number


def clock(t, srt=False):
    m, s = divmod(t, 60)
    if srt:
        return f"00:{int(m):02d}:{int(s):02d},{int(round((s % 1) * 1000)) % 1000:03d}"
    return f"{int(m)}:{s:04.1f}"


os.makedirs(os.path.join(OUT, "narration"), exist_ok=True)
md = ["# Cedar: an introduction (script)", "",
      f"Running time {clock(timeline['duration'])}. Times are from `timeline.json` of the take in `cedar-intro.mp4`.",
      "Each scene is at least as long as its narration at about 150 words a minute, so the voice fits when each",
      "clip starts with its scene.", "",
      "| # | Scene | Starts | Ends | Narration |", "| --- | --- | ---: | ---: | ---: |"]
body, voice, srt = [], [], []
n_cap = 0
for i, s in enumerate(scenes, 1):
    t = times[s["id"]]
    words = len(s["text"].split())
    md.append(f"| {i} | {s['title']} | {clock(t['start'])} | {clock(t['end'])} | {words} words, about {words / 2.45:.0f} s |")
    body += [f"## {i}. {s['title']} ({clock(t['start'])} to {clock(t['end'])})", "",
             f"**On screen.** {s['screen']}", "", f"**Narration.** {s['text']}", ""]
    voice += [spoken(s["text"]), ""]
    open(os.path.join(OUT, "narration", f"{i:02d}-{s['id']}.txt"), "w").write(spoken(s["text"]) + "\n")
    # captions: for as long as the voice clip lasts if there is one, else the estimate
    at = t["start"] + LEAD
    span = clip_seconds("en", i, s["id"]) or min(t["end"] - at - 0.2, words / 2.45)
    entries, n_cap = captions(s["text"], at, span, n_cap)
    srt += entries
open(os.path.join(OUT, "script.md"), "w").write("\n".join(md + [""] + body))
open(os.path.join(OUT, "narration-elevenlabs.txt"), "w").write("\n".join(voice).rstrip() + "\n")
open(os.path.join(OUT, "captions.srt"), "w").write("\n".join(srt))
print(f"en: {len(scenes)} scenes, {sum(len(s['text'].split()) for s in scenes)} words, {n_cap} captions")

# The translations (narration.<lang>.json: scene id -> text; terms as in docs/i18n-glossary.md).
# Captions follow the language's own take (timeline.<lang>.json) when there is one, else the
# English one, and the length of each voice clip when voice.sh has made them. Without clips,
# speech is estimated at CPS characters a second, the pace the English text has at 150 words a minute.
CPS = 13.5
for path in sorted(glob.glob(os.path.join(HERE, "narration.*.json"))):
    lang = os.path.basename(path).split(".")[1]
    texts = json.load(open(path))
    # the language's own take, if there is one: its scenes are as long as its voice clips
    own = os.path.join(OUT, f"timeline.{lang}.json")
    has_own = os.path.exists(own)
    times = {s["id"]: s for s in json.load(open(own if has_own else timeline_path))["scenes"]}
    os.makedirs(os.path.join(OUT, "narration", lang), exist_ok=True)
    voice, srt, n_cap, tight = [], [], 0, []
    for i, s in enumerate(scenes, 1):
        text, t = texts[s["id"]], times[s["id"]]
        voice += [spoken(text), ""]
        open(os.path.join(OUT, "narration", lang, f"{i:02d}-{s['id']}.txt"), "w").write(spoken(text) + "\n")
        at = t["start"] + LEAD
        room = t["end"] - at - 0.2
        clip = clip_seconds(lang, i, s["id"])
        need = clip or len(text) / CPS
        if need > room and not clip:
            tight.append(f"{i} {s['id']} (about {need:.0f} s of speech, {room:.0f} s of film)")
        entries, n_cap = captions(text, at, need if clip else min(room, need), n_cap)
        srt += entries
    open(os.path.join(OUT, f"narration-elevenlabs.{lang}.txt"), "w").write("\n".join(voice).rstrip() + "\n")
    open(os.path.join(OUT, f"captions.{lang}.srt"), "w").write("\n".join(srt))
    print(f"{lang}: {sum(len(x.split()) for x in texts.values())} words, {n_cap} captions"
          + ("; may run over: " + "; ".join(tight) if tight else "; every scene fits"))
