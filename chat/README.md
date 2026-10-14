# cedar-chat

A web chat for a cedar server, with two selectable looks. It is a separate Rust service
(axum + SQLite) that serves a single-page UI and sits between the browser and cedar:

```
browser  ──/api──▶  cedar-chat  ──/v1 (Bearer CEDAR_API_KEY)──▶  cedar  ──▶  vLLM + sandbox
```

The cedar API key stays on the server. The browser only talks to cedar-chat.

## Two interfaces

The settings button (sliders icon, top right) switches between:

| | Style | Layout |
| --- | --- | --- |
| `/v1/` | **Claude style** | Conversations in a sidebar, one thread with tool calls inline |
| `/v2/` | **Cedar style** | "The desk": the document the agent writes is the page, with web results and command output as clippings beside it and the conversation in a margin |

Your choice is saved in this browser (`localStorage`, key `cedar-chat-ui`) and decides where `/`
goes. Without a saved choice, `/` opens Cedar style. Switching keeps the open conversation and
your access token. Opening `/v1/` or `/v2/` directly always shows that interface and does not
change the saved choice.

## Features

Both interfaces have all of these, except where noted:

- A conversation list (a sidebar in Claude style, an "Earlier" list in Cedar style) with new chat,
  rename (click the title), and delete. Cedar style deletes only the open conversation
- Replies stream in as the agent works. Text, tool calls (`bash`, `run_python`, the file,
  document, chart, web, and weather tools, and `web_search` when cedar has a Brave key) with their input and
  output, errors, and token usage are shown as they happen
- A stop button cancels the running response
- A files panel (the folder button in the header) lists what the agent saved under `/workspace`
  and downloads any of it; finished `write_file`, `generate_pdf`, `generate_chart`, and
  `download_file` calls get a download button too
- The images the agent produces are shown in the response, once each, after its reply: a chart
  from `generate_chart`, a picture `write_file` or `download_file` saved, or one a `bash` command
  or `run_python` script created (from the files the call lists). A file that was rewritten
  several times is shown as it is when the response ends. Claude style puts them under the reply,
  Cedar style in the margin below its last note. An image the agent refers to in its text as
  `![caption](chart.png)` is shown in place of the reference, and not again below. PNG, JPEG, GIF,
  WebP, BMP, AVIF, and SVG files up to 16 MB are shown; at most 12 per response, loaded as they
  scroll into view. Each has View and Download buttons under it, and a click on the picture opens
  it full size
- In Cedar style, a Markdown or text file the agent writes, or a PDF it makes from Markdown, is shown
  as the page, with Copy and Download
- A View button opens a workspace Markdown file (`.md`, `.markdown`) as rendered Markdown, a PDF in
  the browser's own PDF viewer, and an image at full size, in a dialog. It is in the files panel, on
  finished file-tool calls, and on the page of a generated PDF. Markdown files over 2 MB and images
  over 16 MB are not previewed. A file named `.pdf` that is not a PDF, or `.png` that is not a PNG,
  is refused. Workspace images inside a Markdown file (`![caption](chart.png)`, relative to
  `/workspace` as `generate_pdf` resolves them) are shown; images from the web are not fetched
- A `show_map` tool call (when cedar offers the tool) is drawn as a map: a numbered pin for each
  place on an OpenStreetMap map you can pan and zoom (click it first, so the wheel keeps scrolling
  the conversation), a list of the places with their coordinates under it, and a note for any names
  cedar could not find. Cedar style puts it on the desk as a clipping, Claude style under the tool
  call. The list is also what you see if the tiles can't load. If the result isn't a usable map, only
  the tool call's raw output is shown
- Markdown rendering: headings, lists, tables, quotes, links, and code blocks with a copy button
- Light and dark themes (follows the system), plus a mobile layout
- An optional model picker and an optional access token
- [Tools and settings](#tools-and-settings): the tools, model, turn limit and system prompt every
  conversation gets, a Tools button to change the tools of one conversation, and (for admins)
  cedar's Brave Search key and Google account
- [Attachments](#attachments): with a multimodal model, images and PDFs go with a message, by the
  paperclip button, by dropping them on the message box, or by pasting an image
- [Scheduled agents](#scheduled-agents): a finished conversation can be turned into one prompt that
  runs on its own every day, on weekdays, or weekly at a local time, with every run's result kept
  under Settings

## Running it

```sh
# 1. Start cedar (see the top-level README), e.g.
CEDAR_API_KEYS=devkey cargo run -p cedar

# 2. Start the chat server and open http://127.0.0.1:3000
CEDAR_API_KEY=devkey cargo run -p cedar-chat
```

The UI is plain HTML, CSS, and JavaScript in `static/` (`v1/` and `v2/` for the two interfaces,
`ui-pref.js` for the saved choice, `file-viewer.js` for the file preview, `image-view.js` and
`image-view.css` for the images the agent produces, `map-view.js` and
`map-view.css` for the maps, `schedule.js` and `schedule.css` for scheduled agents, and
`settings.js` and `settings.css` for the tools, model, and integrations settings, which both
interfaces share, `vendor/leaflet/` for the map library, and
`index.html`, which sends `/` to the chosen one). It is
compiled into the binary, so there is no Node build step. Restart the server after you edit those
files.

### Images

Every image the page shows comes through this server's `/api/conversations/{id}/files/{path}`
with the access token, like a download, so nothing is fetched from elsewhere: a `![caption](url)`
that points at the web stays a link. The bytes are shown from a `blob:` URL whose type is taken
from the file's extension, never from the content, so a file named `.png` is decoded as a PNG or
not at all, and an SVG is drawn by an `<img>`, which runs none of its scripts and loads nothing it
references. "Open in new tab" in the viewer is offered for PDFs and raster images only, since a
tab of its own would run an SVG's scripts with the page's origin.

### Maps and privacy

[Leaflet](https://leafletjs.com) 1.9.4 (BSD 2-Clause, license in `static/vendor/leaflet/LICENSE`) is
served from this server and loaded the first time a map is shown, so no third-party code runs in
the page, which holds your access token. The map tiles are the one thing that comes from elsewhere:
the browser fetches them from `tile.openstreetmap.org`, which sees the viewer's IP address and the
area shown. OpenStreetMap's [tile usage policy](https://operations.osmfoundation.org/policies/tiles/)
allows light use like this and asks for the attribution the map displays; a busy deployment should
use its own tile server (the URL is `TILES` in `static/map-view.js`). The chat server itself makes
no requests for maps; cedar looks the places up (see the top-level README).

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `CEDAR_API_KEY` | required | One of cedar's `CEDAR_API_KEYS` |
| `CEDAR_URL` | `http://127.0.0.1:8080` | cedar's base URL |
| `CHAT_BIND_ADDR` | `127.0.0.1:3000` | Where the chat server listens |
| `CHAT_DATABASE_URL` | `sqlite://cedar-chat.db` | Conversation storage |
| `CHAT_ACCESS_TOKEN` | unset | If set, the page asks for this token and the API requires it |
| `CHAT_MODELS` | unset | Comma-separated models for the picker. They must also be allowed by cedar's `ALLOWED_MODELS` |
| `CHAT_ADMIN_TOKEN` | unset | If set, Settings offers [Integrations](#integrations) (cedar's API keys and Google account) to whoever enters it. Must differ from `CHAT_ACCESS_TOKEN` |
| `CHAT_PUBLIC_URL` | `http://<CHAT_BIND_ADDR>` | This server's address as the browser reaches it; "Connect Google" sends Google `<CHAT_PUBLIC_URL>/api/oauth/google/callback` |
| `CHAT_HISTORY_MAX_BYTES` | `40000` | How much earlier conversation is inlined if the previous run is gone from cedar, or compacted for a schedule in that case (at most 60000) |

Without `CHAT_ACCESS_TOKEN`, anyone who can reach the chat server can run the agent with your
cedar key. Keep the default loopback bind, or set a token before listening on a public address.

## How conversations map onto cedar

The first message in a conversation starts a new cedar run. Each later message starts a run that
**continues** the previous one (`parent_id` in cedar's API), so the agent keeps:

- the whole conversation, including its earlier tool calls and their results, and
- its sandbox workspace: files written under `/workspace` are still there, as are packages the
  agent `pip install`ed (they go to `/workspace/.local`). Running processes are not, since each
  run gets a new container.

cedar removes a workspace once it has been idle for `WORKSPACE_TTL_SECS` (24 hours by default).
After that, a follow-up still has the conversation but starts with an empty workspace, and the
model is told so.

If cedar no longer has the previous run (it was deleted, or cedar's database was reset), cedar-chat
falls back to starting a new run with the earlier exchanges inlined in the prompt, up to
`CHAT_HISTORY_MAX_BYTES`. [Attachments](#attachments) live in cedar's run, so earlier ones are gone
with it; only the new message's files go along.

Only one response per conversation runs at a time. The UI replays each turn from cedar's event log,
so reloading the page or opening a conversation shows the full tool trace. Deleting a conversation
also deletes its runs on cedar (best effort), which frees the workspace.

## Scheduled agents

Once a conversation has a finished response, the **Schedule** button (the clock in Claude style,
"Schedule" in Cedar style) turns it into an agent that runs on its own:

1. cedar reviews the whole conversation, tool calls included, and writes one self-contained prompt
   for it (`POST /v1/compact` in cedar's API). If cedar no longer has the run, the exchanges this
   service remembers are compacted instead, up to `CHAT_HISTORY_MAX_BYTES`.
2. You edit the prompt until it stands on its own, name the schedule, and pick a frequency (every
   day, weekdays, or every week on a weekday), a time of day, and the model. Times are local: the
   browser's time zone is sent along (an IANA name like `Europe/Berlin`), so daylight-saving
   changes are handled, and the next run is confirmed in your local time.
3. From then on the chat server starts the prompt as a **fresh** cedar run each time it is due: a
   new conversation, a new workspace, no memory of the earlier runs, and no files (a schedule's
   prompt is text only). It checks every 30 seconds, so a run starts within half a minute of its time.

Every run lives in a conversation of its own, named after the schedule and the time it was due,
with the full tool trace and files panel. These conversations stay out of the conversation list;
**Settings → Scheduled agents** lists each schedule with its next and last run and, under **Runs**,
every run with its status and result, an **Open** button that shows it as a conversation, and the
prompt that was used. There you can also **Pause** and **Resume** a schedule, **Run now**, **Edit**
the prompt and timing, and **Delete** it. Deleting a schedule deletes its runs' conversations and
their cedar runs too.

If the chat server was down when a run was due, the run starts once at the next start, and the
schedule then moves on to its next slot (missed slots do not pile up). If cedar refuses the run
because it is busy (its per-key active-run cap) or can't be reached, the slot is retried every tick
for an hour, after which it is recorded as a failed run and skipped. Resuming a paused schedule
skips the slots missed while it was paused. At most 100 schedules can exist.

## Tools and settings

**Settings → Tools & model** sets the defaults for every conversation: which of cedar's tool
groups the agent gets (web search, reading web pages, weather, maps, the browser, Gmail and
Calendar, the last two down to single tools), and the default model, turn limit, and system
prompt. The system prompt field shows cedar's own default text until you change it, and "Use the
default" puts it back; the default text is saved as "unset", so it keeps following cedar's
`SYSTEM_PROMPT`. The groups come from cedar's `GET /v1/tools`, so a group cedar doesn't offer is shown
greyed out with the reason (no Brave key, `BROWSER=off`, no Google account). Tools that send or
change things (sending mail, editing events) ask before they are switched on. The sandbox tools
(files, shell, Python, documents, charts) are always on.

The **Tools** button next to the message box changes the tools for the open conversation only
(in a new chat, for the conversation the first message creates); a dot on it shows the
conversation has its own choice. A schedule made from a conversation keeps that conversation's
tools.

Every message sends cedar the resolved choice, `*` when everything is on, so changing the defaults
also applies to follow-ups in existing conversations that don't have their own tools.

### Attachments

The same section has a checkbox, **This model accepts images and PDFs**. With it on, a paperclip
button appears by the message box, and files can also be dropped on the box or pasted (a
screenshot, say). PNG, JPEG, GIF, WebP and PDF files are taken, at most 10 per message, 20 MB each
and 32 MB together; the chosen files are shown as chips, with a thumbnail for images, until the
message is sent, and a message still needs some text. The browser sends them as base64 with the
message, and cedar puts them in front of the prompt as [attachments](../README.md#attachments) of
the run, so the agent sees them for the rest of the conversation. A sent message shows what went
with it (name, type and size; the bytes stay in cedar's run). cedar passes the files to the model
as they are and does not check that it can read them: with a text-only model the response fails
with the model server's error, so leave the box off for one. A message with files is refused while
the box is off.

### Integrations

Settings → **Model server & API keys** is where the model server address and the API keys go.
Without `CHAT_ADMIN_TOKEN` it only explains how to turn it on and which tool groups lack a key.
With it, the section is unlocked with that token for the browser session and manages cedar's model
server (address, port, path and optional API key; checked by listing the server's models, which
are then shown), its Brave Search key (checked with one search before it is saved, then shown
as its last four characters) and its Google account ("Connect Google" goes through Google's
consent page and back to `CHAT_PUBLIC_URL/api/oauth/google/callback`). They are stored by cedar,
encrypted, and take effect on the next message without a restart. This needs cedar's
`CEDAR_SECRET_KEY`, and this server's `CEDAR_API_KEY` in cedar's `CEDAR_ADMIN_KEYS`; see
[Integrations](../README.md#integrations). Keys set in cedar's environment are shown as "Managed by
environment" and can't be changed here.

## API

All routes are under `/api`. When `CHAT_ACCESS_TOKEN` is set, they need
`Authorization: Bearer <token>` (except `/api/config`). Errors look like cedar's:
`{"error": {"code": ..., "message": ...}}`.

| Method and path | Purpose |
| --- | --- |
| `GET /api/config` | `{auth_required, admin_enabled, models}` |
| `GET /api/conversations` | Conversations, most recently active first. Conversations a schedule created are left out |
| `POST /api/conversations` | Create one. Body `{}` or `{"title": "..."}` |
| `GET /api/conversations/{id}` | `{conversation, turns}`. Unfinished turns are refreshed from cedar |
| `PATCH /api/conversations/{id}` | Rename: `{"title": "..."}` |
| `DELETE /api/conversations/{id}` | Delete. Returns 409 while a response is running |
| `POST /api/conversations/{id}/messages` | Send `{"content": "...", "model"?: "...", "attachments"?: [{"name", "media_type", "data"}]}` (`data` is base64; see [Attachments](#attachments) for the types and limits, and the body may be up to 48 MiB). Starts a cedar run with the [settings](#tools-and-settings) and returns the turn, whose `attachments` lists `{name, media_type, size}` for each file. Returns 400 for files while the multimodal setting is off, 409 if a response is still running |
| `PUT /api/conversations/{id}/tools` | `{"tools": [...]}` gives the conversation its own tools; `{"tools": null}` returns it to the defaults |
| `GET /api/conversations/{id}/files` | The workspace files of the latest turn's run, as cedar reports them (`{workspace_id, files, truncated}`); empty for a conversation without turns |
| `GET /api/conversations/{id}/files/{path}` | Download one file, relayed from cedar with its type, length, and file name |
| `POST /api/conversations/{id}/compact` | Have cedar write one prompt for the conversation so far: `{prompt, source}` where `source` is `run` or, when cedar has lost the run, `history`. Returns 409 while a response is running, 404 without a finished response |
| `GET /api/turns/{id}/events?after=N` | The turn's cedar event stream (SSE), relayed |
| `POST /api/turns/{id}/cancel` | Stop the turn's run |
| `GET /api/schedules` | Every schedule, enabled ones first, with its `last_run` |
| `POST /api/schedules` | Create one: `{"title", "prompt", "frequency": "daily"\|"weekdays"\|"weekly", "weekday"?: 0–6 (Monday–Sunday, weekly only), "time": "HH:MM", "timezone": "<IANA name>", "model"?, "conversation_id"?, "tools"?}`. Without `tools` the schedule takes the conversation's own tools, if it has any, else it follows the defaults |
| `GET /api/schedules/{id}` | `{schedule, runs}`, newest run first, each with its `turn` (refreshed while unfinished) or an `error` if the run could not be started |
| `PATCH /api/schedules/{id}` | Change any of the fields above, or `{"enabled": false}` to pause. A changed timing (or resuming) recomputes the next run |
| `DELETE /api/schedules/{id}` | Delete the schedule with its runs' conversations and cedar runs. Returns 409 while one of its runs is going |
| `POST /api/schedules/{id}/run` | Start a run now, without moving the next scheduled one. Returns 409 while its latest run is still going |
| `GET /api/settings` | The defaults for new runs: `{tools, model, system, max_turns, multimodal}` (`null` means cedar's own; `multimodal` says whether messages may carry [attachments](#attachments)), plus `defaults`: cedar's own model, system prompt and turn limit (`null` if cedar can't be reached) |
| `PUT /api/settings` | Replace them. Tool names are checked against cedar's `/v1/tools`; an absent `multimodal` means `false` |
| `GET /api/tools` | cedar's tool groups (`GET /v1/tools`), relayed |
| `GET /api/integrations` | Admin (`X-Admin-Token`): cedar's integrations, plus the Google `redirect_uri` to register |
| `PUT` / `DELETE /api/integrations/llm` | Admin: set cedar's model server and its default model name (`{"base_url", "api_key"?, "model"?}`) or go back to its defaults |
| `PUT` / `DELETE /api/integrations/brave` | Admin: save (`{"api_key"}`) or remove cedar's Brave Search key |
| `POST /api/integrations/google/connect` | Admin: `{url}` of Google's consent page |
| `DELETE /api/integrations/google` | Admin: disconnect the Google account |
| `GET /api/oauth/google/callback` | Where Google sends the browser back. Needs no token: the single-use `state` cedar issued for the admin's "Connect Google" is the check. Redirects to `/?integrations=connected` or `/?integrations=error&message=…` |

## Tests

```sh
cargo test -p cedar-chat
```

The integration tests in `tests/api.rs` run the chat API against a fake cedar server.
