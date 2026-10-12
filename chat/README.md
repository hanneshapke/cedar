# cedar-chat

A Claude.ai-style web chat for a cedar server. It is a separate Rust service (axum + SQLite) that
serves a single-page UI and sits between the browser and cedar:

```
browser  ──/api──▶  cedar-chat  ──/v1 (Bearer CEDAR_API_KEY)──▶  cedar  ──▶  vLLM + sandbox
```

The cedar API key stays on the server. The browser only talks to cedar-chat.

## Features

- A conversation list in the sidebar, with new chat, rename (click the title), and delete
- Replies stream in as the agent works. Text, tool calls (`bash`, `read_file`, `write_file`) with
  their input and output, errors, and token usage are shown as they happen
- A stop button cancels the running response
- Markdown rendering: headings, lists, tables, quotes, links, and code blocks with a copy button
- Light and dark themes (follows the system), plus a mobile layout
- An optional model picker and an optional access token

## Running it

```sh
# 1. Start cedar (see the top-level README), e.g.
CEDAR_API_KEYS=devkey cargo run -p cedar

# 2. Start the chat server and open http://127.0.0.1:3000
CEDAR_API_KEY=devkey cargo run -p cedar-chat
```

The UI is plain HTML, CSS, and JavaScript in `static/`. It is compiled into the binary, so there
is no Node build step. Restart the server after you edit those files.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `CEDAR_API_KEY` | required | One of cedar's `CEDAR_API_KEYS` |
| `CEDAR_URL` | `http://127.0.0.1:8080` | cedar's base URL |
| `CHAT_BIND_ADDR` | `127.0.0.1:3000` | Where the chat server listens |
| `CHAT_DATABASE_URL` | `sqlite://cedar-chat.db` | Conversation storage |
| `CHAT_ACCESS_TOKEN` | unset | If set, the page asks for this token and the API requires it |
| `CHAT_MODELS` | unset | Comma-separated models for the picker. They must also be allowed by cedar's `ALLOWED_MODELS` |
| `CHAT_HISTORY_MAX_BYTES` | `40000` | How much earlier conversation is inlined if the previous run is gone from cedar (at most 60000) |

Without `CHAT_ACCESS_TOKEN`, anyone who can reach the chat server can run the agent with your
cedar key. Keep the default loopback bind, or set a token before listening on a public address.

## How conversations map onto cedar

The first message in a conversation starts a new cedar run. Each later message starts a run that
**continues** the previous one (`parent_id` in cedar's API), so the agent keeps:

- the whole conversation, including its earlier tool calls and their results, and
- its sandbox workspace: files written under `/workspace` are still there. Running processes
  are not, since each run gets a new container.

cedar removes a workspace once it has been idle for `WORKSPACE_TTL_SECS` (24 hours by default).
After that, a follow-up still has the conversation but starts with an empty workspace, and the
model is told so.

If cedar no longer has the previous run (it was deleted, or cedar's database was reset), cedar-chat
falls back to starting a new run with the earlier exchanges inlined in the prompt, up to
`CHAT_HISTORY_MAX_BYTES`.

Only one response per conversation runs at a time. The UI replays each turn from cedar's event log,
so reloading the page or opening a conversation shows the full tool trace. Deleting a conversation
also deletes its runs on cedar (best effort), which frees the workspace.

## API

All routes are under `/api`. When `CHAT_ACCESS_TOKEN` is set, they need
`Authorization: Bearer <token>` (except `/api/config`). Errors look like cedar's:
`{"error": {"code": ..., "message": ...}}`.

| Method and path | Purpose |
| --- | --- |
| `GET /api/config` | `{auth_required, models}` |
| `GET /api/conversations` | Conversations, most recently active first |
| `POST /api/conversations` | Create one. Body `{}` or `{"title": "..."}` |
| `GET /api/conversations/{id}` | `{conversation, turns}`. Unfinished turns are refreshed from cedar |
| `PATCH /api/conversations/{id}` | Rename: `{"title": "..."}` |
| `DELETE /api/conversations/{id}` | Delete. Returns 409 while a response is running |
| `POST /api/conversations/{id}/messages` | Send `{"content": "...", "model"?: "..."}`. Starts a cedar run and returns the turn. Returns 409 if a response is still running |
| `GET /api/turns/{id}/events?after=N` | The turn's cedar event stream (SSE), relayed |
| `POST /api/turns/{id}/cancel` | Stop the turn's run |

## Tests

```sh
cargo test -p cedar-chat
```

The integration tests in `tests/api.rs` run the chat API against a fake cedar server.
