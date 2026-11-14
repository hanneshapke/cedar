#!/usr/bin/env python3
"""Write the script, the ElevenLabs narration and the captions from narration.json and a take's
timeline.json.

    make_script.py [timeline.json]     (default: ../timeline.json)

Writes, next to the timeline: script.md, narration-elevenlabs.txt, narration/NN-<scene>.txt (one
clip each) and captions.srt.
"""
import json, os, re, sys

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
    # captions: one per sentence, sharing the narration's estimated time by word count
    sentences = re.findall(r"[^.!?]+[.!?]+", s["text"])
    at = t["start"] + LEAD
    span = min(t["end"] - at - 0.2, words / 2.45)
    for sentence in sentences:
        sentence = sentence.strip()
        d = span * len(sentence.split()) / words
        n_cap += 1
        srt += [str(n_cap), f"{clock(at, True)} --> {clock(at + d, True)}", sentence, ""]
        at += d
open(os.path.join(OUT, "script.md"), "w").write("\n".join(md + [""] + body))
open(os.path.join(OUT, "narration-elevenlabs.txt"), "w").write("\n".join(voice).rstrip() + "\n")
open(os.path.join(OUT, "captions.srt"), "w").write("\n".join(srt))
print(f"{len(scenes)} scenes, {sum(len(s['text'].split()) for s in scenes)} words, {n_cap} captions")
