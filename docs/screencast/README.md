# Cedar: an introduction (screencast)

A four-minute walk through Cedar's chat: the first start, adding a model, a task with a plan, the
work in the sandbox, an approval, memory, the tool switches, and an agent made from a conversation.

| File | What it is |
| --- | --- |
| `cedar-intro.mp4` | The film: 1920×1080, 30 fps, H.264, 3:54, no sound |
| `cedar-intro-narrated.mp4` | The same film with the English voice, background music and a subtitle track |
| `script.md` | The script: each scene with its times, what is on screen, and the narration |
| `narration-elevenlabs.txt` | The narration as one text for a text-to-speech voice |
| `narration/NN-<scene>.txt` | The same, one file a scene, for one clip each |
| `captions.srt` | The captions, at most two lines of 42 characters each, timed to the voice clips |
| `cedar-intro-narrated.<lang>.mp4` | The film in German (`de`), French (`fr`) and Spanish (`es`), recorded in that language, with the voice, background music and a subtitle track |
| `narration-elevenlabs.<lang>.txt`, `narration/<lang>/`, `captions.<lang>.srt`, `timeline.<lang>.json` | Their narration, captions and scene times |
| `timeline.json` | When each scene of the take starts and ends, in seconds |
| `loop/` | The 18-second cut: a silent loop for a web page, the same with upbeat music, and a vertical reel, each in four languages |
| `youtube.md` | A title and a description, with chapters, for uploading each film to YouTube |
| `tools/` | Everything that makes the above |

## What is real and what is scripted

Cedar, its chat UI and the sandbox are the real programs, built from this branch. The Python step
runs on the attached CSV, the chart and the PDF are made by Cedar's own tools, the approval card
is Cedar's, and the memory, the record set and the agent are stored as shown.

The language model is not real. `tools/mock_llm.py` stands in for vLLM: it answers
`/v1/models` and `/v1/chat/completions` and takes each reply from `tools/scenes.py`, so the take
is the same every time and needs no GPU. The plan's steps, Cedar's sentences, the code it "writes"
and the report's text are that script. The figures in them are the ones the Python step prints for
`tools/household-spending-2026.csv` (and its translations), which is invented data from
`tools/make_csv.py`.

## The voice

The narration is 458 words, about three minutes of speech. Each scene of the film lasts at least
as long as its narration at 150 words a minute plus a second of air, so the simple way to keep
voice and picture together is one clip a scene:

1. Make a clip from each `narration/NN-<scene>.txt` with the same voice and settings, saved
   under the same name (`01-title.mp3`, `02-first-run.mp3`, ...). `tools/voice.sh` does that
   through the ElevenLabs API, with your API key and the ID of the voice:

   ```sh
   ELEVENLABS_API_KEY=... docs/screencast/tools/voice.sh                   # English, default voice, into audio/
   ELEVENLABS_API_KEY=... docs/screencast/tools/voice.sh <voice-id>        # English, another voice
   ELEVENLABS_API_KEY=... docs/screencast/tools/voice.sh <voice-id> de     # into audio-de/
   ```

   English uses the voice `itMkRQx56fOF4CJe1D0C` when none is given; the other languages need
   a voice ID.

   It keeps the clips that are already there, so it can be run again after a failure; delete a
   clip to have it made anew. By hand, paste each text into ElevenLabs' Text to Speech page and
   download the MP3 under that name.
2. `tools/mux.sh <folder with the clips>` starts each clip 0.4 s into its scene and writes
   `cedar-intro-narrated.mp4`. The second argument names another output; never the silent
   `cedar-intro.mp4` itself, which is what it reads.

The texts spell three things the way they should be said: "V L L M", "llama C P P" and "okay".
The wording as written is in `script.md`. If a clip runs longer than its scene (the times are in
`script.md`), slow the film instead of the voice: lower `WPS` (words a second) in `tools/record.js` and
record again.

## The music

`tools/mux.sh` puts background music under the voice unless told otherwise. The piece is made by
`tools/music.py` from sine waves (it needs numpy), for the length of the film: a slow pad through
eight chords in D major, a soft bass and a sparse bell-like figure, with a long reverb. It is the
project's own, so there is nothing to license, and it is the same every time. Most of its energy
is below 600 Hz, under the range a voice needs. In the mix it is about 12 dB below the voice when
alone, and a compressor keyed by the voice turns it down further while the voice speaks.

```sh
docs/screencast/tools/mux.sh docs/screencast/audio                       # with the music
MUSIC=off docs/screencast/tools/mux.sh docs/screencast/audio             # voice only
MUSIC=my-track.mp3 docs/screencast/tools/mux.sh docs/screencast/audio    # your own track, cut and faded to length
MUSIC_DB=-14 docs/screencast/tools/mux.sh docs/screencast/audio          # quieter (the default is -10)
```

## The short loop

`tools/loop.py` cuts 18 seconds from each take: the plan appears, Cedar works, the chart, the
PDF. The four parts are sped up to fit and cross-faded. It writes three films per language into
`loop/` (`.de`, `.fr`, `.es` before the extension for the other languages):

| File | What it is for |
| --- | --- |
| `loop.mp4`, `loop.webm` | 1280×720, no sound, about 0.7 and 1 MB. Made to repeat: it fades from and to the page's background. For a web page, as `<video autoplay muted loop playsinline>` with the WebM first and the MP4 as the fallback |
| `hero.mp4`, `hero.webm` | 1000×800, no sound, repeating, zoomed in on the plan and then the results so it reads in a small spot. `loop.py` copies it, with a still picture, to `docs/landing/shots/hero-<lang>.*`, where the landing page's hero plays it in the page's language |
| `loop-music.mp4` | 1920×1080 with music and a closing card, 20 seconds |
| `reel.mp4` | 1080×1920 for a reel or a short, 22 seconds: an opening card with the mark and the tagline for two seconds, then each part cropped to what matters (the plan, then the results) with a line of text over it, the music (a bar longer, for the opening) and the closing card. Text and film keep clear of the top and bottom strips the apps cover |

The music is the "upbeat" piece of `tools/music.py`: nine bars to the film's length, eight of
groove and one for the last chord to ring under the closing card. Like the calm piece it is
generated here, so it is free to use.

```sh
docs/screencast/tools/loop.py            # every language whose silent take is in docs/screencast
docs/screencast/tools/loop.py de         # one
docs/screencast/tools/loop.py --reel     # only the reels
```

It needs the silent takes (`cedar-intro.mp4`, and `cedar-intro.<lang>.mp4`, which are not kept in
the repository: see below), numpy, and Node with Playwright for the text cards.

## Subtitles

Each narrated film carries its captions as a subtitle track in the film's language, which the
viewer switches on or off in the player (VLC, QuickTime, most apps and televisions). Nothing is
drawn into the picture. Web players that ignore such tracks, GitHub's among them, show none;
for those, and for YouTube or Vimeo, upload the matching `captions.srt` or `captions.<lang>.srt`.

`tools/make_script.py` writes the captions from the narration: one per sentence, long sentences cut
near their middle, each for its share of the time its scene's voice clip takes (or of the
estimate, when the clips are not there). `tools/mux.sh` puts them in; `SUBS=off` leaves them out.
To have them in the picture instead, burn them in from the silent film:

```sh
ffmpeg -i docs/screencast/cedar-intro-narrated.mp4 -vf subtitles=docs/screencast/captions.srt -c:a copy burned.mp4
```

## German, French and Spanish

Each of the three has its own film, with the voice already in it:
`cedar-intro-narrated.de.mp4`, `cedar-intro-narrated.fr.mp4` and `cedar-intro-narrated.es.mp4`.
They are separate takes, not the English one with another voice: the chat is switched to the
language on the first screen (Deutsch (Deutschland), Français (France), Español (Latinoamérica)),
the file, the requests, Cedar's answers, the chart and the PDF are in it, and each scene lasts as
long as its voice clip. `timeline.<lang>.json` and `captions.<lang>.srt` belong to that take.

The translations follow `docs/i18n-glossary.md` (German *du*, French *vous*, Latin American
Spanish *tú*). The narration is in `tools/narration.<lang>.json`, one text a scene, and everything
on screen that is written in a language (the demo file, what is typed, what the scripted model
answers, the title cards) is in `tools/content.py`.

To make one again, say the German one:

```sh
ELEVENLABS_API_KEY=... docs/screencast/tools/voice.sh <voice-id> de           # the clips, into audio-de/
docs/screencast/tools/take.sh /tmp/cedar-take-de de docs/screencast/audio-de  # the take, timed to the clips
cp /tmp/cedar-take-de/cedar-intro.mp4 docs/screencast/cedar-intro.de.mp4
cp /tmp/cedar-take-de/rec/timeline.json docs/screencast/timeline.de.json
docs/screencast/tools/make_script.py                                          # captions.de.srt for that take
docs/screencast/tools/mux.sh docs/screencast/audio-de de                      # cedar-intro-narrated.de.mp4
```

The silent takes `cedar-intro.<lang>.mp4` and the clips are not kept in the repository, only the
narrated films. Without the clips, `take.sh <dir> <lang>` times the scenes by the length of the
text, and `mux.sh` moves a clip that outlasts its scene behind the one before it.

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
`tools/content.py` (the words) or `tools/scenes.py` (the steps); what the pointer does is in `tools/record.js`, one block a scene.
