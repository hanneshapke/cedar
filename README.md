# cedar

A backend service in Rust for running LLM agents against any OpenAI-compatible server (built for DeepSeek V4 on a local vLLM). Clients submit a task over HTTP, cedar runs the agent loop in the background, and clients follow progress live (Server-Sent Events), cancel, or read the result later. Every run's tools execute inside their own Docker container.

## How it works

1. `POST /v1/runs` stores a `queued` run and returns `202`.
2. When a slot is free (`MAX_CONCURRENT_RUNS`), the run starts: cedar creates a sandbox container for it and runs the agent loop.
3. The loop sends the conversation, system prompt, and tool definitions to the LLM server's chat-completions API. The reply is scanned for content blocks:
   - `text` is collected as the answer.
   - `tool_use` (a tool call) is executed in the run's sandbox and the result is sent back as a `tool_result`. Errors are returned to the model with `is_error: true` so it can recover.
4. If the stop reason is `tool_use`, go back to step 3. Otherwise the run is `completed` with the text of the last turn.
5. The sandbox is destroyed and the run ends in exactly one terminal state: `completed`, `failed`, or `cancelled`.

Runs, the full conversation, and an append-only event log are stored in SQLite. A run gives up with `failed` after `max_turns` round trips (capped by `MAX_TURNS_CAP`) or `RUN_TIMEOUT_SECS` of wall-clock time.

## Quick start

You need a recent Rust toolchain, Docker, and an OpenAI-compatible LLM server. cedar defaults to a local [vLLM](https://docs.vllm.ai) serving DeepSeek V4:

```sh
# 0. Start vLLM (vLLM 0.20 or newer; see the vLLM recipe for flags that suit your GPUs:
#    https://recipes.vllm.ai/deepseek-ai/DeepSeek-V4-Flash)
vllm serve deepseek-ai/DeepSeek-V4-Flash --trust-remote-code --tokenizer-mode deepseek_v4 \
  --tool-call-parser deepseek_v4 --enable-auto-tool-choice --reasoning-parser deepseek_v4
```

The tool-call flags are required: without them the model's tool calls come back as plain text and the agent can't act. Check the registered model name with `curl localhost:8000/v1/models` and set `LLM_MODEL` if it differs from the default.

```sh
# 1. Build the image the agent's tools run in
docker build -t cedar-sandbox sandbox/

# 2. Start the server (it listens on 127.0.0.1:8080 by default)
export CEDAR_API_KEYS=$(openssl rand -hex 24)
cargo run --release

# 3. In another shell: start a run, then follow it
KEY=<the key you generated>
curl -s -H "Authorization: Bearer $KEY" -H 'content-type: application/json' \
     -d '{"prompt": "write hello.py that prints the first 10 primes, run it, and show the output"}' \
     localhost:8080/v1/runs
curl -N -H "Authorization: Bearer $KEY" localhost:8080/v1/runs/<id>/events
```

`GET /readyz` tells you whether the database and Docker (including the sandbox image) are usable. See `.env.example` for every setting.

## API

All `/v1` routes need `Authorization: Bearer <key>`. Errors are `{"error": {"code": "...", "message": "..."}}`.

| Route | What it does |
| --- | --- |
| `POST /v1/runs` | Start a run. Body: `{"prompt", "model"?, "system"?, "max_turns"?}`. Returns `202` with the run. `429` if the key already has `MAX_ACTIVE_RUNS_PER_KEY` queued or running. |
| `GET /v1/runs` | Your runs, newest first. Query: `status`, `limit` (max 100), `before` (a run id; use `next_before` from the previous page). |
| `GET /v1/runs/{id}` | Status, `turns`, `final_text`, `stop_reason`, `error`, token `usage`, timestamps. |
| `GET /v1/runs/{id}/messages` | The stored conversation, as content blocks (`text`, `tool_use`, `tool_result`). |
| `GET /v1/runs/{id}/events` | Server-Sent Events. Replays from the beginning, then follows live, and closes after the run's last event. |
| `POST /v1/runs/{id}/cancel` | Ask a queued or running run to stop. Returns `202`; the run reaches `cancelled` shortly after. `409` if it already finished. |
| `GET /v1/runs/{id}/files` | The regular files in the run's workspace: `{workspace_id, files: [{path, size, modified_at}], truncated}`. Sorted by path, at most 1000 entries; `.git`, `.local`, `.cache`, `.venv`, `venv`, `node_modules` and `__pycache__` are skipped. `404` once the workspace has expired. |
| `GET /v1/runs/{id}/files/{path}` | Download one file as an attachment (`Content-Type` from the extension, `Content-Disposition` with the file name). `path` is relative to `/workspace`; `400` if it contains `..`, `404` if there is no such file or the workspace has expired. Files over 32 MiB are refused. |
| `DELETE /v1/runs/{id}` | Delete a finished run with its messages and events. `409` while it is still active. |
| `POST /v1/compact` | Turn a conversation into one self-contained prompt (see [below](#compacting-a-run-into-a-prompt)). Body: `{"run_id"}` or `{"transcript"}`, plus `"model"?`. Returns `{prompt, model, usage}`. |
| `GET /healthz`, `GET /readyz` | Liveness, and readiness (database + Docker). No auth. |

A run belongs to the key that created it. Anyone else gets `404`.

### Compacting a run into a prompt

A client that wants to repeat what a conversation achieved (the chat UI's [scheduled
agents](chat/README.md#scheduled-agents), say) needs one prompt rather than the back-and-forth that
led there. `POST /v1/compact` with `{"run_id": "<a finished run of yours>"}` makes a single model
call over the run's stored conversation, with no tools and no sandbox, asking the model to write a
prompt a fresh agent could carry out unattended: the concrete inputs, the constraints, the output
format, the files to produce, with the dead ends and dropped requests left out. The answer is
returned as `prompt` and not stored. `409` while the run is still going. Tool results are cut to
2,000 characters each and the oldest messages dropped to keep the call under about 60 KB, so a long
run still compacts cheaply; a conversation that ended in the middle of a tool call gets error
results for the open calls first, as a [continuation](#continuing-a-run) would. `{"transcript":
"<text>"}` (at most 100 KB) compacts a plain-text conversation instead, for a run cedar no longer
has. `"model"` works as for a run; without it the run's model (or the default) is used. If the
model returns no text the response is `502 bad_gateway`.

### Events

Each SSE message has `id:` (a per-run sequence number), `event:` (the type), and `data:` (JSON `{seq, type, data, created_at}`). To resume after a dropped connection, reconnect with the `Last-Event-ID` header (browsers' `EventSource` does this automatically) or `?after=<seq>`.

| Type | `data` |
| --- | --- |
| `run.started` | `model`, `max_turns` |
| `assistant.text` | `turn`, `text` |
| `tool.call` | `id`, `name`, `input` |
| `tool.result` | `id`, `output`, `is_error`, `duration_ms` |
| `usage` | `turn`, `input_tokens`, `output_tokens` |
| `run.completed` | `final_text`, `stop_reason` |
| `run.failed` | `error` |
| `run.cancelled` | `reason` |

`run.completed`, `run.failed`, and `run.cancelled` are terminal: the stream closes after one of them.

## Continuing a run

`POST /v1/runs` accepts `"parent_id": "<run id>"` to continue a finished run instead of starting a
new conversation:

```sh
curl -s localhost:8080/v1/runs -H "Authorization: Bearer devkey" \
  -H "Content-Type: application/json" \
  -d '{"prompt": "now add tests for it", "parent_id": "0190..."}'
```

- The new run starts with a copy of the parent's messages, including its tool calls, and appends
  the new prompt. If the parent stopped in the middle of a tool call, that call gets an error
  result so the conversation stays valid.
- It mounts the parent's workspace, so files under `/workspace` are kept. Each run still gets a
  new container, so running processes are not.
- The model and system prompt come from the parent unless the request sets them. A run created before you switched `LLM_MODEL` keeps its old model name, so pass an explicit `"model"` when continuing it.
- A run can be continued once, and only after it has finished, so a chain of runs is a straight
  line and never shares a workspace with a run that is still going. To keep going, continue from
  the latest run.
- Runs report `parent_id`, `workspace_id`, and `workspace_reused`.

Workspaces are kept for `WORKSPACE_TTL_SECS` (default 86400) after their last run ends, then
removed. A continuation of a run whose workspace has expired still gets the conversation, but
starts with an empty workspace, and the model is told so. Deleting the last run that uses a
workspace removes it straight away.

## Workspace files

Anything the agent saves under `/workspace` can be fetched while the workspace exists, whether the
run is still going or has finished:

```sh
curl -s -H "Authorization: Bearer $KEY" localhost:8080/v1/runs/<id>/files
curl -OJ -H "Authorization: Bearer $KEY" localhost:8080/v1/runs/<id>/files/out/report.pdf
```

Every run in a chain of continuations shares one workspace, so any of their ids works. The files
are read through a short-lived container that mounts the workspace read-only, with the same
restrictions as a sandbox; nothing is read from the host filesystem, and a symlink that points
outside `/workspace` is reported as missing.

## Web chat

`chat/` holds **cedar-chat**, a separate Rust service with a web UI for talking to the agent, in a
Claude-style or a Cedar-style ("desk") layout that you pick in Settings: your conversations, live
tool calls, a files panel for downloading what the agent produced, and stop and rename buttons. It calls cedar's HTTP API with a key it keeps on the server.

```sh
CEDAR_API_KEY=devkey cargo run -p cedar-chat   # then open http://127.0.0.1:3000
```

Each follow-up message continues the previous run, so the agent keeps its conversation and its
workspace files. See [chat/README.md](chat/README.md) for configuration.

To run vLLM, cedar, and the chat UI together on an NVIDIA DGX Spark, with systemd services and access from your laptop, see [DEPLOYMENT.md](DEPLOYMENT.md).

![A cedar-chat conversation: the agent writes a test file, runs pytest in its sandbox, and summarizes the result in a table](docs/screenshots/conversation-light.png)

| Starting a conversation | A response in progress |
| --- | --- |
| ![The empty chat with a greeting, the conversation sidebar, and the message box with a model picker](docs/screenshots/welcome.png) | ![A running response: a finished write_file call, a bash call still running, and the stop button](docs/screenshots/streaming.png) |

| Dark theme, with a tool call expanded | Mobile layout |
| --- | --- |
| ![The dark theme, with the write_file tool call expanded to show its input and output](docs/screenshots/conversation-dark.png) | <img src="docs/screenshots/mobile.png" alt="The mobile layout showing a code block with a copy button and token usage" width="260"> |

## Configuration

Settings come from environment variables. The important ones:

| Setting | Default |
| --- | --- |
| `CEDAR_API_KEYS` | required, comma-separated bearer tokens |
| `LLM_BASE_URL` | `http://localhost:8000/v1` |
| `LLM_API_KEY` | empty (only needed if the server runs with `--api-key`) |
| `BIND_ADDR` | `127.0.0.1:8080` |
| `DATABASE_URL` | `sqlite://cedar.db` |
| `LLM_MODEL` | `deepseek-ai/DeepSeek-V4-Flash` (must match the name the server registered) |
| `ALLOWED_MODELS` | empty (any model may be requested) |
| `MAX_TURNS_CAP` | 25 |
| `MAX_TOKENS` | 4096 |
| `MAX_CONCURRENT_RUNS` | 4 |
| `MAX_ACTIVE_RUNS_PER_KEY` | 2 |
| `RUN_TIMEOUT_SECS` / `TOOL_TIMEOUT_SECS` | 600 / 60 |
| `TOOL_OUTPUT_MAX_BYTES` | 32768 per stream |
| `BRAVE_API_KEY` | empty (set it to enable the `web_search` tool) |
| `BRAVE_API_BASE_URL` | `https://api.search.brave.com/res/v1/web/search` |
| `WEB_FETCH` | `on` (set `off` to remove the `fetch_url` and `download_file` tools) |
| `WEB_FETCH_ALLOW_PRIVATE` | `off` (set `on` to let those tools reach loopback and private addresses) |
| `WEATHER` | `on` (set `off` to remove the `weather_forecast` tool) |
| `WEATHER_BASE_URL` / `WEATHER_GEOCODING_BASE_URL` | `https://api.open-meteo.com/v1/forecast` / `https://geocoding-api.open-meteo.com/v1/search` (point both at a self-hosted Open-Meteo to avoid the public API; `show_map` uses the geocoding URL too) |
| `MAPS` | `on` (set `off` to remove the `show_map` tool) |
| `SANDBOX_IMAGE` / `SANDBOX_NETWORK` | `cedar-sandbox:latest` / `none` (set `bridge` to let the agent `pip install` packages) |
| `SANDBOX_MEMORY` / `SANDBOX_CPUS` | `512m` / `1` |
| `BROWSER` | `off` (set `on` to enable the `browser` tool; see [The browser](#the-browser)) |
| `BROWSER_IMAGE` / `BROWSER_NETWORK` | `cedar-browser:latest` / `bridge` (the browser needs internet access, so `none` is refused) |
| `BROWSER_MEMORY` / `BROWSER_CPUS` | `1g` / `1` |

## Tools

| Tool | What it does |
| --- | --- |
| `bash` | Runs a command with `sh -c` in the run's container and returns stdout, stderr, and the exit status if it was non-zero. Killed after `TOOL_TIMEOUT_SECS`. |
| `read_file` | Reads a text file from the container. |
| `write_file` | Writes content to a path in the container, creating parent directories and overwriting the file. |
| `run_python` | Runs a Python 3 script (`code`, optional `args`) in the container with `/workspace` as working directory and on the import path. Returns stdout, stderr, the exit status if non-zero, and the workspace files the script created or modified. Unlike `bash`, the code needs no shell quoting. |
| `read_document` | Extracts the text of a PDF, Word (`.docx`), Excel (`.xlsx`), PowerPoint (`.pptx`), CSV/TSV, or HTML file in the workspace, with tables as Markdown. Takes `path`, optional `pages` (PDF pages or slides, like `2-5` or `1,4-`), `sheet`, and `rows` (spreadsheet or CSV data rows; the header is always shown). |
| `generate_pdf` | Renders Markdown (default) or HTML to a PDF file in the workspace. Takes `path` (must end in `.pdf`), `content`, optional `format` (`markdown` or `html`) and optional `title`. Returns the page count and size, plus the renderer's warnings (a missing image, say). |
| `generate_chart` | Draws a chart with matplotlib and saves it as PNG, SVG, or PDF. Takes `path`, `kind` (`line`, `bar`, `barh`, `area`, `scatter`, `pie`, `histogram`), `labels` (x categories or values), `series` (`[{name?, values}]`, `null` for gaps), and optional `title`, `x_label`, `y_label`, `stacked`, `bins`, `log_scale`. |
| `web_search` | Searches the web with [Brave Search](https://brave.com/search/api/) and returns numbered results (title, URL, age, snippet). Takes `query`, optional `count` (1–20, default 5) and optional `freshness` (`pd`, `pw`, `pm`, `py`). Only offered when `BRAVE_API_KEY` is set. |
| `fetch_url` | Fetches an http(s) URL and returns its text: HTML converted to plain text (title, headings, lists, links as numbered references; scripts and styles dropped), text, JSON, XML, and CSV as-is. Long pages are cut at `TOOL_OUTPUT_MAX_BYTES` with a note giving the `start` offset to continue from. Binary content is refused with a pointer to `download_file`. |
| `download_file` | Downloads an http(s) URL (up to 25 MB) to `path` in the workspace, so the agent can work with PDFs, spreadsheets, or datasets from the web while the container stays offline. |
| `weather_forecast` | Current conditions and a daily forecast from [Open-Meteo](https://open-meteo.com) (open source, open data, no API key). Takes a `location` (a place name or postal code, geocoded to its best match) or `latitude` and `longitude`, optional `days` (1–16, default 3) and optional `units` (`metric` or `imperial`). Returns the resolved place, current temperature, conditions, humidity, wind, and precipitation, then per day the conditions, low and high, precipitation and its chance, wind, UV index, and sunrise and sunset. Unless `WEATHER=off`. |
| `show_map` | Shows the user a map in the chat, with a numbered pin for each place. Takes `markers` (1–25 objects, each with a `location` such as `Berlin` or `Paris, France`, or `latitude` and `longitude`, and an optional `label`) and an optional `title`. Names are looked up with Open-Meteo's geocoding API, which knows cities, towns, and regions but not street addresses or businesses. Returns JSON with the coordinates used; names that could not be found are listed under `not_found`, and a call where none are found is an error. The chat interfaces draw the JSON as a map ([chat/README.md](chat/README.md)); other clients just get the JSON. Unless `MAPS=off`. |
| `browser` | Drives a headless Chromium for pages that need JavaScript or interaction. Takes `action` (`navigate` with `url`, `snapshot`, `click` or `type` with `ref`, `scroll`, `back`) and returns the page as text with interactive elements numbered like `[12] button "Show all reviews"`. Only offered when `BROWSER=on`; see [The browser](#the-browser). |

`web_search`, `fetch_url`, `download_file`, `weather_forecast`, and `show_map` run in the cedar process, not in the container, because the container has no network. `web_search` returns snippets only; `fetch_url` reads a page. A rate-limited request (Brave's free plan allows one per second) is retried once. Get a key from the [Brave Search API dashboard](https://api-dashboard.search.brave.com/).

`weather_forecast` talks only to the two Open-Meteo endpoints in the settings above, never to a URL the model picks, so it needs no address checks. A rate-limited request is retried once. Open-Meteo's public API is free for non-commercial use only; for commercial use, run your own instance or buy their commercial plan and set `WEATHER_BASE_URL` and `WEATHER_GEOCODING_BASE_URL` to it. The forecast ends with Open-Meteo's CC BY 4.0 attribution line so the model can pass it on.

`show_map` draws nothing itself: cedar only turns place names into coordinates, with the same geocoder and the same non-commercial terms as `weather_forecast`, and returns them as JSON. The chat front end draws the map in the browser, which loads the map tiles from OpenStreetMap (see [chat/README.md](chat/README.md)).

`fetch_url` and `download_file` are on unless `WEB_FETCH=off`. Since their requests come from the cedar host, they only reach public addresses: hostnames are resolved by cedar and loopback, private, link-local, CGNAT, and other internal addresses are dropped (so DNS tricks don't get through), IP literals are checked the same way, every redirect is re-checked (at most 5), and proxy environment variables are ignored because a proxy would resolve names itself. `WEB_FETCH_ALLOW_PRIVATE=on` lifts that rule for tests or a trusted intranet.

Every run's system prompt (the default or a custom one) gets a short note appended saying whether the sandbox has network access and whether `web_search`, `fetch_url`, `download_file`, `weather_forecast`, `show_map`, and `browser` are available, so the model searches with the tool instead of trying `curl` against a search engine. Without `BRAVE_API_KEY`, cedar logs a warning at startup.

`generate_pdf` runs in the sandbox like the file tools: the document is staged in the container's `/tmp`, rendered with `cedar-render-pdf` (a script in the sandbox image, built on [WeasyPrint](https://weasyprint.org)), and the staging file removed. Markdown gets a page template with A4 pages, margins, page numbers, and styles for headings, tables, code blocks, and footnotes; HTML that is already a full document (`<html>`) is rendered as-is with its own styles. Relative image URLs resolve against `/workspace`, so a chart the agent saved earlier can be embedded. The tool is always offered, so a custom `SANDBOX_IMAGE` must ship `cedar-render-pdf` or the model gets an error telling it so.

`generate_chart` and `read_document` work the same way, through `cedar-render-chart` and `cedar-read-document` in the sandbox image (`sandbox/render_chart.py` and `sandbox/read_document.py`). `generate_chart` checks the spec in cedar first (kinds, matching label and value counts, at most 20 series and 100,000 points), so most mistakes come back before anything runs. `read_document` picks the format from the file's content where it can (PDF and the Office zip formats), otherwise from the extension; legacy `.doc`/`.xls`/`.ppt` files and scanned PDFs without a text layer are reported rather than guessed at. `run_python` stages the script in `/tmp`, runs it, and lists files newer than a marker it touches just before, skipping the same cache and dependency directories as the workspace listing.

Output is cut at `TOOL_OUTPUT_MAX_BYTES` per stream with a `[truncated N bytes]` marker, so a large file or noisy command can't fill the context window.

### The sandbox image

`sandbox/Dockerfile` builds a Debian image with `python3`, `pip`, `venv`, `git`, `curl`, `jq`, `cedar-render-pdf` (WeasyPrint and python-markdown from PyPI, with the pango and DejaVu font packages they need), `cedar-render-chart` (matplotlib), and `cedar-read-document` (pypdf, python-docx, openpyxl, python-pptx). Those libraries are importable from `run_python` too. pip installs go to `/workspace/.local` (via `PYTHONUSERBASE`), so packages the agent installs in one run are still there in a [continuation](#continuing-a-run), and scripts they install are on `PATH`. Installing needs network access: with the default `SANDBOX_NETWORK=none` the container is offline and `pip install` fails to reach PyPI, so set `SANDBOX_NETWORK=bridge` if you want the agent to add packages itself (see [Safety](#safety)). Anything the agent should have offline goes into the Dockerfile.

### The browser

`fetch_url` reads the HTML a server sends, which is not enough for sites that build their pages with JavaScript or hide content behind buttons and dialogs (a listing's "Show all reviews", say). With `BROWSER=on`, the `browser` tool gives the model a real headless Chromium instead:

- **One browser per run, started on first use.** The first `browser` call starts a container from `BROWSER_IMAGE` for the run; later calls talk to the same browser, so the page, open dialogs, and history carry over. The container is removed when the run ends (also on failure or cancellation), and leftovers from a crash are removed at startup. Runs that never browse start nothing.
- **Pages as text.** Every call answers with the page's URL, title, HTTP status (when it isn't a success), and text: headings as Markdown, list items, and each visible interactive element numbered, like `[12] button "Show all 12 reviews"` or `[3] link "Next page" -> /search?page=2`. The model passes that number as `ref` to `click` or `type`. The numbers are reassigned on every call. When a modal dialog is open, only the dialog is described. `scroll` scrolls the open dialog (or the page) and says when it has reached the bottom, after giving the page time to load more. Long pages are cut at `TOOL_OUTPUT_MAX_BYTES`, continued with `{"action": "snapshot", "start": N}`, and runs of look-alike numbered buttons (a calendar's days) are written as one line.
- **Limits.** Only the six actions are passed to the browser; the model can't run scripts of its own in it. Images, media, and fonts are not loaded, downloads are refused, and links that would open a new tab open in the same one. A browser that hangs (no answer within 90 s) or dies (out of memory, say) is removed, the model is told to navigate again, and its next call starts a new one.

The URL in a `navigate` call is checked like `fetch_url`'s: http(s) only, and it must resolve to public addresses unless `WEB_FETCH_ALLOW_PRIVATE=on`. Pages the browser reaches on its own (links, redirects, a page's scripts) are not checked, so it can reach whatever its Docker network reaches; see [Safety](#safety).

`browser/Dockerfile` builds the image: Debian with Playwright and Chromium's headless shell, plus the driver (`browser/driver.py`, installed as `cedar-browser`) that keeps the browser running and answers one call at a time over a Unix socket. Build it with `docker build -t cedar-browser browser/`; `/readyz` reports whether it is present when the browser is on. You can drive it by hand to see what the model sees:

```sh
docker run -d --name cb --shm-size 256m cedar-browser
docker exec cb cedar-browser call '{"action":"navigate","url":"https://example.com"}'
docker rm -f cb
```

## Adding a tool

A tool is a name, a description, a JSON schema for its input, and an async function. Implement the `Tool` trait in `src/tools/` and add it to `ToolSet::standard`.

```rust
struct Now;

#[async_trait]
impl Tool for Now {
    fn name(&self) -> &str { "now" }
    fn description(&self) -> &str { "Return the current Unix time in seconds." }
    fn schema(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    async fn call(&self, _ctx: &ToolContext, _input: &Value) -> Result<String> {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        Ok(secs.to_string())
    }
}

// in ToolSet::standard:
Self::new(vec![Box::new(Bash), Box::new(ReadFile), Box::new(WriteFile), Box::new(Now)])
```

The `definition()` method builds the tool definition for the LLM client, so you do not write it by hand. A tool that touches files or runs processes must go through `ctx.sandbox`, never the host; if it needs a program the sandbox doesn't have, add it to `sandbox/Dockerfile` the way `generate_pdf` adds its renderer. A tool that needs credentials or the network (like `web_search`) runs host-side instead; register it in `ToolSet::from_config`, conditioned on its setting. Return `Err` for a failure the model can recover from; it is sent back with `is_error: true`.

## Safety

The agent runs arbitrary commands, so isolation is the point of the design:

- Each run gets its own container, created at the start and removed at the end, mounting a workspace volume that is kept for [continuations](#continuing-a-run) until `WORKSPACE_TTL_SECS` expires. Containers run as a non-root user with all capabilities dropped, `no-new-privileges`, and memory, CPU, and process limits. Networking is off (`SANDBOX_NETWORK=none`) unless you change it.
- Every file and command operation goes through `docker exec`. The host filesystem is never touched, so a symlink the agent creates can't point out of its workspace.
- `LLM_API_KEY` and `BRAVE_API_KEY` are never passed into a container. With `web_search` enabled, the model's search queries leave your machine for Brave.
- With `weather_forecast` enabled, the place names and coordinates the model asks about leave your machine for Open-Meteo. With `show_map` enabled, so do the place names it looks up. Set `WEATHER=off` and `MAPS=off` if that is not acceptable.
- `fetch_url` and `download_file` make requests from the cedar host to URLs the model chooses. They refuse non-public addresses (see [Tools](#tools)), but they do let the agent send data to any public site in a URL, and fetched pages can carry prompt injections. Set `WEB_FETCH=off` if that is not acceptable.
- API keys are compared in constant time, and runs are scoped to the key that created them. Infrastructure error details (Docker paths and so on) go to the server log, not to API clients.
- cedar speaks plain HTTP. Put it behind a TLS-terminating proxy before exposing it. `/healthz` and `/readyz` are unauthenticated.
- The user running cedar needs access to the Docker daemon, which is effectively root on that machine. Run cedar on a host or VM you are willing to dedicate to it.
- Turning networking on for the sandbox (`SANDBOX_NETWORK=bridge`, needed for `pip install`) lets the agent reach anything that container network can reach, including internal services.
- The browser (`BROWSER=on`) needs internet access. cedar checks the URLs the model navigates to, but not where the browser goes from there: a link, a redirect, or a page's own script can take it anywhere its Docker network (`BROWSER_NETWORK`) reaches, including this host and your internal network. Give it a network that only reaches the internet before turning it on outside a trusted setup. Each run's browser starts with a fresh profile, so it has no cookies or sign-ins, and pages it reads can carry prompt injections like any fetched page.
- Downloads are served from the workspace volume only, through a read-only container; the host filesystem stays out of reach. A downloaded file is whatever the agent produced, so treat it like any other untrusted download.

## Operations

- **Restarts:** SQLite file plus in-process queue means one instance only. Runs that were queued or running when the process stopped cannot be resumed, so at startup they are marked `failed` and leftover containers are removed. Workspace volumes that still belong to a run in the database are kept until `WORKSPACE_TTL_SECS` expires; any other workspace volume is removed.
- **Shutdown:** on SIGTERM or Ctrl-C cedar stops accepting connections, cancels active runs (they end as `cancelled` with reason `server shutting down`), and waits up to 10 seconds for them to record their final state.
- **API errors:** requests to the LLM server time out and retry on 429, 5xx, and connection errors (up to 4 attempts, exponential backoff, honoring `retry-after`). Other 4xx errors fail the run immediately with the API's message.
- **Logs:** `RUST_LOG` controls verbosity (default `info`). Prompts and keys are not logged.

## Development

```sh
cargo test                        # unit + integration tests; no Docker or network needed
cargo clippy --all-targets -- -D warnings
cargo test --test docker -- --ignored   # sandbox tests; need Docker and the cedar-sandbox image
cargo test --test docker_browser -- --ignored   # browser tests; need Docker and the cedar-browser image
```

The integration tests drive the real router with a scripted LLM, a fake sandbox, and a fake browser (`tests/common/mod.rs`), and the LLM client against a local mock of a chat-completions server.

## Limitations

- No token-level streaming from the model; events arrive per model turn and per tool call.
- A run is a single prompt. To follow up, start a new run that [continues](#continuing-a-run) the finished one.
- cedar has no scheduler of its own: a run starts when a client asks for it. Recurring runs are a [cedar-chat](chat/README.md#scheduled-agents) feature.
- Workspace files can be listed and downloaded one at a time, up to 32 MiB each, only while the workspace exists (`WORKSPACE_TTL_SECS` after the last run). There is no upload.
- One instance: SQLite and the run queue live in the process.
- Tool calls within a turn run one after another.
- Per-key limiting is only the active-run cap; there is no request rate limit or token budget.

## License

Apache 2.0. See [LICENSE](LICENSE).
