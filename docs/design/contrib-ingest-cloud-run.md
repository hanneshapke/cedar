# Design: the contribution ingest on Google Cloud Run, with poisoning defenses

_Status: proposal. Companion to [contributions.md](contributions.md), which defines the bundle, the
consent model and the client side. This document is the server: `cedar-contrib`, the service a
cedar-chat points `CHAT_CONTRIBUTIONS_URL` at._

The service has two jobs that pull against each other. It must accept bundles from people it
cannot identify, because the client side promises no accounts and no identity. And it must keep a
training corpus clean when anyone on the internet can submit to it. The resolution is to accept
cheaply, store nothing as "the dataset" at submission time, and make admission into a release a
separate, slow, multi-signal decision that no single submission, pseudonym or burst can force.

## 1. Threat model

What "poisoning" means here, and what each attacker wants:

| Attack | How it looks | What it does to the model |
| --- | --- | --- |
| **Volume / sybil** | Thousands of bundles from one actor, under many pseudonyms or none | Swamps the corpus; the attacker's style or goals dominate |
| **Backdoor trigger** | Normal-looking threads where a rare phrase or token pattern precedes a specific bad behaviour | The trained model learns "when you see X, do Y" |
| **Injected instructions** | Tool results or page bodies carrying "ignore your instructions and…", obfuscated or not | The model learns to obey injected text |
| **Reward hacking** | Bad trajectories labelled `succeeded`, good ones labelled `failed`, thumbs flipped | RL learns the wrong reward |
| **Harmful demonstrations** | Trajectories of the agent doing something harmful, presented as success | The model learns the behaviour |
| **Synthetic flooding** | Model-generated threads at scale, low variety, plausible labels | Collapse of diversity; style lock-in |
| **Duplicate / near-duplicate** | The same good thread many times | Over-weighting; memorisation |
| **Forged provenance** | A hand-written bundle claiming a model, usage and events it never had | Pollutes model-conditioned splits |
| **Poison by withdrawal** | Submit good data, wait for others to build on it, withdraw it; or withdraw others' with a stolen token | Mostly a nuisance; tokens must be unguessable |
| **PII laundering** | Someone else's private data submitted as "their" thread | Legal and ethical exposure of the corpus |

The service cannot know who a contributor is, so every defense is about **content** and
**behaviour over time**, never about identity.

## 2. Architecture

```
cedar-chat ──HTTPS──▶ Global external HTTPS LB ──▶ Cloud Run: contrib-api
                       (Cloud Armor: WAF, rate limits)      │
                                                             ├─ Firestore: receipts, pseudonym ledger, quotas
                                                             ├─ GCS: contrib-quarantine (private)
                                                             └─ Pub/Sub: contrib.submitted
                                                                       │
                                                       Cloud Run: contrib-screener (push subscription)
                                                             ├─ Vertex AI (judge model) or self-hosted vLLM
                                                             ├─ Firestore: verdicts, flags
                                                             └─ GCS: contrib-quarantine (status in metadata)

Cloud Scheduler ──▶ Cloud Run Job: contrib-release (monthly)
                        ├─ reads quarantine (aged ≥ 14 days, status accepted)
                        ├─ batch analysis: near-dup, clusters, per-contributor caps
                        ├─ ablation eval with cedar-bench (Cloud Run Job with GPU, optional)
                        └─ writes GCS: contrib-releases (public, versioned) → Hugging Face, GitHub Releases
```

Resources, by name:

- **`contrib-api`** (Cloud Run service, Rust/axum, the same stack as cedar). Public invoker
  (`allUsers`), min instances 1 so the first upload of the day does not wait, max instances small
  (10) so a flood costs the attacker effort and us little. Concurrency 80, CPU throttled outside
  requests. Memory 512 MiB. Request body cap 8 MiB at the LB and in the service.
- **`contrib-screener`** (Cloud Run service, private). Invoked by the Pub/Sub push subscription
  with an OIDC token. Runs the slow checks (§5). Max instances 3; it is allowed to lag.
- **`contrib-release`** (Cloud Run Job). Triggered by Cloud Scheduler monthly and by hand.
- **Global external Application Load Balancer** in front of `contrib-api`, which is what makes
  Cloud Armor available. The LB terminates TLS at a fixed hostname (`contrib.<project domain>`)
  that cedar-chat pins.
- **Cloud Armor policy**: per-client-IP rate limit (`rate_based_ban`, 30 requests/minute,
  ban 10 minutes), the preconfigured WAF rules for SQLi/XSS/LFI (cheap, and the body is JSON only),
  a geo rule only if law requires one. Cloud Armor counts IPs in memory for its windows; that is
  not a log.
- **Firestore** (native mode): `receipts`, `pseudonyms`, `verdicts`, `review_queue`. Small,
  serverless, no connection pool to manage.
- **GCS buckets**: `contrib-quarantine` (private, uniform access, object versioning off, so a
  withdrawal is a real delete; lifecycle: delete `status=rejected` objects after 30 days),
  `contrib-releases` (public read, versioning on, retention per object once published),
  `contrib-audit` (private, bucket lock 400 days: the moderation decision log, §7).
- **Secret Manager**: `withdraw-hmac-key`, `judge-api-key` if the judge is hosted. Mounted as env.
- **Service accounts**, one per service, least privilege: `contrib-api` can create objects in
  quarantine, publish to the topic, and read/write its Firestore collections. It cannot delete
  releases or read the audit bucket. `contrib-screener` reads quarantine and writes verdicts.
  `contrib-release` reads everything and writes releases and audit.
- **Cloud Build** from the `cedar-contrib` repo with Binary Authorization requiring its attestor,
  so only images built from the repo can run. The pipeline is public: the defense should survive
  the attacker reading it.

## 3. API

Four routes. Everything else is 404.

| Route | What it does |
| --- | --- |
| `POST /v1/contributions` | Body: one bundle (§10 of the client design). Runs the synchronous checks (§4). `201 {receipt_id, withdraw_token, status: "quarantined"}`. `400` with a coarse `reason` code for a bundle the person can fix (`schema`, `too_large`, `secret_detected` with the span's placeholder index only, `model_not_allowed`, `not_consistent`); `409 duplicate`; `429` from quotas. |
| `GET /v1/contributions/{receipt_id}` | With `Authorization: Bearer <withdraw_token>`: `{status, submitted_day, released_in: [versions]}`. `status` is `quarantined`, `accepted`, `held`, `rejected` or `withdrawn`. No reasons beyond that for `held` and `rejected` (§8). |
| `DELETE /v1/contributions/{receipt_id}` | With the token. Deletes the quarantine object, marks the receipt `withdrawn`, records the hash in the release job's exclusion list. `204` always, so the route does not reveal whether a receipt exists. |
| `GET /v1/policy` | The current rules in machine form: schema versions accepted, model allowlist, size caps, licenses, the canary prefix. cedar-chat fetches it to refuse locally what the server would refuse. No auth. |

Receipt ids are UUIDv4. The withdraw token is `base64url(HMAC-SHA256(key, receipt_id || content_sha256))`,
so it is never stored: the service recomputes it. A stolen token withdraws one contribution and
nothing else.

The `cedar-chat` version header is the only header the service reads, and only to refuse schema
versions it no longer accepts. User-Agent is not stored.

## 4. The request path: cheap, deterministic, fail closed

Everything in this section runs inside the `POST` and takes milliseconds. Its purpose is to make
a bundle that *could* be admitted and to refuse the obvious, without being an oracle for the rest.

1. **Shape.** Body ≤ 8 MiB, JSON, `schema_version` accepted, canary string present and well
   formed, license in the allowed set, language a two-letter tag. Turn count 1–50, messages per
   turn ≤ 400, any single string ≤ 256 KiB.
2. **Model allowlist.** `model.name` must be on `GET /v1/policy`'s list (models whose terms allow
   publishing their outputs). Anything else is `400 model_not_allowed`, and never stored.
3. **Internal consistency.** The checks that a hand-forged bundle usually fails and a real cedar
   bundle cannot:
   - every `tool_use` block has a `tool_result` with the same id in the next user message, and
     every `tool_result` refers to a prior `tool_use`;
   - every tool called is in the turn's `tool_definitions`;
   - `calls[i].message_count` is non-decreasing and ≤ the number of messages;
   - `usage.input_tokens` is within a wide band of the byte length of the messages sent to that
     call (a 3-message call cannot claim 900k tokens; a 200-message call cannot claim 50);
   - events form a legal sequence (`plan` before `plan.updated`, `approval.required` before its
     `resolved`, exactly one terminal event per turn, a `cancelled` turn has no later turns);
   - placeholders (`[EMAIL_n]`, `[KEY_n]`…) are numbered densely and match `redactions` counts.
   A failure is `400 not_consistent`. This does not stop a careful forger. It stops bulk forgery
   with a script that didn't bother, which is most of it.
4. **Secret scan**, the same patterns the client runs. A hit is `400 secret_detected` with the
   turn and message index, and the body is not stored, not even in rejected form. The client
   should have caught it; the server is the backstop for old clients and copied bundles.
5. **Exact duplicate.** `content_sha256` over the canonicalised bundle minus `contributor`,
   `outcome` and `canary`. Known hash: `409 duplicate`. The receipt of the first one is not
   revealed.
6. **Quotas** (Firestore counters, no IPs involved):
   - per pseudonym: 20 bundles per day, 200 per 30 days, and the pseudonym must be ≥ 24 hours old
     for more than 3 bundles (a new pseudonym can show good faith with 3, then wait);
   - unlinked (no pseudonym): accepted, but see §6's cap, and limited to 5 per day *per content
     fingerprint cluster* (a SimHash prefix), which is the only sybil defense that doesn't need
     identity: the same actor's threads look alike;
   - the Cloud Armor per-IP limit above is the backstop for raw floods and is deliberately loose.
7. **Store.** Write the object to `contrib-quarantine/<yyyy-mm>/<receipt_id>.json` with metadata
   `status=quarantined`, `sha256`, `pseudonym`, `submitted_day`. Create the receipt. Publish
   `{receipt_id}` to `contrib.submitted`. Return the receipt.

The order matters for privacy: nothing is written to disk until step 7, so a refused bundle
leaves no trace beyond a counter.

## 5. The screener: slow, probabilistic, asynchronous

`contrib-screener` runs within minutes of submission and writes a **verdict** per bundle:
`accepted`, `held` (a human looks) or `rejected`, with a score vector it keeps for the batch
analysis. It never deletes; the release job and lifecycle rules do that. Checks, roughly in cost
order:

1. **Near-duplicate.** MinHash over 5-gram shingles of the user prompts and assistant text,
   compared against the last 90 days of quarantine (an LSH index in Firestore, rebuilt nightly by
   the release job). Jaccard > 0.8 against a different pseudonym: `held`. Against the same
   pseudonym: `rejected` as a resubmission.
2. **Obfuscation scan.** Zero-width characters, bidi overrides, homoglyph mixes, long base64 or
   hex runs outside tool results, repeated rare token sequences across turns. These are what a
   trigger is made of when someone wants it invisible to a reviewer. Any hit: `held`, and the
   span is marked for the reviewer.
3. **Injection detector.** Tool results and stubbed page bodies are scanned, first with patterns
   ("ignore previous", "you are now", "system:", role markers in text), then with the judge
   model asked one question: does any tool result contain text addressed to the model rather than
   data? A thread whose *assistant turns then comply* with such text is `rejected`: that is the
   one trajectory that must never be trained on as a success. A thread where the agent ignored it
   is `accepted` and tagged `injection_resisted`, which is valuable training data.
4. **Harmful-behaviour judge.** The judge model reads the trajectory with a fixed rubric: does the
   agent exfiltrate data, run destructive commands outside its workspace, deceive the user, or
   produce content the corpus policy excludes? Any score above threshold: `held`, never auto-
   accepted. The rubric and prompts live in the repo; they are versioned with the verdict.
5. **Label consistency.** The judge estimates the outcome from the trajectory alone (did the
   agent do what the prompt asked?) and compares with the contributor's `outcome` and ratings.
   Disagreement is not a rejection, it is a **signal stored on the pseudonym**: a contributor whose
   self-labels disagree with the judge far more often than the median gets their labels discounted
   (§6) and their new bundles `held`. This is the reward-hacking defense: it works over time, not
   per bundle.
6. **Synthetic-flood detector.** Per batch and per pseudonym: distribution of prompt lengths,
   vocabulary size, repeated openings, the ratio of unique 4-grams. A pseudonym whose last 50
   bundles are far below the corpus's variety: `held`, with the pseudonym flagged.
7. **PII rescan.** A second NER/pattern pass independent of the client's; a likely name, address or
   organisation outside a placeholder: `held`. Held-for-PII bundles are redacted by the reviewer,
   never accepted as is.
8. **Language and length sanity.** Declared language matches detected; the thread is not just one
   prompt and "OK".

Verdicts are written to Firestore `verdicts/{receipt_id}` with every score, the check versions,
and the model and prompt hash used by the judge, so a verdict can be re-derived or audited later.
The object's metadata `status` is updated. A `held` bundle goes to `review_queue`.

**The judge model.** Either Vertex AI with a hosted model, or the same open-weight model the corpus
trains, served by vLLM on Cloud Run with a GPU and min instances 0 (it is only needed in bursts).
The second is slower and cheaper, and keeps bundles inside the project. Either way the judge never
sees the pseudonym, the receipt id or anything outside the bundle text, and its output is a JSON
score, not free text that could be injected back into the pipeline.

**Human review.** `held` bundles are reviewed in a minimal internal page (Cloud Run, IAP-protected,
org accounts only) that shows the bundle with the screener's marks, and three buttons: accept,
redact-and-accept, reject. Every decision is a line in the audit log (§7). Reviewers see the
pseudonym only as a hash, and never a receipt's withdraw token.

## 6. Release-time defenses: where poisoning is actually stopped

A single bundle that slips through §4 and §5 cannot poison a model. A *coordinated set* can. The
release job is where sets are seen.

1. **Aging.** Only bundles ≥ 14 days in quarantine with status `accepted` are eligible. This gives
   withdrawals time, gives the screener time to re-run with updated checks (the job re-screens
   everything eligible with the current check versions, so a detector improved last week applies
   to last month's submissions), and means an attacker's campaign is analysed as a whole.
2. **Per-contributor cap.** No pseudonym contributes more than 1% of a release, and all unlinked
   bundles together no more than 20%. Over the cap, a uniform random sample of that contributor's
   bundles is taken; the rest wait for a later release (and are counted against the cap again).
   This bounds what any one actor can do to the corpus without knowing who anyone is.
3. **Cluster analysis.** Embeddings of each bundle (prompt + final answer) clustered; a cluster
   that is one pseudonym, or many new pseudonyms created in the same week, or far denser than the
   corpus, is `held` as a group with its reason, for a reviewer to accept or drop as a set.
   Trigger phrases show up here as a cluster of otherwise unrelated threads sharing a rare n-gram
   that precedes similar assistant behaviour; the job lists the top shared rare n-grams per
   cluster.
4. **Label discount.** Each pseudonym's label-agreement rate (§5.5) becomes a `label_weight` in
   the released metadata, and bundles from pseudonyms below a floor ship with their self-labels
   replaced by the judge's and marked as such. Trainers can choose; the default pipeline uses the
   weight.
5. **Ablation evaluation** (optional, the expensive check). Fine-tune the reference open-weight
   model briefly on the previous release plus the candidate batch, and on the previous release
   plus the batch minus each flagged cluster, then run cedar-bench's safety and reliability
   tasks. A batch or cluster that moves safety scores down is held, whatever the screener said.
   This is the only check that measures the thing we actually care about, and it also catches
   poisons nobody designed a detector for. A Cloud Run Job with a GPU, started by the release job,
   results written beside the release.
6. **Manifest.** The release writes `manifest.json`: every bundle's receipt id, sha256, license,
   model, language, label weight, tags (`injection_resisted`, `sandbox_only`, …), and the versions
   of every check that admitted it. Plus the exclusion list of withdrawn hashes, so a downstream
   mirror can honour withdrawals. The manifest is signed with a KMS key; cedar-bench and anyone
   else can verify a release before training on it.
7. **Publish.** Tarball per license per source model to `contrib-releases/v<yyyy.mm>/`, mirrored
   to the Hugging Face dataset (then squash history) and attached to a GitHub release.

## 7. Audit log and reproducibility

Every admission decision, automatic or human, is one append-only JSON line in `contrib-audit`
(bucket lock, 400 days): receipt id, check versions, scores, verdict, who or what decided, and
the release it went into. The bucket is write-only for the services and readable by org admins.
If a poison is found after a release, the log answers which checks passed it, which pseudonym it
came from, what else that pseudonym and that cluster contributed, and which releases contain any
of it, so a targeted re-release is possible rather than a rebuild from scratch.

The log contains no IPs and no bundle text, only hashes and scores.

## 8. Don't be an oracle

An attacker who can submit and read verdicts can tune a poison until it passes. So:

- `400` reasons are coarse and only for faults the honest client can fix. `held` and `rejected`
  are reported as a status with no reason.
- Status is readable only with the withdraw token, so an attacker learns only about their own
  submissions, at their own quota's pace.
- Screener thresholds and judge prompts are public in the repo (the project is open), but the
  release-time cluster analysis, the ablation, the caps and the human review are not bypassable
  by tuning a single bundle: they act on sets and on time.
- Verdict changes are asynchronous and batched (the release job), so a bundle's status flips from
  `quarantined` to its final state at an unpredictable time, which denies a tight feedback loop.

## 9. The no-logs promise, concretely

Cloud Run and the load balancer log requests with client IPs by default. The policy says they
don't, so the configuration must make it true and reviewable:

- LB backend service: `logConfig.enable = false`.
- Cloud Logging: an **exclusion filter** on the project's `_Default` sink for
  `resource.type="cloud_run_revision" AND logName:"requests"` (drops Cloud Run request logs
  entirely), and a second one dropping `httpRequest.remoteIp` from anything else via a log
  router with a field exclusion. Application logs (`tracing`) never include headers or bodies;
  the only per-request line at `info` is the receipt id and the verdict.
- Cloud Armor rule logging off (it would log IPs of rate-limited clients).
- Pub/Sub, Firestore and GCS audit logs (`DATA_READ`/`DATA_WRITE`) stay **off**; admin activity
  logs stay on (they record our changes, not users' requests).
- Terraform for all of the above lives in the `cedar-contrib` repo, and PRIVACY.md links to the
  file, so "we don't log IPs" is a line someone can check.

Cloud Armor's rate limiting keeps IP counters in memory for its window and is documented as such.

## 10. Costs, roughly

With a few hundred bundles a day: `contrib-api` with one warm instance is in the tens of dollars a
month, the LB about the same, Firestore and GCS a few dollars, the screener's judge calls a few
dollars per thousand bundles on a hosted model (more on a self-hosted GPU instance that scales to
zero), and the monthly ablation run is the one real cost, a few GPU hours. The flood case is
bounded by max instances and the LB rate limit, not by the attacker.

## 11. What this does not stop, said plainly

- A patient, careful actor submitting a small number of high-quality, individually plausible
  poisoned threads under pseudonyms aged over months. The caps bound their share; the ablation may
  catch the effect; nothing guarantees it. That is true of every open-contribution corpus.
- Poisons whose effect is below the bench's resolution.
- A compromised judge model (an attacker who can influence the judge can influence verdicts),
  which is why verdicts are never the only gate and why the release job re-screens with the
  current judge.
- The legal exposure of data someone laundered through a contribution. The withdrawal route,
  the audit log and the exclusion list are the remedy, not a prevention.

## 12. Order of work

1. `contrib-api` with §3 and §4, Terraform for the project, buckets, Firestore, LB, Cloud Armor,
   logging exclusions. This alone is a usable private ingest.
2. `contrib-screener` with checks 1, 2, 4 (patterns), 7 and 8; the review page.
3. The judge checks (3, 4, 5) and the pseudonym ledger.
4. `contrib-release` with aging, caps, manifest signing and publication.
5. Cluster analysis, then the ablation job once cedar-bench has a stable safety suite.
