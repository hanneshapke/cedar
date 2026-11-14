#!/usr/bin/env python3
"""Write the background music for the screencast: a quiet ambient piece, made here from sine
waves so it is free to use and the same every time.

    music.py <out.wav> [seconds] [calm|upbeat] [bars]      (default 240 seconds, calm; needs numpy)

A slow pad moves through eight chords in D major, two bars each at 66 beats a minute, over a soft
bass, with a sparse bell-like figure on top and a long reverb. It is written to sit under a
voice: little happens between 1 and 4 kHz, nothing has a hard attack, and it ends on the home
chord when the time is up. mux.sh mixes it in, well below the narration.

"upbeat" is a second piece, for the short loop (loop.py): nine bars (or [bars]) to the given length, all but one of
a driving groove (kick, clap, hats, a bass and a sixteenth-note figure over I vi IV V in D) and a
last bar for the final chord to ring. It is meant to be heard on its own, not under a voice.
"""
import sys
import wave

import numpy as np

SR = 44100
BPM = 66
BAR = 4 * 60 / BPM
CHORD_BARS = 2

# The progression, as MIDI notes: (bass, pad voices). D major, no thirds jumping about: each
# chord shares notes with the next so the pad moves by small steps.
CHORDS = [
    (38, [57, 62, 66, 69, 73]),   # Dmaj7
    (35, [57, 62, 66, 69, 74]),   # Bm7
    (31, [59, 62, 66, 69, 74]),   # Gmaj7
    (33, [57, 61, 64, 69, 71]),   # A6/9
    (38, [57, 62, 66, 69, 73]),   # Dmaj7
    (40, [59, 62, 66, 67, 74]),   # Em9
    (31, [59, 62, 66, 69, 74]),   # Gmaj7
    (33, [57, 62, 64, 69, 71]),   # Asus
]
HOME = (38, [57, 62, 66, 69, 74])  # the last chord: D, with nothing left to resolve


def hz(note):
    return 440.0 * 2 ** ((note - 69) / 12)


def envelope(n, attack, release):
    """Rise over `attack` seconds, hold, fall over `release` seconds; n samples in all."""
    env = np.ones(n)
    a, r = min(int(attack * SR), n // 2), min(int(release * SR), n // 2)
    env[:a] = np.sin(np.linspace(0, np.pi / 2, a)) ** 2
    env[n - r:] = np.cos(np.linspace(0, np.pi / 2, r)) ** 2
    return env


def pad_note(note, seconds, rng):
    """One pad voice in stereo: a few slightly detuned sines per side, with a slow shimmer."""
    n = int(seconds * SR)
    t = np.arange(n) / SR
    f = hz(note)
    out = np.zeros((n, 2))
    for side in (0, 1):
        for detune, level in ((-0.0025, 0.5), (0.0, 1.0), (0.0031, 0.5)):
            phase = rng.uniform(0, 2 * np.pi)
            drift = 1 + detune + 0.0006 * np.sin(2 * np.pi * rng.uniform(0.05, 0.12) * t + phase)
            wave_ = np.sin(2 * np.pi * f * drift * t + phase)
            wave_ += 0.10 * np.sin(2 * np.pi * 2 * f * drift * t + phase * 1.7)   # a little warmth
            wave_ += 0.02 * np.sin(2 * np.pi * 3 * f * drift * t + phase * 0.6)
            out[:, side] += level * wave_
    shimmer = 1 + 0.12 * np.sin(2 * np.pi * rng.uniform(0.08, 0.16) * t + rng.uniform(0, 6))
    # the higher a voice, the quieter: that keeps the pad dark and out of the narrator's range
    level = 1.0 if note <= 62 else 0.7 if note <= 69 else 0.4
    return out * (shimmer * envelope(n, 2.2, 3.0))[:, None] * level


def bass_note(note, seconds):
    n = int(seconds * SR)
    t = np.arange(n) / SR
    f = hz(note)
    tone = np.sin(2 * np.pi * f * t) + 0.3 * np.sin(2 * np.pi * 2 * f * t)
    return np.repeat((tone * envelope(n, 0.6, 2.5))[:, None], 2, axis=1)


def bell(note, velocity, pan):
    """A soft struck tone, like a kalimba heard from the next room."""
    n = int(3.5 * SR)
    t = np.arange(n) / SR
    f = hz(note)
    tone = np.sin(2 * np.pi * f * t) * np.exp(-t * 1.6)
    tone += 0.35 * np.sin(2 * np.pi * 2 * f * t) * np.exp(-t * 3.5)
    tone += 0.10 * np.sin(2 * np.pi * 4.07 * f * t) * np.exp(-t * 7.0)
    tone *= np.minimum(1, t / 0.012)          # no click at the start
    return np.stack([tone * velocity * (1 - pan), tone * velocity * pan], axis=1)


def add(mix, sound, at):
    start = int(at * SR)
    if start >= len(mix):
        return
    end = min(len(mix), start + len(sound))
    mix[start:end] += sound[:end - start]


def reverb(dry, rng, seconds=3.2, wet=0.42):
    """Convolve with a burst of noise that dies away: a large, dark room."""
    n = int(seconds * SR)
    t = np.arange(n) / SR
    out = np.empty_like(dry)
    size = 1 << int(np.ceil(np.log2(len(dry) + n)))
    for side in (0, 1):
        ir = rng.standard_normal(n) * np.exp(-t * 2.1)
        # darken the tail: a running mean is a cheap low-pass
        ir = np.convolve(ir, np.ones(24) / 24, mode="same")
        ir[: int(0.02 * SR)] *= np.linspace(0, 1, int(0.02 * SR))
        ir /= np.sqrt(np.sum(ir ** 2))
        tail = np.fft.irfft(np.fft.rfft(dry[:, side], size) * np.fft.rfft(ir, size), size)[: len(dry)]
        out[:, side] = (1 - wet) * dry[:, side] + wet * tail
    return out


def compose(seconds):
    rng = np.random.default_rng(11)
    total = int(seconds * SR)
    pads, bells, bass = np.zeros((total, 2)), np.zeros((total, 2)), np.zeros((total, 2))
    span = CHORD_BARS * BAR
    # Whole chords only: the last one takes whatever time is left (half a chord to a chord and a
    # half), so the piece never ends on a chord that has only just begun.
    count = max(1, int(round(seconds / span)))
    for i in range(count):
        at = i * span
        last = i == count - 1
        length = max(span, seconds - at) if last else span
        root, voices = HOME if last else CHORDS[i % len(CHORDS)]
        for note in voices:
            add(pads, pad_note(note, length + 3.0, rng), max(0, at - 1.0))
        add(bass, bass_note(root + 12, length + 1.5), at)   # an octave up: small speakers have no 50 Hz
        if i == 0 or last:
            continue                           # the first chord and the last are left bare
        # the figure: four to six notes from the chord, on the half beats, never the same twice
        slots = sorted(rng.choice(np.arange(1, 16), size=int(rng.integers(4, 7)), replace=False))
        for slot in slots:
            note = int(rng.choice(voices[1:])) + (12 if rng.random() < 0.6 else 0)
            add(bells, bell(note, rng.uniform(0.35, 0.8), rng.uniform(0.25, 0.75)), at + slot * BAR / 8)
            if rng.random() < 0.5:             # an echo, a dotted quarter later and quieter
                add(bells, bell(note, 0.22, rng.uniform(0.15, 0.85)), at + slot * BAR / 8 + 1.5 * 60 / BPM)
    mix = 0.20 * pads + 0.20 * bass + 0.34 * bells
    mix = reverb(mix, rng)
    # fade in and out, then set the level: about -24 dBFS RMS, with the peaks well clear of full scale
    fade_in, fade_out = int(2.5 * SR), int(min(6.0, seconds / 4) * SR)
    mix[:fade_in] *= np.linspace(0, 1, fade_in)[:, None] ** 2
    mix[-fade_out:] *= np.linspace(1, 0, fade_out)[:, None] ** 2
    mix *= 10 ** (-24 / 20) / np.sqrt(np.mean(mix ** 2))
    peak = np.max(np.abs(mix))
    if peak > 0.7:
        mix *= 0.7 / peak
    return mix


# --- the upbeat piece, for the short loop ------------------------------------------------------

def kick():
    n = int(0.32 * SR)
    t = np.arange(n) / SR
    pitch = 55 + 110 * np.exp(-t * 38)                      # a sine that falls from 165 Hz to 55
    tone = np.sin(2 * np.pi * np.cumsum(pitch) / SR) * np.exp(-t * 11)
    tone[: int(0.004 * SR)] *= np.linspace(0, 1, int(0.004 * SR))
    return np.repeat(tone[:, None], 2, axis=1)


def noise_hit(rng, seconds, decay, bright):
    """Filtered noise: a hat (short, bright), a clap (longer, mid) or a crash (long)."""
    n = int(seconds * SR)
    t = np.arange(n) / SR
    out = np.empty((n, 2))
    for side in (0, 1):
        x = rng.standard_normal(n)
        x = np.diff(x, prepend=0.0) if bright else np.convolve(x, np.ones(6) / 6, mode="same") - np.convolve(x, np.ones(60) / 60, mode="same")
        out[:, side] = x * np.exp(-t * decay)
    return out / np.max(np.abs(out))


def pluck(note, seconds=0.5):
    n = int(seconds * SR)
    t = np.arange(n) / SR
    f = hz(note)
    tone = sum(level * np.sin(2 * np.pi * k * f * t) * np.exp(-t * (7 + 3 * k)) for k, level in ((1, 1.0), (2, 0.5), (3, 0.28), (4, 0.12)))
    tone *= np.minimum(1, t / 0.004)
    return tone


def synth_bass(note, seconds):
    n = int(seconds * SR)
    t = np.arange(n) / SR
    f = hz(note)
    tone = sum(np.sin(2 * np.pi * k * f * t) / k for k in (1, 2, 3, 4))
    return np.repeat((tone * np.minimum(1, t / 0.006) * np.exp(-t * 3.5))[:, None], 2, axis=1)


def stab(notes, seconds):
    """A chord, bright and short at the start with a tail: the pad of the upbeat piece."""
    n = int(seconds * SR)
    t = np.arange(n) / SR
    out = np.zeros((n, 2))
    for i, note in enumerate(notes):
        f = hz(note)
        for side, detune in ((0, 0.997), (1, 1.003)):
            out[:, side] += sum(np.sin(2 * np.pi * k * f * detune * t + i) / k ** 1.4 for k in (1, 2, 3))
    return out * (np.minimum(1, t / 0.02) * (0.35 + 0.65 * np.exp(-t * 2.5)) * np.minimum(1, (seconds - t) / 0.08))[:, None]


def compose_upbeat(seconds, bars=9):
    """`bars` bars in `seconds` (nine unless said): all but the last are groove (I vi IV V in D, two
    bars each) and the last is for the final chord to ring. The first two bars have no drum but the
    hat, so it starts light and lifts."""
    rng = np.random.default_rng(5)
    groove = bars - 1
    bar = seconds / bars
    beat, six = bar / 4, bar / 16
    total = int((seconds + 0.5) * SR)
    drums, bass, arp, pads = (np.zeros((total, 2)) for _ in range(4))
    chords = [(38, [62, 66, 69, 74]), (35, [62, 66, 71, 74]), (31, [59, 62, 67, 71]), (33, [61, 64, 69, 73])]
    the_kick, hat, clap, crash = kick(), noise_hit(rng, 0.05, 90, True), noise_hit(rng, 0.16, 28, False), noise_hit(rng, 2.2, 2.2, True)
    pump = np.ones(total)                                    # the kick pushes everything else down for a moment
    for b in range(groove):
        at = b * bar
        root, voices = chords[(b // 2) % 4]
        if b % 2 == 0:
            add(pads, stab(voices, 2 * bar), at)
        for q in range(4):
            if b >= 2:
                add(drums, 0.95 * the_kick, at + q * beat)
                i = int((at + q * beat) * SR)
                duck = 1 - 0.55 * np.exp(-np.arange(int(0.22 * SR)) / (0.07 * SR))
                pump[i:i + len(duck)] = np.minimum(pump[i:i + len(duck)], duck)
                if q in (1, 3):
                    add(drums, 0.42 * clap, at + q * beat)
            add(drums, 0.20 * hat, at + q * beat + beat / 2)             # the off-beat hat
            if b >= 4:
                add(drums, 0.08 * hat, at + q * beat + six)
                add(drums, 0.08 * hat, at + q * beat + 3 * six)
        if b >= 2:
            for e in range(8):                                           # the bass, in eighths, off the kick
                if e % 2 == 1 or b >= groove - 2:
                    add(bass, (0.9 if e % 2 else 0.55) * synth_bass(root + (12 if e == 7 else 0), beat / 2), at + e * beat / 2)
        up = voices + [voices[0] + 12, voices[1] + 12]
        pattern = [0, 2, 1, 3, 2, 4, 3, 5, 4, 2, 3, 1, 2, 4, 3, 5] if b % 2 == 0 else [5, 3, 4, 2, 3, 1, 2, 0, 1, 3, 2, 4, 3, 5, 4, 2]
        for k, step in enumerate(pattern):
            tone = pluck(up[step] + 12) * (0.9 if k % 4 == 0 else 0.55)
            pan = 0.35 + 0.3 * (k % 2)
            add(arp, np.stack([tone * (1 - pan), tone * pan], axis=1), at + k * six)
            echo = pluck(up[step] + 12) * 0.18                           # one echo, three sixteenths on, on the other side
            add(arp, np.stack([echo * pan, echo * (1 - pan)], axis=1), at + (k + 3) * six)
    # the last bar: the home chord and a crash, left to ring
    end = groove * bar
    add(pads, 1.3 * stab([50, 57, 62, 66, 69, 74], bar + 0.4), end)
    add(bass, synth_bass(38, bar), end)
    add(drums, 0.95 * the_kick, end)
    add(drums, 0.30 * crash, end)
    add(arp, np.repeat((pluck(86, 1.5) * 0.8)[:, None], 2, axis=1), end)
    mix = 0.70 * drums + (0.30 * bass + 0.40 * arp + 0.17 * pads) * pump[:, None]
    mix = mix[: int(seconds * SR)]
    fade = int(0.9 * SR)
    mix[-fade:] *= np.linspace(1, 0, fade)[:, None] ** 2
    mix = np.tanh(1.6 * mix / np.max(np.abs(mix)))           # a soft limit, for some density
    return mix * 0.89 / np.max(np.abs(mix))


def main():
    out = sys.argv[1]
    seconds = float(sys.argv[2]) if len(sys.argv) > 2 else 240.0
    style = sys.argv[3] if len(sys.argv) > 3 else "calm"
    bars = int(sys.argv[4]) if len(sys.argv) > 4 else 9
    pcm = ((compose_upbeat(seconds, bars) if style == "upbeat" else compose(seconds)) * 32767).astype("<i2")
    with wave.open(out, "wb") as f:
        f.setnchannels(2)
        f.setsampwidth(2)
        f.setframerate(SR)
        f.writeframes(pcm.tobytes())


if __name__ == "__main__":
    main()
