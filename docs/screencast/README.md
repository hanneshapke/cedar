# Cedar: an introduction (screencast)

A four-minute walk through Cedar's chat: the first start, adding a model, a task with a plan, the
work in the sandbox, an approval, memory, the tool switches, and an agent made from a conversation.

| File | What it is |
| --- | --- |
| `cedar-intro.mp4` | The film: 1920×1080, 30 fps, H.264, 3:54, no sound |
| `script.md` | The script: each scene with its times, what is on screen, and the narration |
| `narration-elevenlabs.txt` | The narration as one text for a text-to-speech voice |
| `narration/NN-<scene>.txt` | The same, one file a scene, for one clip each |
| `captions.srt` | Captions, timed by word count within each scene |
| `timeline.json` | When each scene of the take starts and ends, in seconds |
| `tools/` | Everything that makes the above |

## What is real and what is scripted

Cedar, its chat UI and the sandbox are the real programs, built from this branch. The Python step
runs on the attached CSV, the chart and the PDF are made by Cedar's own tools, the approval card
is Cedar's, and the memory, the record set and the agent are stored as shown.

The language model is not real. `tools/mock_llm.py` stands in for vLLM: it answers
`/v1/models` and `/v1/chat/completions` and takes each reply from `tools/scenes.py`, so the take
is the same every time and needs no GPU. The plan's steps, Cedar's sentences, the code it "writes"
and the report's text are that script. The figures in them are the ones the Python step prints for
`tools/household-spending-2026.csv`, which is invented data from `tools/make_csv.py`.

## The voice

The narration is 458 words, about three minutes of speech. Each scene of the film lasts at least
as long as its narration at 150 words a minute plus a second of air, so the simple way to keep
voice and picture together is one clip a scene:

1. In ElevenLabs, generate a clip from each `narration/NN-<scene>.txt` with the same voice and
   settings, and save it under the same name (`01-title.mp3`, `02-first-run.mp3`, ...).
2. `tools/mux.sh <folder with the clips>` starts each clip 0.4 s into its scene and writes
   `cedar-intro-narrated.mp4`.

The texts spell three things the way they should be said: "V L L M", "llama C P P" and "okay".
The wording as written is in `script.md`. If a clip runs longer than its scene (the times are in
`script.md`), slow the film instead of the voice: lower `WPS` (words a second) in `tools/record.js` and
record again.

## Recording it again

It needs what Cedar needs (a Rust toolchain and Docker) plus Node with Playwright and a Chromium,
Python 3 and ffmpeg.

```sh
cargo build --release -p cedar -p cedar-chat
docker build -t cedar-sandbox sandbox/
docs/screencast/tools/take.sh /tmp/cedar-take       # about five minutes
cp /tmp/cedar-take/cedar-intro.mp4 /tmp/cedar-take/rec/timeline.json docs/screencast/
docs/screencast/tools/make_script.py                # script.md, the narration texts, captions.srt
```

`take.sh` stops any running `cedar`, `cedar-chat` and containers named `cedar*`, then starts the
scripted model on port 8000, cedar on 8080 and the chat on 3000 with new databases in the take's
folder (`start.sh`), drives the chat in a headless Chromium (`record.js`), and encodes the frames
(`encode.py`). `record.js` draws its own pointer, because a headless browser has none, and
captures about 16 frames a second, each with its time, so the film runs in real time.

To change what is said, edit `tools/narration.json`; to change what Cedar "answers", edit
`tools/scenes.py`; what the pointer does is in `tools/record.js`, one block a scene.
