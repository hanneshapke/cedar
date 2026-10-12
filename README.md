# cedar

A backend service in Rust for running Claude agents. Clients submit a task over HTTP, cedar runs the agent loop in the background, and clients follow progress live (Server-Sent Events), cancel, or read the result later. Every run's tools execute inside their own Docker container.

## How it works

1. `POST /v1/runs` stores a `queued` run and returns `202`.
2. When a slot is free (`MAX_CONCURRENT_RUNS`), the run starts: cedar creates a sandbox container for it and runs the agent loop.
3. The loop sends the conversation, system prompt, and tool definitions to the Anthropic Messages API. The reply is scanned for content blocks:
   - `text` is collected as the answer.
   - `tool_use` is executed in the run's sandbox and the result is sent back as a `tool_result`. Errors are returned to the model with `is_error: true` so it can recover.
   - `server_tool_use` (web search) is run by Anthropic. It is only reported as an event.
4. If the stop reason is `tool_use` (or `pause_turn` for a paused server-tool loop), go back to step 3. Otherwise the run is `completed` with the text of the last turn.
5. The sandbox is destroyed and the run ends in exactly one terminal state: `completed`, `failed`, or `cancelled`.

Runs, the full conversation, and an append-only event log are stored in SQLite. A run gives up with `failed` after `max_turns` round trips (capped by `MAX_TURNS_CAP`) or `RUN_TIMEOUT_SECS` of wall-clock time.

## Quick start

You need a recent Rust toolchain, Docker, and an Anthropic API key.

```sh
# 1. Build the image the agent's tools run in
docker build -t cedar-sandbox sandbox/

# 2. Start the server (it listens on 127.0.0.1:8080 by default)
export ANTHROPIC_API_KEY=sk-ant-...
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
| `GET /v1/runs/{id}/messages` | The stored conversation, with content blocks exactly as exchanged with the API. |
| `GET /v1/runs/{id}/events` | Server-Sent Events. Replays from the beginning, then follows live, and closes after the run's last event. |
| `POST /v1/runs/{id}/cancel` | Ask a queued or running run to stop. Returns `202`; the run reaches `cancelled` shortly after. `409` if it already finished. |
| `DELETE /v1/runs/{id}` | Delete a finished run with its messages and events. `409` while it is still active. |
| `GET /healthz`, `GET /readyz` | Liveness, and readiness (database + Docker). No auth. |

A run belongs to the key that created it. Anyone else gets `404`.

### Events

Each SSE message has `id:` (a per-run sequence number), `event:` (the type), and `data:` (JSON `{seq, type, data, created_at}`). To resume after a dropped connection, reconnect with the `Last-Event-ID` header (browsers' `EventSource` does this automatically) or `?after=<seq>`.

| Type | `data` |
| --- | --- |
| `run.started` | `model`, `max_turns` |
| `assistant.text` | `turn`, `text` |
| `tool.call` | `id`, `name`, `input` |
| `tool.result` | `id`, `output`, `is_error`, `duration_ms` |
| `web_search` | `turn`, `name`, `input` |
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
- The model and system prompt come from the parent unless the request sets them.
- A run can be continued once, and only after it has finished, so a chain of runs is a straight
  line and never shares a workspace with a run that is still going. To keep going, continue from
  the latest run.
- Runs report `parent_id`, `workspace_id`, and `workspace_reused`.

Workspaces are kept for `WORKSPACE_TTL_SECS` (default 86400) after their last run ends, then
removed. A continuation of a run whose workspace has expired still gets the conversation, but
starts with an empty workspace, and the model is told so. Deleting the last run that uses a
workspace removes it straight away.

## Web chat

`chat/` holds **cedar-chat**, a separate Rust service with a Claude.ai-style web UI for talking to
the agent: a conversation sidebar, live tool calls, and stop and rename buttons. It calls cedar's
HTTP API with a key it keeps on the server.

```sh
CEDAR_API_KEY=devkey cargo run -p cedar-chat   # then open http://127.0.0.1:3000
```

Each follow-up message continues the previous run, so the agent keeps its conversation and its
workspace files. See [chat/README.md](chat/README.md) for configuration.

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
| `ANTHROPIC_API_KEY` | required |
| `CEDAR_API_KEYS` | required, comma-separated bearer tokens |
| `BIND_ADDR` | `127.0.0.1:8080` |
| `DATABASE_URL` | `sqlite://cedar.db` |
| `ANTHROPIC_MODEL` | `claude-sonnet-5-5` |
| `ALLOWED_MODELS` | empty (any model may be requested) |
| `MAX_TURNS_CAP` | 25 |
| `MAX_TOKENS` | 4096 |
| `WEB_SEARCH_MAX_USES` | 5 per request (0 turns web search off) |
| `MAX_CONCURRENT_RUNS` | 4 |
| `MAX_ACTIVE_RUNS_PER_KEY` | 2 |
| `RUN_TIMEOUT_SECS` / `TOOL_TIMEOUT_SECS` | 600 / 60 |
| `TOOL_OUTPUT_MAX_BYTES` | 32768 per stream |
| `SANDBOX_IMAGE` / `SANDBOX_NETWORK` | `cedar-sandbox:latest` / `none` |
| `SANDBOX_MEMORY` / `SANDBOX_CPUS` | `512m` / `1` |

## Tools

| Tool | What it does |
| --- | --- |
| `bash` | Runs a command with `sh -c` in the run's container and returns stdout, stderr, and the exit status if it was non-zero. Killed after `TOOL_TIMEOUT_SECS`. |
| `read_file` | Reads a text file from the container. |
| `write_file` | Writes content to a path in the container, creating parent directories and overwriting the file. |
| `web_search` | Anthropic's server-side web search. It is executed by Anthropic, needs no local code, and works even though the container has no network. |

Output is cut at `TOOL_OUTPUT_MAX_BYTES` per stream with a `[truncated N bytes]` marker, so a large file or noisy command can't fill the context window.

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

The `definition()` method builds the JSON the API expects, so you do not write it by hand. A tool that touches files or runs processes must go through `ctx.sandbox`, never the host. Return `Err` for a failure the model can recover from; it is sent back with `is_error: true`.

## Safety

The agent runs arbitrary commands, so isolation is the point of the design:

- Each run gets its own container, created at the start and removed at the end, mounting a workspace volume that is kept for [continuations](#continuing-a-run) until `WORKSPACE_TTL_SECS` expires. Containers run as a non-root user with all capabilities dropped, `no-new-privileges`, and memory, CPU, and process limits. Networking is off (`SANDBOX_NETWORK=none`) unless you change it.
- Every file and command operation goes through `docker exec`. The host filesystem is never touched, so a symlink the agent creates can't point out of its workspace.
- The Anthropic API key is never passed into a container.
- API keys are compared in constant time, and runs are scoped to the key that created them. Infrastructure error details (Docker paths and so on) go to the server log, not to API clients.
- cedar speaks plain HTTP. Put it behind a TLS-terminating proxy before exposing it. `/healthz` and `/readyz` are unauthenticated.
- The user running cedar needs access to the Docker daemon, which is effectively root on that machine. Run cedar on a host or VM you are willing to dedicate to it.
- Turning networking on for the sandbox lets the agent reach anything that container network can reach, including internal services.

## Operations

- **Restarts:** SQLite file plus in-process queue means one instance only. Runs that were queued or running when the process stopped cannot be resumed, so at startup they are marked `failed` and leftover containers are removed. Workspace volumes that still belong to a run in the database are kept until `WORKSPACE_TTL_SECS` expires; any other workspace volume is removed.
- **Shutdown:** on SIGTERM or Ctrl-C cedar stops accepting connections, cancels active runs (they end as `cancelled` with reason `server shutting down`), and waits up to 10 seconds for them to record their final state.
- **API errors:** requests to Anthropic time out and retry on 429, 5xx, and connection errors (up to 4 attempts, exponential backoff, honoring `retry-after`). Other 4xx errors fail the run immediately with the API's message.
- **Logs:** `RUST_LOG` controls verbosity (default `info`). Prompts and keys are not logged.

## Development

```sh
cargo test                        # unit + integration tests; no Docker or network needed
cargo clippy --all-targets -- -D warnings
cargo test --test docker -- --ignored   # sandbox tests; need Docker and the cedar-sandbox image
```

The integration tests drive the real router with a scripted LLM and a fake sandbox (`tests/common/mod.rs`), and the LLM client against a local mock of the Messages API.

## Limitations

- No token-level streaming from the model; events arrive per model turn and per tool call.
- A run is a single prompt. To follow up, start a new run that [continues](#continuing-a-run) the finished one.
- There is no API to download workspace files, and workspaces are removed after `WORKSPACE_TTL_SECS`. Have the agent include what you need in its answer.
- One instance: SQLite and the run queue live in the process.
- Tool calls within a turn run one after another.
- Per-key limiting is only the active-run cap; there is no request rate limit or token budget.

## License

Apache 2.0. See [LICENSE](LICENSE).
