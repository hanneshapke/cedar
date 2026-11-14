#!/usr/bin/env python3
"""Cut the short loop from a take: the plan appears, Cedar works, the chart, the PDF. 18 seconds.

    loop.py [lang ...]        (default: every language whose silent take is here)

From ../cedar-intro.mp4 and ../timeline.json (or ../cedar-intro.<lang>.mp4 and its timeline), into
../loop/, with .<lang> before the extension for de, fr and es:

    loop.mp4, loop.webm     1280x720, silent, made to repeat: for a web page (autoplay, muted, loop)
    hero.mp4, hero.webm     1000x800, silent, repeating, zoomed in like the reel: for a small spot on
                            a page, where the whole screen would be too small to read. Copied, with
                            a still picture, to ../../landing/shots/hero-<lang>.* for the landing page
    loop-music.mp4          1920x1080 with the upbeat piece of music.py and a closing card
    reel.mp4                1080x1920 for a reel or a short: each part cropped to what matters, a
                            line of text over it, the same music and closing card

The four parts are found from the scenes' start times, sped up to fit, and cross-faded. Needs
ffmpeg, numpy (for the music) and Node with Playwright (for the text cards).
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.join(HERE, "..")
OUT = os.path.join(ROOT, "loop")
FPS = 30
LENGTH = 18.0           # the loop
RING = LENGTH / 8       # one more bar for the music's last chord, under the closing card
FADE = 0.3              # the cross-fade between two parts
PAPER = "0xf7f5f0"

# The parts: the scene, the seconds into it where the part starts and ends, and how long it lasts
# in the loop (so the first runs at about its own speed and the second at more than twice).
PARTS = [
    ("plan", 0.9, 5.0, 3.9),       # the request, "thinking", the plan
    ("work", -0.6, 18.0, 8.4),     # "start", the Python step, the chart, the PDF, the answer
    ("result", 4.5, 10.5, 3.8),    # the pointer across the chart
    ("result", 13.2, 17.2, 2.8),   # the PDF
]
# The zoomed films (hero, reel) look at the results while Cedar works, where nothing happens until
# the first step lands: their second part starts later and runs slower.
ZOOM_PARTS = [PARTS[0], ("work", 2.8, 18.0, 8.4), PARTS[2], PARTS[3]]
# What the reel shows of each part, in the take's 1920x1080 pixels (x, y, width, height): the
# conversation column for the plan, the results beside it for the rest.
BOX = (1000, 1140)
CROPS = [(0, 76, 587, 669), (621, 82, 933, 998), (621, 82, 933, 998), (621, 82, 933, 998)]
# The same for the hero film, which is wider than tall.
HERO_BOX = (1000, 800)
HERO_CROPS = [(0, 76, 775, 620), (621, 82, 933, 746), (621, 82, 933, 746), (621, 82, 933, 746)]
LANDING = os.path.join(ROOT, "..", "landing", "shots")

TEXT = {
    "en": {"steps": ["It shows you its plan first.", "Then does the work<br>in a sealed box.", "A chart you can explore.", "And the finished PDF."],
           "tagline": "Your own assistant.<br>Runs at home, answers to you.", "free": "Free · Open source"},
    "de": {"steps": ["Erst zeigt es dir seinen Plan.", "Dann arbeitet es<br>in einer abgeschotteten Box.", "Ein Diagramm zum Erkunden.", "Und das fertige PDF."],
           "tagline": "Dein eigener Assistent.<br>Läuft zu Hause, hört auf dich.", "free": "Kostenlos · Open Source"},
    "fr": {"steps": ["D’abord, il vous montre son plan.", "Puis il travaille<br>dans une boîte scellée.", "Un graphique à explorer.", "Et le PDF terminé."],
           "tagline": "Votre propre assistant.<br>Il vit chez vous, à vos ordres.", "free": "Gratuit · Open source"},
    "es": {"steps": ["Primero te muestra su plan.", "Luego trabaja<br>dentro de una caja sellada.", "Un gráfico para explorar.", "Y el PDF terminado."],
           "tagline": "Tu propio asistente.<br>Vive en tu casa, a tus órdenes.", "free": "Gratis · Código abierto"},
}
URL = "github.com/hanneshapke/cedar"


def run(*args):
    subprocess.run([str(a) for a in args], check=True)


def ffmpeg(*args):
    run("ffmpeg", "-loglevel", "error", "-y", *args)


def parts_filter(times, shape, parts=PARTS):
    """The filter that cuts the four parts from input 0, each fitted by `shape(i)`, and cross-fades
    them into [film]; also the times in the loop at which each part takes over."""
    chains, starts, at = [], [0.0], 0.0
    for i, (scene, a, b, length) in enumerate(parts):
        start = times[scene]["start"] + a
        speed = (b - a) / length
        chains.append(f"[0:v]trim={start:.3f}:{start + (b - a):.3f},setpts=(PTS-STARTPTS)/{speed:.4f},"
                      f"fps={FPS},{shape(i)},format=yuv420p,setsar=1[p{i}]")
        if i:
            at += PARTS[i - 1][3] - FADE
            last = "p0" if i == 1 else f"x{i - 1}"
            chains.append(f"[{last}][p{i}]xfade=transition=fade:duration={FADE}:offset={at:.3f}[x{i}]")
            starts.append(at + FADE / 2)
    chains.append(f"[x{len(PARTS) - 1}]trim=0:{LENGTH},setpts=PTS-STARTPTS[film]")
    return ";".join(chains), starts


def build(lang, tmp):
    suffix = "" if lang == "en" else f".{lang}"
    film = os.path.join(ROOT, f"cedar-intro{suffix}.mp4")
    times = {s["id"]: s for s in json.load(open(os.path.join(ROOT, f"timeline{suffix}.json")))["scenes"]}
    out = lambda name, ext: os.path.join(OUT, f"{name}{suffix}.{ext}")
    cards = os.path.join(tmp, lang)
    os.makedirs(cards)
    run("node", os.path.join(HERE, "loop_cards.js"), cards, json.dumps({**TEXT[lang], "url": URL}))
    music = os.path.join(tmp, "upbeat.wav")
    if not os.path.exists(music):
        run("python3", os.path.join(HERE, "music.py"), music, LENGTH + RING, "upbeat")

    # 1. the silent loop, 1280x720: fades from and to the page's paper so the repeat is soft
    graph, _ = parts_filter(times, lambda i: "scale=1280:720:flags=lanczos")
    ends = f"[film]fade=t=in:d=0.25:color={PAPER},fade=t=out:st={LENGTH - 0.25}:d=0.25:color={PAPER}[v]"
    ffmpeg("-i", film, "-filter_complex", f"{graph};{ends}", "-map", "[v]", "-an", "-c:v", "libx264", "-preset", "slow",
           "-crf", "26", "-movflags", "+faststart", out("loop", "mp4"))
    ffmpeg("-i", out("loop", "mp4"), "-an", "-c:v", "libvpx-vp9", "-crf", "36", "-b:v", "0", "-row-mt", "1", out("loop", "webm"))

    # 1b. the hero film: the same parts, zoomed in
    w, h = HERO_BOX
    def zoom(i):
        x, y, cw, ch = HERO_CROPS[i]
        return f"crop={cw}:{ch}:{x}:{y},scale={w}:{h}:flags=lanczos"
    graph, _ = parts_filter(times, zoom, ZOOM_PARTS)
    ffmpeg("-i", film, "-filter_complex", f"{graph};{ends}", "-map", "[v]", "-an", "-c:v", "libx264", "-preset", "slow",
           "-crf", "26", "-movflags", "+faststart", out("hero", "mp4"))
    ffmpeg("-i", out("hero", "mp4"), "-an", "-c:v", "libvpx-vp9", "-crf", "36", "-b:v", "0", "-row-mt", "1", out("hero", "webm"))
    if os.path.isdir(LANDING):
        for ext in ("mp4", "webm"):
            shutil.copyfile(out("hero", ext), os.path.join(LANDING, f"hero-{lang}.{ext}"))
        ffmpeg("-ss", "3.2", "-i", out("hero", "mp4"), "-frames:v", "1", "-q:v", "5", os.path.join(LANDING, f"hero-{lang}.jpg"))

    # the closing card comes in over the film's last moments and stays while the last chord rings
    total = LENGTH + RING
    closing = lambda card: (f"[{card}:v]format=yuva420p,fade=t=in:st={LENGTH - 0.3}:d=0.5:alpha=1[end];"
                            f"[body][end]overlay=enable='gte(t,{LENGTH - 0.3})',fade=t=out:st={total - 0.4}:d=0.4:color={PAPER}[v]")
    audio = ["-map", "1:a", "-c:a", "aac", "-b:a", "192k", "-t", f"{total}", "-movflags", "+faststart"]
    video = ["-c:v", "libx264", "-preset", "slow", "-crf", "19", "-pix_fmt", "yuv420p", "-r", str(FPS)]

    # 2. the same in 1920x1080 with the music
    graph, _ = parts_filter(times, lambda i: "scale=1920:1080")
    ffmpeg("-i", film, "-i", music, "-loop", "1", "-t", total, "-i", os.path.join(cards, "wide-end.png"), "-filter_complex",
           f"{graph};[film]tpad=stop_mode=clone:stop_duration={RING},fade=t=in:d=0.25:color={PAPER}[body];{closing(2)}",
           "-map", "[v]", *video, *audio, out("loop-music", "mp4"))

    # 3. the reel: each part cropped and fitted into the box, on the frame loop_cards.js drew
    w, h = BOX
    def fit(i):
        x, y, cw, ch = CROPS[i]
        return (f"crop={cw}:{ch}:{x}:{y},scale={w}:{h}:force_original_aspect_ratio=decrease:flags=lanczos,"
                f"pad={w}:{h}:(ow-iw)/2:0:color={PAPER}")
    graph, starts = parts_filter(times, fit, ZOOM_PARTS)
    inputs = ["-i", film, "-i", music]
    for i in range(len(PARTS)):
        inputs += ["-loop", "1", "-t", total, "-i", os.path.join(cards, f"reel-{i}.png")]
    inputs += ["-loop", "1", "-t", total, "-i", os.path.join(cards, "reel-end.png")]
    layers = f"color=c={PAPER}:s=1080x1920:r={FPS}:d={total}[bg];[film]tpad=stop_mode=clone:stop_duration={RING}[boxed];[bg][boxed]overlay=40:450[l0]"
    for i in range(len(PARTS)):
        until = starts[i + 1] if i + 1 < len(PARTS) else total
        layers += f";[l{i}][{i + 2}:v]overlay=enable='between(t,{starts[i]:.3f},{until:.3f})'[l{i + 1}]"
    layers += f";[l{len(PARTS)}]fade=t=in:d=0.25:color={PAPER}[body];{closing(len(PARTS) + 2)}"
    ffmpeg(*inputs, "-filter_complex", f"{graph};{layers}", "-map", "[v]", *video, *audio, out("reel", "mp4"))
    print(lang, "done")


def main():
    langs = sys.argv[1:] or [l for l in TEXT if os.path.exists(os.path.join(ROOT, "cedar-intro.mp4" if l == "en" else f"cedar-intro.{l}.mp4"))]
    os.makedirs(OUT, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        for lang in langs:
            build(lang, tmp)


if __name__ == "__main__":
    main()
