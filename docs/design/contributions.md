# Design: contributing conversations to a training corpus (opt-in)

_Status: proposal. Nothing here is implemented._

cedar's privacy policy is one sentence: **cedar does not collect any data. Period.** This design
adds a way for a person to *give* a conversation to a shared corpus, used to train an open-weight
agent model with reinforcement learning, without weakening that sentence. The test for every
decision below: with the feature present but not used, cedar behaves exactly as it does today, and
nothing leaves the machine that the person did not read first.

## 1. Goals and non-goals

Goals

- A person can contribute one conversation at a time, on purpose, after seeing exactly what will be
  sent.
- The contribution is useful for RL: the full trajectory (system prompt, tool definitions, every
  model turn, every tool call and result), the outcome, and a signal of how good the result was.
- No account, no identity. A contributor is at most a random pseudonym they can rotate or drop.
- A contribution can be withdrawn.
- The corpus can be published under an open license, so training an open-weight model on it is
  unambiguous.

Non-goals

- Automatic or background collection of any kind. There is no "share everything" switch.
- Telemetry riding along with a contribution (versions, device, timings, crash data).
- Differential privacy or formal anonymity guarantees. Free text is re-identifying by nature; the
  design relies on redaction plus human review, and says so.

## 2. What a thread contains (and what must never go)

A chat thread is a conversation in cedar-chat whose turns are a chain of cedar runs
(`parent_id`). Per run, cedar stores:

| Data | Where | Value for training | Default handling |
| --- | --- | --- | --- |
| User prompts | `runs.prompt`, `messages` | essential | sent, after redaction and review |
| System prompt + cedar's note | `runs.system`, trace | essential (the model was conditioned on it) | sent; the **memory list** inside it is stripped |
| Tool definitions | trace `tools` | essential | sent (they are cedar's, not the user's) |
| Assistant text, tool calls | `messages` | essential | sent |
| Reasoning (`reasoning_content`) | `llm_calls.response` | valuable for distillation, risky (quotes prompts verbatim, may mention memory) | sent only if the person ticks it |
| Tool results: sandbox (`bash`, `run_python`, file tools) | `messages` | essential for tool-use RL | sent, after secret scan |
| Tool results: `web_search` snippets | `messages` | useful | sent |
| Tool results: `fetch_url`, `browser`, `download_file` bodies | `messages` | useful but third-party copyrighted text | replaced by `{url, sha256, bytes}` stubs; the person can keep them |
| Tool results: `gmail_*`, `calendar_*` | `messages` | other people's data without their consent | **thread is not contributable** |
| Tool results: community tools with `health_data` | `messages` | health data | **thread is not contributable** |
| Attachments (images, PDFs) | `run_attachments` | the bytes are the person's documents | never sent; replaced by `{media_type, bytes, sha256}`, name reduced to its extension |
| Memories | `memories` + system note | the most personal thing cedar stores | never sent |
| Workspace files | Docker volume | the produced artifacts are good outcome evidence | file list (paths, sizes) sent; contents only for files the agent wrote in that thread, after the same scan |
| Events: plan, `plan.updated`, approvals, cancel | `events` | preference signal (see §6) | sent, with timestamps coarsened |
| Usage, cost | `events` | useful for cost-aware RL | tokens sent; `cost_usd` dropped (reveals the provider's price list, not the user's data, but adds nothing) |
| Timestamps | everywhere | low | coarsened to the day; time zone dropped |
| Locale | run | needed (language of the data) | language only (`de-CH` → `de`) |
| Model id, model name | run | needed (licensing, §8) | sent |
| API key, owner, run ids, conversation ids | `runs.owner`, ids | identifying | never sent; ids replaced by per-contribution sequence numbers |
| Scheduled agents' conversations | `schedules` | often unattended email digests | not contributable (their tools are usually `gmail`); a person can copy a run into a normal conversation to share it |

The rule for anything not in the table: it is not sent.

## 3. Consent model: three layers, all off

1. **Operator.** Whoever runs cedar-chat sets `CHAT_CONTRIBUTIONS_URL` to an ingest server. Unset
   (the default), the feature does not exist: no settings page, no menu entry, no code path that
   builds a bundle. This mirrors `CHAT_UPDATE_CHECK` and `TOOLS_REGISTRY_URL`: outbound traffic is
   something the operator turns on, and is named in PRIVACY.md.
2. **Person, once.** Settings → Contribute shows, in plain language: what a contribution contains
   (the table above, condensed), where it goes, the license it is released under, that it can be
   withdrawn, and that redaction is best effort and reviewing is their job. They pick:
   - a **license** for their contributions (CC0 or CC BY 4.0; see §8),
   - whether to use a **pseudonym** (a random id generated locally, shown to them, rotatable, or
     "none": each contribution stands alone and cannot be linked to the others),
   - whether **reasoning** may be included,
   - whether **fetched page bodies** may be included.
   Saving this page does not send anything. It only makes the per-thread action appear.
3. **Thread, every time.** A *Contribute this conversation* entry in the conversation's More menu.
   It opens the review dialog (§5). Nothing is sent until the person clicks *Send* in that dialog.
   There is no multi-select and no "share all": each thread is read before it goes.

No nudges: no banner after a good answer, no "help us improve" toast, no pre-ticked boxes, no
re-asking after a decline. The phone apps pair with a cedar-chat server and talk only to it; they
get the same per-thread action, and the server does the sending, so the phone never contacts the
ingest server.

## 4. Redaction: local, layered, visible

All redaction runs in cedar-chat, on the person's own server, before any network call. Three
passes, each adding marks that the next can see:

1. **Structural** (deterministic, always on): everything in §2's "never sent" rows; the memory
   note cut out of the system prompt; attachment bytes and names; ids; timestamps; `owner`.
2. **Pattern scan** (deterministic, always on): API keys and tokens (prefixes such as `sk-`,
   `ghp_`, `xox`, AWS `AKIA`, JWTs, high-entropy strings in `KEY=`/`Authorization:` positions),
   email addresses, phone numbers, IBANs and card numbers, IP addresses, URLs with query tokens,
   home directory paths (`/Users/<name>`, `/home/<name>`, `C:\Users\<name>`), and the server's own
   hostnames. Each becomes a typed placeholder: `[EMAIL_1]`, `[KEY_1]`, `[PATH_1]`, consistent
   within the thread so the trajectory still makes sense. Anything the scan flags as a **secret**
   blocks sending until the person confirms the replacement or edits it; secrets are not something
   to ship as `[KEY_1]` and hope.
3. **Model pass** (on by default, skippable): one call to the person's own registered model, with no
   tools and no sandbox, asking for a list of spans that name a person, an employer, an address, a
   project name or anything else that identifies the author, plus a one-line reason each. The
   output is a proposal, applied as placeholders the person can accept or reject one by one. This
   reuses `POST /v1/compact`'s shape (one model call over a stored conversation) and goes to the
   same LLM server the conversation already went to, so it widens nothing.

What the passes do **not** promise: catching an identifying fact stated in the user's own words
("my startup, the one that makes oat-milk kegs in Portland"). The dialog says this.

## 5. The review dialog: what you see is what is sent

Same principle as Settings → Memory: the page shows exactly the bytes that leave.

- The thread rendered turn by turn, in the normal chat look, with every placeholder highlighted and
  hoverable (shows the type and the original text, locally only). Click a placeholder to restore the
  text, select any text to redact it. Tool results are folded; stubbed bodies say why.
- A sidebar with: what was removed structurally (counts: "2 attachments, the memory list, 3 ids"),
  the pseudonym or "unlinked", the license, the model, whether reasoning is included.
- A **feedback step** before Send (§6): did the agent do what you asked? What was wrong?
- **Send** posts the JSON shown under *Raw* (a tab with the exact document, copy and download). The
  response is a receipt (§7) stored locally and shown.
- Threads that §2 marks not contributable show why, and the action is disabled.

A thread with a running or queued turn cannot be contributed; the chain must be finished.

## 6. What makes it RL data rather than a chat log

A transcript alone is supervised fine-tuning data at best. RL needs a reward, or at least a
preference. cedar already records most of what a reward can be built from; the share dialog adds
the rest:

- **Explicit outcome**, asked at share time: `succeeded` / `partly` / `failed`, and an optional
  free-text "what was wrong" (redacted like everything else). This is the main reward signal.
- **Per-turn thumbs** (a small UI addition independent of contributing): a rating on any assistant
  turn, stored in `turns`, sent with the contribution if present.
- **Implicit signals, already recorded:**
  - `plan.updated` by `user`: the proposed plan vs the edited one is a preference pair.
  - `approval.resolved` with `deny`: the model asked for something the person refused.
  - `run.cancelled` by the user, a retry with a reworded prompt, or a follow-up that starts with a
    correction: negative evidence for the turn before it.
  - `stop_reason` and `error`: `max_turns` and `failed` runs are useful negatives.
  - Workspace: the files the task asked for exist (or don't).
- **Replayability.** Trajectories whose tools are all sandbox tools (`bash`, Python, files,
  documents, charts) can be re-executed in cedar-bench's hermetic stack, so they can seed
  verifiable tasks rather than stay offline data. The bundle tags each thread with the tool groups
  it used so a training pipeline can select these.
- **Provenance for the trainer.** Model id and name, cedar version, reasoning level, tool
  definitions as sent, and the sequence of model calls with `message_count` per call, so a
  trajectory can be re-tokenised exactly as the model saw it.

## 7. The central repository

Keep the ingest as dumb as possible so there is little to trust.

- **Ingest service** (`cedar-contrib`, its own small repo): `POST /v1/contributions` with the
  bundle; answers `{receipt_id, withdraw_token}`. `DELETE /v1/contributions/{receipt_id}` with the
  token withdraws. Nothing else. It validates the schema, re-runs the pattern scan (a submission
  that still carries a likely secret is refused with the span, not stored), caps size, rate-limits
  per pseudonym and per IP in memory only, and writes the bundle to a quarantine bucket. **No
  request logs with IPs**, and the policy says so; a person who doesn't want to trust that can
  send through Tor or a VPN, since the sender is their own server.
- **Receipts** live in cedar-chat (`contributions` table: conversation id, receipt id, withdraw
  token encrypted with `CEDAR_SECRET_KEY`, content hash, license, submitted and withdrawn at).
  Settings → Contribute lists them with a Withdraw button. Deleting the conversation keeps the
  receipt, so withdrawal stays possible.
- **Publication.** A periodic job builds a dataset release from quarantine: dedup by content hash,
  a second automated scan, a human spot-check of a sample, then a versioned release on Hugging Face
  (or a plain tarball) under the licenses chosen, split by license and by source model (§8).
  Withdrawn items are dropped from the next release. Already-published versions are immutable
  snapshots; the policy says plainly that a withdrawal removes the item from future versions and
  from the live quarantine, and cannot recall copies others downloaded.
- **Canary.** Every bundle and every release carries a canary string, as `bench/` does, so the
  corpus can be recognised and filtered where it should not be (for instance out of cedar-bench's
  own evaluation data, or by anyone whose policy excludes it).
- **Why not GitHub PRs** like the tool registry: a PR carries a GitHub identity, which is the
  opposite of a pseudonym. The registry's pinned-hash pattern is still right for the *downloaded*
  side: the published dataset's index lists SHA-256 per file.

## 8. Licensing, the part that bites later

- **User text.** The person licenses their prompts and feedback under the license they chose (CC0
  is the useful default for training; CC BY 4.0 for people who want credit by pseudonym).
- **Model outputs.** The assistant turns come from whatever model the operator registered.
  DeepSeek's weights are MIT-licensed, but a hosted provider's API terms are a separate question, and some forbid
  using outputs to train competing models. The bundle records `model` and `model_id`; the release
  job groups by model and ships only those whose terms allow it, and the ingest refuses models on
  a deny-list from the start so nobody contributes what can't be published.
- **Third-party content.** Fetched pages, emails, product pages, and documents in attachments are
  not the person's to license, which is why §2 stubs or excludes them by default.
- **Tool definitions and system prompt** are cedar's, Apache-2.0, fine.

## 9. Changes to cedar, in order

1. **PRIVACY.md**: a section "Contributing a conversation (opt-in)" that keeps the first sentence
   true: cedar collects nothing; *you* can send one conversation to a corpus whose operator and
   license are named in Settings, after reading exactly what goes, and withdraw it. Name the
   ingest host like the update check names GitHub.
2. **cedar (core)**: no new routes needed; `GET /v1/runs/{id}/trace` already returns system, tools,
   messages, calls and events, with attachment bytes already replaced by sizes
   (`trace::without_file_data`). One addition: `trace` should mark the span of the memory note in
   `system` so the chat can cut it exactly (today it is text inside the prompt).
3. **cedar-chat**:
   - `CHAT_CONTRIBUTIONS_URL` (unset = absent), `CHAT_CONTRIBUTIONS_LICENSES` (default `cc0,cc-by-4.0`).
   - Migration `0012_contributions.sql`: `contributions` receipts; `turn_ratings` (or a column on
     `turns`); the person's choices go in the existing `settings` table under a `contributions` key.
   - `contrib.rs`: build the bundle from the thread's traces, the three redaction passes, the
     model pass via `POST /v1/compact`-style call, submit, withdraw.
   - Routes: `GET /api/conversations/{id}/contribution` (the preview: bundle + proposed
     redactions + why-not reasons), `POST .../contribution` (send what the dialog holds, which is
     the full document, not a diff: the server sends exactly what it was given), `DELETE
     /api/contributions/{receipt}`; `GET/PUT /api/settings/contributions`.
   - UI: Settings → Contribute (`settings.js`), the More menu entry (`more-menu.js`), the review
     dialog (`contrib-view.js`/`.css`, built on `trace-view.js`'s rendering), thumbs on turns.
   - i18n strings in `chat/static/i18n/` and the glossary.
4. **iOS / Android**: the same per-thread action and settings page, calling the chat routes; the
   review dialog can start as a web view of the chat's dialog, as the trace view does.
5. **`cedar-contrib`**: the ingest service and the release job, with the schema (`schema_version`
   in every bundle) and the canary string shared with this repo's `bench/`.

## 10. Bundle schema (sketch)

```json
{
  "schema_version": 1,
  "canary": "CEDAR-CONTRIB CANARY <uuid>",
  "license": "cc0",
  "contributor": "p_8f3a…" ,
  "cedar_version": "0.3.4",
  "language": "en",
  "model": {"id": "deepseek", "name": "deepseek-ai/DeepSeek-V4-Flash", "reasoning": "high"},
  "tools": ["web_search", "web_fetch", "memory"],
  "outcome": {"result": "partly", "notes": "it ignored the CSV header"},
  "turns": [
    {
      "seq": 1,
      "prompt": "…redacted…",
      "system": "…memory note removed…",
      "tool_definitions": [ … ],
      "messages": [ … content blocks, stubs for dropped bodies … ],
      "calls": [{"seq": 1, "turn": 1, "message_count": 1, "stop_reason": "tool_use",
                 "usage": {"input_tokens": 1200, "output_tokens": 310}, "reasoning": null}],
      "events": [{"type": "plan.updated", "by": "user", "steps": [ … ]}],
      "rating": 1,
      "status": "completed", "stop_reason": "end_turn",
      "day": "2026-10-31"
    }
  ],
  "redactions": {"EMAIL": 2, "PATH": 1, "KEY": 0, "model_spans": 3},
  "dropped": {"attachments": 2, "memory": true, "fetched_bodies": 4}
}
```

## 11. Open questions

- **Who runs the ingest and under whose name?** The policy must name an operator people can hold
  to the no-logs promise. A project-run service and a Hugging Face organisation are the obvious
  pair.
- **Reasoning on or off by default** in the one-time settings? Off is the privacy answer; on is
  the training answer. This design says off, since the per-thread review still shows it when on.
- **Fetched page bodies**: stub (this design) or keep with a length cap? Keeping them makes
  browsing trajectories trainable but ships copyrighted text.
- **Should a thumbs-down alone be contributable without the thread?** Cheap signal, no privacy
  cost, but useless without the trajectory. Leave out.
- **Bulk review** for people who want to contribute many threads: a queue that opens the review
  dialog for each in turn is acceptable; a list with checkboxes is not.
