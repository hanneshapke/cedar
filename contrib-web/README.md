# The contribution site

A standalone page where anyone can give a conversation they had with **ChatGPT** or **Claude** to
Cedar's open corpus: the same corpus [cedar-chat](../chat/README.md#contributing-a-conversation)
sends to, received by the same ingest ([`contrib/`](../contrib/README.md)), under the same design
([docs/design/contributions.md](../docs/design/contributions.md)).

The person downloads their data export (ChatGPT: *Settings → Data controls → Export data*;
Claude: *Settings → Privacy → Export data*) and drops the .zip on the page. **The export never
leaves the browser.** The page reads it, lists the conversations, and builds a bundle from the one
the person picks with this crate's engine, compiled to WebAssembly. The engine uses the shared
schema crate's redaction scan and consistency checks, the same ones the ingest runs. The only
thing that is sent is that bundle, when the person presses *Send* after reviewing it.

## What the page does

1. **Load.** A .zip export (read with the browser's own `DecompressionStream`, ZIP64 included) or
   a `conversations.json`. Both formats are recognised by shape, and an export can be large: only
   the selected conversation is handed to the engine.
2. **Pick.** Titles, dates and prompt counts, searchable. Titles are never sent.
3. **Review.** The conversation exactly as it will be sent, with every stand-in marked, and a
   *Raw JSON* tab with the document itself (download included). The person can:
   - replace phrases of their own (names, employers, places), by typing them or by selecting text,
     with a replacement or a neutral `redacted-…`;
   - send only the first *n* exchanges;
   - include the model's reasoning (o-series thoughts, Claude's thinking) and pages the model
     fetched, both off by default;
   - rate answers, report the outcome (`succeeded` / `partly` / `failed`) with a note;
   - pick the license (CC0 or CC BY 4.0) and an optional random pseudonym, kept in the browser;
   - for Claude, whose export doesn't name the model, say which model answered.
   *Send* stays off until the bundle passes every check and the person ticks the consent box.
4. **Receipts.** Each contribution's receipt and withdraw token are kept in `localStorage`, with
   *Refresh status*, *Withdraw*, and download and import of the receipts file.

## How an export becomes a bundle

| In the export | In the bundle |
| --- | --- |
| The branch the person last looked at (ChatGPT's `current_node`; Claude's newest message's parent chain) | The conversation. Edited prompts and regenerated answers on other branches are left out |
| Each prompt and what followed it | One turn, whose messages are the conversation up to the end of that exchange (as a cedar run continuing a conversation starts from its parent's copy) |
| Prompts with no answer | Left out |
| Assistant text | `text` blocks |
| Tool calls (ChatGPT's `recipient`, Claude's `tool_use`) and their results | `tool_use` / `tool_result` pairs, with a minimal definition per tool (the export has none). A call without a result gets one saying so |
| Web browsing and search results | Stubs (`[FETCHED BODY LEFT OUT: n bytes, sha256 …]`) unless the person keeps them |
| Results of searching the person's uploaded files | Always stubs |
| Uploaded files and images | `image` / `document` blocks with the type and size only; no name, no bytes |
| Reasoning | On the model calls, only if chosen |
| ChatGPT custom instructions and memories, the `bio` tool, Claude's memory and past-chat tools | Cut; `dropped.memory` says so |
| Connectors to mail, calendar, drives, Slack, Notion, GitHub and the like | The conversation is blocked: other people's data |
| Titles, ids, times | Never sent; each turn keeps its day |
| The model | `model.id` `chatgpt-export` or `claude-export`; `model.name` always `openai/…` or `anthropic/…` |

Then the scan replaces keys, tokens, email addresses, phone, card and bank numbers, public IP
addresses and home-directory user names with stand-ins of the same shape. A user name found in a
path is replaced wherever else it occurs too. The stand-ins come from a seed that is random per
review and stays fixed while it is open.

## Model outputs and the ingest's model list

OpenAI's and Anthropic's terms restrict using their models' outputs to develop other models. The
ingest publishes only the models in its `CONTRIB_MODELS` (the Terraform default is
`deepseek-ai/*`). The page reads `GET /v1/policy` and, for a model the corpus doesn't take, shows
why and keeps *Send* off. The ingest refuses such a bundle anyway (`model_not_allowed`). Accepting
`openai/*` or `anthropic/*` is a decision for whoever runs the corpus, made with those terms in
mind, not something this page decides.

## Building and hosting

```sh
rustup target add wasm32-unknown-unknown
contrib-web/build.sh                     # → contrib-web/dist
CONTRIB_INGEST_URL=https://contrib.example.org contrib-web/build.sh /srv/www/contribute
```

`dist/` is static files (about 1.4 MB, most of it the engine) for any static host. The landing page
workflow (`.github/workflows/pages.yml`) publishes it at `/contribute/` on the project's site. The
page sends to the ingest named in `config.js` (the project's own by default). That ingest must
list the page's origin in `CONTRIB_CORS_ORIGINS` (`cors_origins` in `contrib/terraform`), or the
browser blocks the request.

The page loads nothing from elsewhere but Google Fonts, and its Content-Security-Policy allows
scripts from its own origin only (plus `wasm-unsafe-eval` for the engine).

### Locally

```sh
contrib-web/build.sh
CONTRIB_HMAC_KEY=$(openssl rand -hex 24) CONTRIB_CORS_ORIGINS=http://127.0.0.1:8099 cargo run -p cedar-contrib &
(cd contrib-web/dist && python3 -m http.server 8099 --bind 127.0.0.1)
# open http://127.0.0.1:8099/?ingest=http://127.0.0.1:8090
```

`?ingest=` is honoured for `localhost` and `127.0.0.1` addresses only, so a link can't point
someone's contribution at another server.

## The engine

`src/` is plain Rust, tested natively (`cargo test -p cedar-contrib-web`). `contrib/tests/api.rs`
also posts a bundle it builds to the ingest and checks that it is accepted. The WebAssembly
interface has no bindings generator and no imports: `cw_alloc`, `cw_free` and `cw_build`, with JSON
strings in and out (see `src/lib.rs`).

What it does not do, as with cedar-chat's export: find names, employers or places with no fixed
shape (that's what the person's own phrases are for), or check an image's or file's content
(they're never sent).
