# Memory, revised: a plan

Status: proposal, November 2026. Nothing here is built yet.

cedar's memory today is a flat list of up to 200 short facts per API key, shown whole in every
prompt, written by a `remember` tool the model rarely calls and by one extraction call at the end
of each interactive run that reads only the message the user just sent. It is not useful in
practice. This document says why, grounded in the code and in the research review
*Agent Memory: When to Store, Expire, Refresh, and Retrieve* (Nov 2026), and lays out a
replacement in six phases. Everything is open to change; the privacy stance is not.

## 1. Why the current memory is not useful

The implementation is in `src/memory.rs`, `src/tools/memory.rs`, the memory hooks in
`src/agent.rs` (`run_agent`, `finish`, `note_facts`) and migration `0014_memories.sql`. The
problems, each mapped to what the research says about it:

1. **It only knows what the user said about themselves.** The extractor is shown the run's
   `prompt` and nothing else: not the conversation so far, not the answer, not what the agent
   did. So a continuation prompt like "yes, use that one" extracts nothing, "the project I
   mentioned" cannot be resolved, and the whole experiential side of memory (what worked, what
   the user's repository needs, how the user wants a weekly report laid out, which approach
   failed last time) is never written. Records hold rows of data, not know-how. The research
   treats episodic and procedural memory as the main value for task agents (ReasoningBank,
   AWM, ExpeL), written at episode end and gated on outcome.
2. **Writes are reconciled against nothing.** `Memories::remember` deduplicates by
   case-insensitive text equality only. "Lives in Bern" and "Lives in Zurich" coexist unless
   the model happens to pass `replaces`, and "Prefers metric units" and "Uses metric" are two
   items. The field standard is to retrieve the most similar existing memories and decide
   add / update / supersede / no-op against them (Mem0, Zep, SimpleMem, Memory-R1).
3. **There is no notion of time or validity.** A fact has `created_at` and `updated_at`
   (transaction time) and nothing about when it was true (event time). `replaces` overwrites in
   place and `forget` hard-deletes, so there is no history, no audit, and no way to answer
   "where did I live before". The settled position is to supersede with a closed validity
   interval and keep the old version (Zep, TSM, MemStrata, TOKI).
4. **Every item is in every prompt, oldest first, with a 36-character UUID each.** At 200 items
   that is tens of thousands of tokens of mostly irrelevant text ahead of every request, which
   is exactly the context-rot regime (Chroma, 2025) and the over-use regime (OP-Bench,
   MemCalib: ~84% of memory atoms should be ignored, and models over-use far more than they
   under-use). There is no ranking, no decay, no retrieval, and no rule for when memory should
   stay out of the way (opinion, creative and moral requests, where every tested system raised
   sycophancy).
5. **Nothing is typed.** A name, a diet, a standing instruction, "has an exam tomorrow" and
   "is migrating the blog to Hugo this month" are all the same kind of row and age the same
   way. Types should age differently (Supermemory, Mem-α, MemOS); temporary facts should
   expire; standing instructions should be pinned and always visible.
6. **The write policy is an instruction, not a mechanism.** "Never save anything you only read
   in a tool result" is a sentence in a prompt. Nothing checks that a saved fact is supported by
   the user's own words. The 2026 admission-gate work (ConsistencyGate, MemTxn, SAGE) makes
   source support a check between extraction and commit, which also closes the poisoning path
   (AgentPoison, MINJA) for a store that later reaches every run.
7. **Capacity is a hard wall.** At 200 the list refuses writes until the user or model deletes
   something. Nothing is dropped silently, which is right, but nothing is ranked either, so the
   wall arrives with the store full of stale and duplicate items. Usage-based decay of *rank*
   with a floor is empirically free (Xiong et al. 2025, Selective Forgetting 2026); hard
   deletion should be reserved for user, privacy and security requests.
8. **No measurement.** cedar-bench has no memory task. There is no way to say whether a change
   made memory better, nor whether memory helps at all against the same model with no memory
   (the "context saturation gap" the Anatomy of Agentic Memory paper proposes as the first
   diagnostic).

What is worth keeping: memory lives only in cedar's own SQLite database and goes nowhere but the
configured model server; the note is rebuilt per request so a deletion applies to the very next
run; the trace shows exactly what the model was sent and where the memory sits in the prompt
(`memory_note`), which the contributions export relies on; nothing is dropped silently; unattended
runs cannot write facts about the user; the user sees and deletes everything in Settings on four
clients. All of these survive the revision.

## 2. Goals and principles

- **Remember what a personal agent needs across conversations:** who the user is and how they
  want things done (profile), facts and preferences with their history, what happened in past
  conversations (episodes), and how to do recurring things well (procedures).
- **Use memory only when it helps.** A small core always in context; the rest recalled when the
  request calls for it, with an explicit selection step; nothing recalled for requests where
  memory makes answers worse.
- **Supersede, never silently lose.** Every change is a new version with a closed interval and
  an audit row. Hard deletion only on the user's request.
- **Mechanisms over instructions.** Source support, novelty, typed expiry, deterministic
  supersede for keyed facts, and outcome gating are checks in code, not sentences in a prompt.
- **Privacy unchanged.** Nothing leaves the server but the request to the model. Facts about
  the user come only from the user's own words, enforced by a span check. The user sees every
  memory, where it came from, when it was recalled, and can edit, pin, archive and delete.
- **Cheap on the request path.** No model call before the answer. Extraction stays after the
  answer, as today; consolidation moves off the request path entirely.
- **Measurable.** A memory suite in cedar-bench, judged against the no-memory baseline.

Non-goals: a vector database or any new service (SQLite does it all at this scale), training a
memory policy (cedar talks to frontier and open models through an API; the research's
RL-trained managers are for people who train the model), and remembering across API keys.

## 3. The memory model

Five kinds of memory, in one table, typed at write time because they age and surface differently.

| Kind | What it holds | Source | Shown | Ages |
| --- | --- | --- | --- | --- |
| `profile` | One capped, continuously rewritten summary of the user: name, how to address them, where they are, their work, standing instructions, current projects. At most ~400 tokens. | Consolidation, from facts and instructions; user can edit directly | Always, in every run that has the `memory` group | Rewritten, never decays |
| `fact` | One atomic statement about the user with references resolved and absolute dates ("Lives in Zurich since 2024-03"). Optionally keyed by (subject, relation) for deterministic supersede. | User's own words, via extraction or `remember` | Recalled when relevant; the current fact for a key is also folded into the profile when important | Superseded when contradicted; rank decays with disuse |
| `preference` | How the user wants things: units, language, tone, formats, tools they prefer. | User's own words | Recalled when relevant; strong ones go into the profile | Strengthens with repetition, decays with disuse |
| `instruction` | A standing request ("always cite sources", "remind me to stretch when I ask for a long task"). Pinned by default. | User's own words, or user-added in Settings | Always, in the profile | Never decays; user removes |
| `temporary` | A fact with a known end ("exam on Nov 12", "away until Friday"). | User's own words | Recalled while valid | Expires at `valid_until`; then archived |
| `episode` | What a conversation was about and how it ended: a title, a third-person summary, the outcome, the files and tools involved, the workspace. One per run chain. | The agent's own trajectory, written at run end, no judge needed | Recalled on recall cues ("like last time", "the script we wrote") or topic overlap | Rolled up into monthly summaries by consolidation; raw rows archived, never deleted without the user |
| `procedure` | How to do a recurring thing for this user, abstracted: title, one line, the rationale, the pitfall to avoid. Never concrete command traces. | The agent's own trajectory, at run end, gated on outcome; or the user stating it | Recalled by topic overlap with the task | Rank decays with disuse; a procedure whose runs keep failing is archived |

Every row carries:

- **Provenance:** `source` (`user`, `agent`, `consolidation`, `api`), `run_id`, and for
  user-sourced kinds the `support` span: the user's own words the fact is taken from.
- **Bi-temporal time:** `valid_from` / `valid_until` (event time, when it was true) separately
  from `created_at` / `updated_at` (when cedar learned it).
- **Versioning:** `superseded_by`, so a contradicted fact is closed and linked, not deleted;
  `archived_at` for rows out of the active set; `pinned`.
- **Confidence:** `hedged` (the user said "I think" or "probably"); the note preserves the
  hedge instead of turning hearsay into a fact.
- **Usage:** `recall_count`, `last_recalled_at`, which drive rank.
- **Topics:** a small set of tags (project names, domains) for cheap scoping.
- A short per-owner **sequence number** shown to the model as `m42` instead of a UUID, which
  cuts the note's token cost by more than half on its own.

A separate `memory_ops` table is the audit log: every add, update, supersede, archive, restore,
pin, edit and delete, with who did it (a run, consolidation, the user through the API), when,
and the before and after text. Settings → Memory gets a history view from it.

Retrieval index: an FTS5 table over `text` and `topics` (SQLite's bundled build has FTS5, so no
new dependency), plus an optional `embedding` column filled through the model server's
`/v1/embeddings` when an admin registers an embedding model. Stores are hundreds to a few thousand
rows per owner, so brute-force cosine over the owner's rows is fine; no vector index.

Exclusions become a mechanism too: the extractor is told what not to save, and the admission
gate rejects government and financial identifiers, credentials and anything matching the secret
patterns `cedar-contrib-schema` already has. Health and diet stay allowed by default (users of a
self-hosted assistant want "vegetarian" remembered) but are tagged `sensitive`, which Settings can
turn off; the same `health_data` rules records use apply when a sensitive memory is in the note.

## 4. The write path

Two moments, two policies, as in every strong 2025–2026 result: cheap per-turn writes
reconciled against similar memories, and episode-end writes gated on outcome.

### 4.1 Every run leaves an episode row, with no model call

When a run finishes, cedar writes an `episode` row directly: the prompt, the final answer's first
lines, the tools used, files written, the outcome (`completed` / `failed` / `cancelled` /
`ask_user`), the workspace and the chain's parent. This is the raw layer every LongMemEval
leader keeps beneath its extracted facts. It costs nothing and is what consolidation and
recall cues read later. Unattended runs write it too.

### 4.2 Fact extraction, rebuilt

The post-answer extraction call stays, but changes in four ways:

- **Context.** It sees the last few exchanges of the conversation (user and assistant text only,
  never tool results or attachments) with the newest user message marked, so references resolve
  and "yes, that one" is understood. It still saves only what the user said.
- **Reconciliation.** Before the call, cedar retrieves the top-10 most similar active memories
  (FTS5 + embeddings when available) for each candidate the model proposes, and the model decides
  per candidate: `add`, `update` (same fact, more detail), `supersede` (the old one is no longer
  true, give `valid_until`), or `noop`. Never `delete`: cosine similarity cannot tell a
  contradiction from a paraphrase (MemStrata, AUROC 0.59), and a second dog is not a
  contradiction of the first (Memory-R1's motivating case). For keyed facts (same subject and
  relation, different object: home city, employer, partner's name) the supersede is deterministic
  and the model is not asked.
- **Admission gate.** Each candidate must quote its `support` span, and cedar checks the span
  occurs in the user's own text of this conversation. No span, no save. This is what makes "never
  from a tool result, email or web page" a property rather than a request, and it is the poisoning
  defence. A second, cheap novelty check skips candidates whose text is within a small edit or
  similarity distance of an active memory without calling anything.
- **Typing and normalising.** The model assigns a kind, resolves relative dates to absolute ones
  using the run's clock and locale, marks hedges, and proposes topics. cedar validates all of it.

`MEMORY_EXTRACT=off` still turns this off and leaves only the tools.

### 4.3 Experience at run end, gated on outcome

For runs that used tools and ended `completed`, a second short call reads the trajectory (tool
calls summarised, results trimmed, as `/v1/compact` already does) and writes at most one
`procedure` candidate and the episode's title and summary. The gate is in code:

- A run that was cancelled, failed, or ended in `ask_user` writes no procedure.
- A procedure is stored as title + one line + rationale + pitfall, with commands, paths and
  values removed; concrete traces are what made failure memories hurt in AWM.
- Failures are stored only as the pitfall line of an episode ("the Hugo build fails when the
  theme submodule is missing"), never as a workflow.
- A procedure starts with low rank and is promoted by recall and by later successful runs that
  recalled it; a procedure recalled by runs that then failed, or whose next run in the chain
  opens with a correction, is archived by consolidation. Agents imitate what they retrieve
  ("experience following"), so a flawed procedure must not compound.
- If the chat passes its thumbs rating through to cedar (`POST /v1/runs/{id}/feedback`, new), a
  thumbs-down archives the run's procedure and a thumbs-up promotes it.

Unattended runs write episodes and may write procedures (they are the agent's own experience,
not facts about the user), but never user facts; the read-only rule for `remember` stays.

### 4.4 The tools

`remember` and `forget` stay for the model, with `remember` gaining `kind`, `valid_until` and
`supersedes`, and `forget` becoming an archive rather than a delete. A new `recall` tool
(section 6) lets the model search memory itself.

## 5. Keeping memory current: consolidation off the request path

A background pass per owner, run by the manager when an owner has had `MEMORY_CONSOLIDATE_AFTER`
finished runs (default 5) since the last pass or has been idle for an hour with pending changes.
It never runs on the request path. Each pass is one to three model calls over the owner's active
rows and recent episodes, and every change it makes is an audit row. It is deliberately
conservative: over-consolidation measurably hurts, and systems that restructure on every write pay
30× in latency.

What it does, in order:

1. **Merge duplicates** that the novelty gate let through (two phrasings of one fact) into one
   row, keeping the older one's id and `valid_from`.
2. **Resolve open conflicts** the extractor could not: two active facts for one key, or a static
   fact (a birthplace, a name) contradicted by one later hedged mention. The rule is not
   last-writer-wins: a `pinned` or long-stable fact needs a clear, unhedged statement to be
   superseded; otherwise the newer row is kept as `hedged` beside it and the user is shown both.
3. **Rewrite the profile** from pinned instructions, current keyed facts, strong preferences and
   active projects, within its token cap, and store the old profile as a superseded version.
4. **Roll up episodes** older than a month into one monthly summary row per topic, archiving
   the raw rows (still listed in Settings, still searchable, never deleted).
5. **Age and archive:** expire `temporary` rows past `valid_until`; archive procedures with
   repeated failed follow-ups; recompute rank (section 7). Nothing is deleted.

The current `/v1/memory` promise "what you see is what the model sees" becomes "what the model
saw in a run is in its trace, and Settings shows the whole store with what is always shown, what
is recalled, and what is archived". The trace's `memory_note` span stays, and gains a `memory`
section listing the rows the run was given and why (score, cue, pinned).

## 6. The read path: a core always in context, the rest recalled and selected

Per request, as today, cedar builds the memory part of the system prompt last, so its span can
still be cut. Its content changes:

1. **Always:** the profile and every pinned instruction. This is the only reliable way to make
   standing instructions and "remind me when" memories fire (TriggerBench), and it is the
   cacheable prefix that Letta blocks and Mastra's observation log rely on.
2. **Recalled:** candidates retrieved by hybrid search over the prompt plus the newest user
   turns, with the current date for temporal filtering:
   - Facts, preferences and temporary rows above a similarity threshold.
   - Episodes and procedures only on a cue: a recall phrase ("last time", "again", "the script
     we wrote", "as before"), an explicit date or relative time, a topic or workspace shared
     with the run, or a prompt that names a file or project the episode involved.
   - A `recall` tool the model can call itself with a query and optional kind and time range,
     for the cases the gate misses. The Letta result (a grep-and-read agent over raw transcripts
     beating a vector pipeline) says self-generated queries matter more than the backend.
3. **Selected:** retrieve with high recall (widened k, lower threshold), then apply an explicit
   selection step in code before anything reaches the prompt: drop superseded and archived rows,
   drop a row whose key has a newer current value, drop rows out of their validity interval,
   and keep the budget to `MEMORY_RECALL_TOKENS` (default 600) beyond the profile. More evidence
   without selection lowers accuracy (Obsolete Facts, MemChain).
4. **Stayed out of:** when nothing clears the threshold and there is no cue, only the profile is
   shown. That is the suppression rule for opinion, creative and moral requests, without a
   classifier. The note keeps framing memory as the user's facts, not instructions, which is the
   one framing that did not raise sycophancy in the harm benchmarks.

Records are untouched: the records note stays, and `recall` may answer with a pointer to a
record set when an episode mentions one.

## 7. Expiry and rank

Rank, not deletion, with a floor:

- A row's rank is its kind's base weight times a disuse factor keyed to `last_recalled_at`, with a
  floor of 0.3 so nothing silently vanishes (Mem0's decay, FadeMem). Pinned rows and the profile
  have no decay. Preferences gain weight with each repetition the extractor sees.
- Kinds have expiry rules: `temporary` expires at `valid_until`; episodes are rolled up after a
  month; procedures are archived after N recalls by failing runs; facts, preferences and
  instructions never expire on age.
- Capacity: the per-owner cap becomes per-kind budgets (facts and preferences 500, episodes
  2,000 raw, procedures 200, configurable). At a budget, consolidation archives the lowest-rank,
  never-recalled rows first. The model is never refused a write because the list is full; the
  user sees archived rows in Settings and can restore them.
- Hard deletion happens only through the user: `DELETE` in Settings or the API, "forget this" in
  a conversation (which archives, then the user confirms deletion in Settings), or `DELETE
  /v1/memory` for everything, including audit rows.

## 8. API, trace and clients

- `GET /v1/memory` lists active rows by kind, with `kind`, `pinned`, `source`, `support`,
  `valid_from`, `valid_until`, `recall_count`, `last_recalled_at`, `superseded_by`; `?archived`
  and `?kind` filters; `?q` searches.
- `GET /v1/memory/profile` returns the profile; `PUT` lets the user edit it, and the edit is
  pinned against the next consolidation rewrite.
- `POST /v1/memory` takes `kind`, `pinned`, `valid_until`; `PATCH /v1/memory/{id}` edits text,
  pins and unpins, archives and restores; `GET /v1/memory/{id}/history` returns its versions and
  audit rows; `DELETE` stays.
- `GET /v1/memory/export` and `POST /v1/memory/import` (JSON), so a user can move between
  servers and back up.
- `POST /v1/runs/{id}/feedback` with `{rating: up|down}` for the outcome gate.
- Events: `memory.updated` keeps its shape and gains `kind` and `op`; a new `memory.recalled`
  event at run start lists what was shown (ids and why), so the chat can show a small "using
  memory" line the user can expand.
- Trace: the `memory` call stays; a `memory` section lists the rows shown with scores and cues;
  `memory_note` stays as the cut span for contributions.
- Settings → Memory on the web, desktop, iOS and Android: the profile at the top (editable),
  then tabs or filters by kind, pinned first, each row with its support quote, source run, when
  it was last used, a history button, pin, edit, archive and delete; an Archived list; Export.
  Under a reply, the existing *remember* steps are joined by *recalled* and *noted* steps.
- `GET /v1/defaults` reports the new limits under `memory`.

Configuration: `MEMORY`, `MEMORY_EXTRACT` stay. New: `MEMORY_EXPERIENCE` (procedures and
episode summaries, on), `MEMORY_CONSOLIDATE` (on) and `MEMORY_CONSOLIDATE_AFTER` (5),
`MEMORY_RECALL_TOKENS` (600), `MEMORY_PROFILE_TOKENS` (400), per-kind budgets, and
`MEMORY_EMBEDDING_MODEL` (the id of a registered model whose server offers `/v1/embeddings`;
without it, FTS5 alone). `MEMORY_MAX_ITEMS` is read as the facts budget for one release, then
removed.

## 9. Migration

One migration, `0017_memory_v2.sql`: a new `memories` table with the columns above, `memory_ops`,
the FTS5 table and triggers. Existing rows become `kind = fact`, `source = user`, `valid_from =
created_at`, `support = NULL` (grandfathered; shown as "no quote" in Settings), with their
`run_id`. The first consolidation pass after upgrade writes each owner's profile. The old table is
dropped at the end of the migration. Clients that predate the new fields keep working: the list
endpoint's existing fields keep their names and meaning.

## 10. Evaluation, before anything else

cedar-bench gets a `memory` category, using the three personas and the fixture server, with
multi-session tasks: a fact stated in session one and needed in session five; a fact updated in
session three and the old value asked about historically; a standing instruction that must fire in
an unrelated session; a procedure learned in one run and reusable in a later one; a temporary fact
that must not be used after it expired; and negative controls: a creative request and an opinion
question where the right answer uses no memory, and a prompt with high lexical overlap with a
memory that is nonetheless irrelevant. Each task is scored on the answer and on what the trace
shows was recalled, so over-use and under-use are measured separately, and every model runs each
task with `MEMORY=off` and with full history in context as the two baselines. A change to memory
ships only if the gap against those baselines does not shrink.

Unit and integration tests mirror today's `tests/memory.rs` for each phase: the span check
rejects a fact quoted from a tool result; a keyed contradiction supersedes without a model call; a
superseded fact is not recalled but is in history; the profile is in every run and a recalled fact
only in the run that matched; consolidation merges a duplicate and never deletes; the budget
archives the lowest rank and Settings lists it.

## 11. Phases

Each phase ships on its own, behind the existing `MEMORY` flag, with docs, CHANGELOG and tests.

| Phase | What | Why first | Rough size |
| --- | --- | --- | --- |
| 0 | Bench memory suite and baselines; `memory.recalled` event and trace section on the current implementation; short ids in the note | Without a measurement, every later phase is a guess. Short ids alone cut prompt cost at once | 1 week |
| 1 | Schema v2: kinds, bi-temporal columns, supersede, archive, pin, audit log, FTS5; API and four clients updated; `forget` archives | The foundation every other phase writes to; the user-visible history and pin controls are useful on day one | 2 weeks |
| 2 | Extraction rebuilt: conversation context, reconciliation against similar rows, deterministic keyed supersede, admission gate with span check, typing and date normalisation; episode row per run | Fixes the duplicate, stale and provenance problems; the span check closes the tool-result path for good | 2 weeks |
| 3 | Read path: profile (hand-written at first, then from phase 4) always in context, hybrid recall with cue rules, selection step, token budget, `recall` tool | The step that stops stuffing the prompt; measured against phase 0 for over-use and under-use | 2 weeks |
| 4 | Consolidation pass in the manager: merge, conflict resolution, profile rewrite, episode roll-up, expiry and rank | Moves every heavy operation off the request path; makes the profile real | 2 weeks |
| 5 | Experience: procedures and episode summaries at run end, outcome gate, feedback endpoint, promotion and archiving by later outcomes | The part that makes memory useful for work, not just small talk; last because it depends on the gate, rank and consolidation to stay safe | 2 weeks |
| 6 | Optional embeddings through a registered model; export and import; sensitive-kind toggle | Quality and portability once the mechanism is proven | 1 week |

Phases 1 and 2 can be one pull request if the schema is settled first. Phase 3 is the one to
measure hardest: it changes what every run sees.

## 12. Decisions to make

1. **Unattended writes.** This plan lets scheduled runs write episodes and procedures, not user
   facts. The alternative is to keep every unattended write off, which keeps the current rule
   simple and loses what a weekly agent learns about its own job.
2. **Health and diet.** On by default with a `sensitive` tag and a toggle, as proposed, or off by
   default as the hosted assistants do.
3. **Embeddings.** FTS5 alone is good enough for a start and needs nothing new; an embedding model
   is a second server for most people. Keep it optional, or skip it entirely until the bench
   shows FTS5 missing paraphrases.
4. **The profile's editability.** A user edit pins the profile against rewrites. Should
   consolidation still append below the user's text, or propose changes the user accepts in
   Settings?
5. **Chat feedback.** Passing thumbs ratings to cedar's feedback endpoint is the cleanest outcome
   signal, and it is the first time the chat tells cedar something about a run after the fact.

## 13. Research mapping

| Report rule | Where it lands in this plan |
| --- | --- |
| 1 Raw turns as cheap inserts | Episode row per run, no model call (4.1) |
| 2 Reconcile against top-10 similar; never delete on similarity | Extraction rebuilt (4.2) |
| 3 Admission gate: novelty and source support | Span check and novelty check (4.2) |
| 4 Experiential memory at episode end, judged, failures abstracted | Procedures and pitfalls (4.3) |
| 6 Three granularities | Episodes, facts, profile (3) |
| 7 Type at write time | `kind` column (3) |
| 8 Provenance, event vs transaction time, hedges | Row columns (3) |
| 9 Exclusions and user controls | Gate patterns, sensitive tag, Settings (3, 8) |
| 10 Decay rank by last access with a floor; prune only under capacity | Rank and budgets (7) |
| 11 Supersede with validity intervals; deterministic keys | `superseded_by`, keyed facts (3, 4.2) |
| 12 Freeze contract-like facts | `pinned`, stable-fact rule in consolidation (5) |
| 13 Sleep-time consolidation, conservative | Consolidation pass (5) |
| 14 Core always in context | Profile and pinned instructions (6) |
| 15 Retrieve on cues and need | Cue rules and threshold (6) |
| 16 Suppress for opinion and creative requests | Profile-only when nothing matches (6) |
| 17 High recall then explicit selection, small token budget | Selection step and `MEMORY_RECALL_TOKENS` (6) |
| 18 Overwrite-style state for revised values | Keyed facts; records for data (3) |
| 19 Measure over-use and under-use; judge against full context | Bench suite (10) |
