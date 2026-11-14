#!/usr/bin/env python3
"""Turn a take's timestamped frames into a constant 30 fps H.264 file.

    encode.py <rec-dir> <out.mp4>

Each output frame is the latest captured frame at that instant, so the film runs in real time
however unevenly the frames were captured.
"""
import json, os, shutil, subprocess, sys

rec, out = sys.argv[1], sys.argv[2]
FPS = 30
times, names = [], []
t = 0.0
for line in open(os.path.join(rec, "frames.txt")):
    kind, _, value = line.strip().partition(" ")
    if kind == "file":
        names.append(value.strip("'")); times.append(t)
    elif kind == "duration":
        t += float(value)
duration = json.load(open(os.path.join(rec, "timeline.json")))["duration"]
seq = os.path.join(rec, "seq")
shutil.rmtree(seq, ignore_errors=True); os.makedirs(seq)
i = 0
for k in range(int(duration * FPS)):
    while i + 1 < len(times) and times[i + 1] <= k / FPS:
        i += 1
    os.symlink(os.path.abspath(os.path.join(rec, names[i])), os.path.join(seq, f"{k:06d}.jpg"))
subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-framerate", str(FPS), "-i", os.path.join(seq, "%06d.jpg"),
                "-vf", "format=yuv420p", "-c:v", "libx264", "-preset", "slow", "-crf", "17",
                "-movflags", "+faststart", out], check=True)
shutil.rmtree(seq)
