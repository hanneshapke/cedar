# Run evidence and an offline verifier: a plan

Status: proposal, November 2026. Nothing here is built.

This document reviews the verifier concept in
[probityai/agent-evidence-vectors](https://github.com/probityai/agent-evidence-vectors) (read at
v0.36.3) and plans how cedar could offer the same thing in Rust: a signed record of what a run
did, and a small program that checks that record offline and says what it does and does not show.

## 1. The concept being replicated

The reference repository is three things that depend on each other.

**An evidence record.** An in-toto Statement whose predicate carries *observation records*: one
DSSE envelope per thing an observer saw, each signed by the observer's Ed25519 key. The records
are leaves of an RFC 6962 Merkle tree (`leaf = H(0x00 || PAE)`, `node = H(0x01 || l || r)`, no
padding), and the statement carries the root. On top of the records sit *rows*: readable claims
that each cite the records they rest on. A one-word `result` summarises the rows.

**A verifier.** A pure function from `(statement bytes, consumer policy)` to a report. No
network, no account, no database. It runs fixed stages and stops at the first that fails:

| Stage | Question |
| --- | --- |
| Gate 0, well-formedness | Are these bytes one unambiguous document of the expected type? Strict JSON (no duplicate members, RFC 8785 canonical payloads), closed vocabularies, digest shapes. |
| Gate 1, coverage | Does every carried record hash into the signed root, with none added, removed or duplicated? Does every row cite records that exist and can carry its claim? |
| Recompute | Does the carried `result` equal the result computed again from the rows? |
| Gate 2, evidence tier | Per row: `attested` if its records verify under a key *the consumer pinned*, `unattested` if not, `declared` if it rests on the producer's word alone. |
| Admission | Does the valid statement also meet the consumer's own rules (accepted results, age, pinned anchors)? |

The rules that make it trustworthy are worth more than the stages:

- Keys come from the consumer, out of band. A record's `keyid` is a lookup hint, never a source.
- An invalid statement yields no result and no tiers, so nobody can consume a result they were
  told to refuse.
- Signature outcomes change the tier, never validity or the recomputed result. The same bytes
  give the same verdict with and without keys.
- A limit is not a pass. A valid record that nobody you trust signed is reported as
  `NOT_PUBLICLY_VERIFIABLE`, and the consumer decides whether that is enough.
- Failures map to a small closed set of verdict codes (`MALFORMED`, `LEAF_HASH_MISMATCH`,
  `PROOF_BROKEN`, `CLAIM_NOT_BOUND`, `SUMMARY_MISMATCH`) plus two limits. A code the verifier
  does not know is a failure.
- The report lists which members are only the producer's words (`producerAsserted`).

**A conformance corpus.** Saved records with known outcomes (valid, one byte changed, a record
removed, evidence missing), a manifest of expected verdicts, and a harness that runs any verifier
as `<cmd> <file>`, reading the exit status and one JSON object on the last line of stdout. The
corpus is what stops a verifier from quietly accepting something, and it runs in CI.

The reference's main predicate (Adversarial Execution Evidence) describes an attack corpus run
against a containment substrate, which is not what a cedar run is. Its other predicates are
closer in spirit: the *done receipt* gives each claim a verdict of `supported`, `contradicted` or
`not_established`, and the *self-reported record* corpus states the one rule that decides
whether any of this means anything: **the attesting key must not be one the observed party can
mint.**

## 2. What this means for cedar

cedar already has the raw material: an append-only `events` table per run, the verbatim
`messages`, the `llm_calls` trace, approvals, sandbox denials, and the workspace files. What it
lacks is that none of it is sealed. A row in SQLite can be edited after the fact and
`GET /v1/runs/{id}/trace` will serve the edit.

**Who observes whom.** In cedar the observed party is the agent: the model and whatever it runs
in the sandbox. The observer is the cedar server, which executes every tool call itself and
holds a key the sandbox cannot reach. So a cedar-signed record is real evidence *against the
agent*: the model cannot claim a tool call it did not make, or hide one it did.

It is not evidence against cedar's operator, who holds the key. The plan says so in the report
rather than hiding it: everything cedar signs is attested only for a consumer who chose to pin
that server's key, and phase 5 adds an outside witness for consumers who will not.

**What cedar did not observe.** Some facts cedar only relays, and they must be tiered `declared`
however many signatures surround them: the model name the LLM server reports, its token counts,
the user's prompt as the client sent it. cedar can attest that it *received* those bytes, not
that they are true.

### Design decisions

1. **A cedar predicate, the reference's building blocks.** Define
   `https://cedar.dev/attestation/run-evidence/v0.1` (URI to be settled) rather than forcing a
   run into the AEE shape. Keep the envelope, the Merkle construction, the canonical JSON profile,
   the report fields and the harness contract identical, so the reference runner and any
   in-toto/DSSE tooling can read cedar's output.
2. **Seal at append, not at export.** Each event is canonicalised and signed in the same
   transaction that inserts it. Sealing at export would let the database be edited in between.
3. **Large content by digest.** Tool output, file bytes and attachments are named by SHA-256 in
   the signed record and carried beside the statement as optional artifacts. A bundle without
   them still verifies; claims that need the missing bytes come back `not_established`.
4. **The verifier is its own crate with no server dependencies**, so it can be audited, built for
   wasm, and installed without cedar.

## 3. The record

One observation record per event, as a DSSE envelope with payload type
`application/vnd.cedar.observation+json`. The payload is RFC 8785 canonical JSON:

```json
{"run":"<uuid>","seq":7,"kind":"tool.call","at":"2026-11-11T10:02:11Z",
 "prev":"<leaf hash of seq 6, hex>",
 "data":{"id":"t3","name":"bash","input":{"sha256":"…","bytes":212}}}
```

`prev` chains each record to the one before it, so a verifier can tell a truncated tail from a
complete run even before the root is issued. `data` is the event's existing JSON with any value
over a size threshold replaced by `{sha256, bytes}`.

Two new record kinds are added to what `events` holds today:

- `run.sealed.start`, written before the first model call: digests of the system prompt as sent,
  the tool definitions as sent, the sandbox image, the sandbox policy, and the model
  registration (`base_url`, `model`).
- `run.sealed.end`, written with the terminal event: the record count, digest of `final_text`,
  and a manifest `{path, sha256, bytes}` of the workspace files.

The statement, issued when the run ends, is itself a DSSE envelope:

```json
{"_type":"https://in-toto.io/Statement/v1",
 "subject":[{"name":"run:<uuid>","digest":{"sha256":"<run binding>"}}],
 "predicateType":"https://cedar.dev/attestation/run-evidence/v0.1",
 "predicate":{
   "result":"completed",
   "recordCount":42,
   "batchRoot":"<hex>",
   "issuedAt":"…",
   "observer":{"keyid":"<sha256 of the public key>","version":"0.7.0"},
   "claims":[
     {"type":"tool_call/v1","basis":"observed","refs":[7,8],"call":"t3","name":"bash","isError":false},
     {"type":"approval/v1","basis":"observed","refs":[9,10],"decision":"allow","by":"user"},
     {"type":"sandbox_denied/v1","basis":"observed","refs":[14]},
     {"type":"model/v1","basis":"declared","refs":[1,3],"model":"deepseek-ai/DeepSeek-V4-Flash"},
     {"type":"output_file/v1","basis":"observed","refs":[41],"path":"hello.py","sha256":"…"}
   ],
   "observationRecords":[ … DSSE envelopes … ]}}
```

`result` is `completed`, `failed` or `cancelled` and must equal what the terminal record says.
The run binding is `sha256(run id || start-record leaf hash)`, so a statement cannot be re-pointed
at another run.

## 4. The verifier

Stages mirror the reference, with cedar's own code vocabulary:

| Stage | Checks | Codes (verdict code) |
| --- | --- | --- |
| Gate 0 | Strict parse, statement and predicate type, closed vocabularies, digest shapes, canonical payloads | `statement-malformed`, `payload-not-canonical` (`MALFORMED`) |
| Gate 1 | Leaf hashes reach `batchRoot`; `recordCount` matches; `seq` contiguous from 1; `prev` chain holds; exactly one start and one end record; no duplicates; every claim's `refs` resolve to records of a kind that can carry it; run binding | `batch-root-mismatch`, `record-missing`, `chain-broken` (`PROOF_BROKEN`); `run-binding-mismatch` (`LEAF_HASH_MISMATCH`); `ref-out-of-range`, `claim-uncovered` (`CLAIM_NOT_BOUND`) |
| Recompute | `result` equals the terminal record's; every `observed` claim's fields equal the fields of the records it cites; no tool call or approval in the records lacks a claim | `result-recompute-mismatch`, `claims-incomplete` (`SUMMARY_MISMATCH`) |
| Gate 2 | Per claim: `attested` when every cited record and the statement verify under a pinned key, `unattested` otherwise, `declared` for `basis: declared` | tiers; limits `NOT_PUBLICLY_VERIFIABLE`, `NOT_RECONSTRUCTIBLE` |
| Artifacts | For each artifact supplied, bytes hash to the digest a record names | `artifact-digest-mismatch` (`LEAF_HASH_MISMATCH`); absent artifact is `not_established`, not a failure |
| Admission | Consumer policy: accepted results, maximum age, required tier, pinned image and policy digests | `policyCodes`, `admitted` |

The report keeps the reference's field names (`verdict`, `codes`, `primaryCode`, `result`,
`tiers`, `policyCodes`, `admitted`, `producerAsserted`, `verdict_code`) so its harness can score
it unchanged.

On top of record verification, a claim check in the style of `probityai` and the done receipt:
the user states something about the run and gets `supported`, `contradicted` or
`not_established`. Initial claim vocabulary: `tool-called`, `tool-not-called`,
`no-sandbox-denials`, `approved-by-user`, `file-sha256`, `image-digest`, `policy-digest`,
`final-text-sha256`.

## 5. Rust layout

| Piece | Where | Notes |
| --- | --- | --- |
| `cedar-evidence` | new workspace member `evidence/` | Library plus the `cedar-verify` binary. No tokio, sqlx or axum. Depends on `sha2`, `hex`, `serde_json`, `base64`, and the reference's own Rust crates from crates.io: `dsse` (envelope, PAE, Ed25519 threshold verification) and `jcs-admit` (strict admission and RFC 8785). `#![forbid(unsafe_code)]`, no `unwrap` outside tests. |
| Types and stages | `evidence/src/{statement,merkle,gate0,gate1,recompute,tier,admission,codes,claims}.rs` | One file per stage, as in the reference's Go core, behind one `verify(bytes, &Policy) -> Report`. A sealed `EvalContext` that only `evaluate()` can build, so later stages cannot run on an unvalidated statement. |
| Producer | `src/evidence.rs` in the server | Builds payloads, signs, builds the statement. Calls `cedar_evidence::verify_for_emit` before signing the statement and refuses to sign one that fails its own gates. |
| Sealing | `src/store.rs` `insert_event`, migration `0019_evidence.sql` | Adds `payload BLOB`, `leaf_hash BLOB`, `signature BLOB` to `events`, and an `evidence` table for the issued statement. Canonicalise, chain and sign inside the insert transaction. |
| Key | `src/config.rs`, `src/secrets.rs` | `CEDAR_EVIDENCE_KEY` (32-byte Ed25519 seed, hex), or generated on first start and stored encrypted under `CEDAR_SECRET_KEY`. Evidence is off when neither is set. Never passed into a sandbox. |
| API | `src/api.rs` | `GET /v1/runs/{id}/evidence` (the statement; `?artifacts=true` for a tar bundle with tool outputs and workspace files), `GET /v1/evidence/key` (the public key and keyid, for convenience only). `GET /v1/defaults` gains `evidence: {keyid} | null`. |
| Corpus | `evidence/vectors/` | `MANIFEST.json`, statements named by digest, a generator that derives every reject vector from an accept vector by one named edit. `cargo test -p cedar-evidence` replays it; a CI job also runs it through the binary under the reference's harness contract. |

`dsse` 0.1.6 needs Rust 1.88; the repository builds on 1.97, so that is no obstacle. If taking a
dependency on the reference's crates is unwanted, PAE and the Merkle tree are about sixty lines
and `ed25519-dalek` with `serde_json_canonicalizer` covers the rest; the strict duplicate-member
parse is the part worth not rewriting.

## 6. User workflow

**Operator, once.** Set `CEDAR_EVIDENCE_KEY` (or let cedar generate one), restart, and publish
the public key somewhere consumers already trust: the organisation's site, a repository, a
signed email. `GET /readyz` reports `evidence: on` and the keyid.

**Person running a task.** Nothing changes. The run ends, and the chat UI shows a "Verified
record" entry on the run with a download. Through the API:

```sh
curl -H "Authorization: Bearer $KEY" localhost:8080/v1/runs/$ID/evidence -o run.evidence.json
curl -H "Authorization: Bearer $KEY" "localhost:8080/v1/runs/$ID/evidence?artifacts=true" -o run.evidence.tar
```

**Person receiving the record** (a reviewer, an auditor, a CI job, the same user a year later).
They need the file, the verifier, and a key set they chose. No cedar server, no account.

```sh
cargo install cedar-evidence          # or a release binary, checked against its published hash

cat > keys.json <<'EOF'
{"observerKeys": [{"keyid": "…", "publicKeyHex": "…"}]}
EOF

cedar-verify run.evidence.json --keys keys.json
```

```text
run 0193…c2  completed  2026-11-11T10:04:52Z
valid   42 records   root 9f3a…   signed by a pinned key
  attested   tool_call      bash (t3)             ok
  attested   approval       gmail.send  allow     by user
  attested   output_file    hello.py              sha256 4be1…
  declared   model          deepseek-ai/DeepSeek-V4-Flash   (the model server's word)
  declared   usage          12,480 in / 911 out             (the model server's word)
verdict: supported   admitted: yes
```

Exit status 0 means valid and admitted. The same file without `--keys` still verifies its hashes
and exits non-zero with `NOT_PUBLICLY_VERIFIABLE`, unless the caller passes
`--allow NOT_PUBLICLY_VERIFIABLE`. A changed byte gives `contradicted` with the code that caught
it and no result at all.

Checking a specific claim:

```sh
cedar-verify run.evidence.tar --keys keys.json \
  --claim tool-not-called=gmail.send \
  --claim file-sha256=report.pdf:4be1… \
  --claim no-sandbox-denials
```

Each claim prints `supported`, `contradicted` or `not_established` with the records it rested on.
`file-sha256` against a bundle without artifacts is `supported` from the sealed manifest alone;
`--artifact-root dir/` additionally checks the bytes on disk.

**In CI.** `cedar-verify --json` prints one JSON object on its last line and sets the exit status,
so it drops into a workflow step, and into the reference harness:

```sh
uvx agent-evidence-vectors --corpus <cedar corpus> --verifier "cedar-verify --json"
```

**Someone writing their own verifier** runs it against `evidence/vectors/` the same way. That is
the point of shipping the corpus: cedar's verifier is one implementation, not the definition.

## 7. Phases

| Phase | Delivers | Done when |
| --- | --- | --- |
| 0. Spec and vectors | `docs/design/run-evidence-spec.md` (the predicate, the codes, the verdict-code map), a hand-built accept vector and one reject vector per code | The vectors are reviewed before any verifier code exists, so the tests are not written to fit the code. |
| 1. Verifier crate | `cedar-evidence` library and `cedar-verify` for gates 0 to 2, `--json`, `--keys`, `--allow` | Every vector behaves as the manifest says; a mutation pass (flip each byte class of an accept vector) finds no accepted mutant. |
| 2. Sealing | Migration, sealed append, start and end records, statement issued at run end, `GET /v1/runs/{id}/evidence` | An integration test runs a real run, exports, verifies; then edits one `events` row in SQLite and sees the export fail verification. |
| 3. Artifacts and claims | Tar bundle, `--artifact-root`, `--claim`, the three-valued verdicts | The four-arm demo holds: intact verifies, changed byte is contradicted, missing artifact is not established, a different verifier build agrees. |
| 4. Clients | "Verified record" in the chat UI and desktop app, with in-browser verification from a wasm build of the same crate | A user can verify a run without a terminal. |
| 5. Outside witness | The chat server or a second cedar co-signs each statement's root; optionally an RFC 3161 timestamp or a transparency log entry; consumer policy can require N of M keys | A consumer who does not trust the operator's key still gets `attested` for "this record existed, unchanged, at time T". |

Phases 0 to 2 are the useful minimum, roughly the size of the records feature. Phase 5 is what
moves cedar's records from "the runtime's own account" to something the reference's
self-reported-record corpus would not refuse.

## 8. What a clean verdict will and will not show

| Shows | Does not show |
| --- | --- |
| The record is the one the pinned key signed, complete, unedited, in order | That the key belongs to who you think. That is the key set's provenance. |
| Which tools ran, with which inputs and outputs (by digest), and which were approved, denied or blocked | That the model server ran the model it named, or that the answer is correct |
| The sandbox image and policy digests the run started under | That the operator did not alter cedar itself. Until phase 5 the operator can sign anything. |
| The workspace files at the end hash to what the record says | That this is the only run, or the latest one. Offline means no freshness check. |

## 9. Open questions

1. **Predicate home.** A cedar-owned type URI, or propose it to the reference's predicate
   registry so its corpus list and runner carry it? The second gets independent verifiers
   sooner and costs control over the format.
2. **Existing runs.** Runs from before sealing have no signatures. Export them as all-`declared`
   statements (`NOT_RECONSTRUCTIBLE`), or refuse to export? The plan assumes refuse.
3. **Privacy.** A record names tool inputs by digest, but short inputs are guessable from a
   digest. Salt each digest with a per-run secret disclosed only in the artifact bundle?
4. **Deletion.** `DELETE /v1/runs/{id}` removes the events. Keeping the statement's root (and
   nothing else) would let a holder of an exported record still prove it was genuine.
5. **Continuations.** A run with a `parent_id` should bind its parent's batch root in its start
   record, so a conversation verifies as a chain. Left for after phase 2.
6. **Key rotation.** A key set with validity intervals in the consumer policy, checked against
   record times, is enough offline; revocation needs phase 5's log.
