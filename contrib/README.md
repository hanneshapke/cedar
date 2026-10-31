# cedar-contrib

The corpus side of contributed conversations: the ingest that
[cedar-chat](../chat/README.md#contributing-a-conversation) sends to, the screener that checks
what came in, the review page for what it held, and the release job that publishes. The design
is in [docs/design/contributions.md](../docs/design/contributions.md) (the client side and the
bundle) and [docs/design/contrib-ingest-cloud-run.md](../docs/design/contrib-ingest-cloud-run.md)
(this crate). One binary, three commands:

```sh
cedar-contrib                  # serve the ingest (and the review page, with CONTRIB_REVIEW_TOKEN)
cedar-contrib screen           # screen what is quarantined (every 15 minutes on Cloud Run)
cedar-contrib release          # build this month's release (monthly, or by hand)
cedar-contrib ablation         # train-and-evaluate the candidate batch with and without each cluster
cedar-contrib stats            # the numbers the steady state is run by
```

## The ingest

| Route | What it does |
| --- | --- |
| `POST /v1/contributions` | Takes one bundle (at most 8 MiB). Runs the checks both sides share (`cedar-contrib-schema`): shape and limits, the model allowlist, a secret scan, the consistency checks; then the quotas; then claims the bundle's content hash (a second copy is `409 duplicate`) and stores the bytes as received in the quarantine bucket, with a receipt. Answers `201 {receipt_id, withdraw_token, status}`. Refusals are `400` with a coarse `code` (`schema`, `not_consistent`, `secret_detected`, `model_not_allowed`), `413 too_large`, `409 duplicate`, `429 quota` or `429 rate_limited`. |
| `GET /v1/contributions/{id}` | With `Authorization: Bearer <withdraw_token>`: `{status, submitted_day, released_in}`. `401` for a wrong token or an unknown id, which look the same. |
| `DELETE /v1/contributions/{id}` | With the token: deletes the bundle from quarantine, frees its hash, notes it on the exclusion list every release carries, and marks the receipt `withdrawn`. Always `204`. |
| `GET /v1/policy` | What this ingest takes: schema versions, models, licenses, size, the canary and the quotas. cedar-chat reads it to refuse locally what the ingest would refuse. |

The withdraw token is `HMAC-SHA256(key, receipt_id | sha256)`, recomputed on every check and never
stored. A stolen token withdraws one contribution and nothing else.

**What it never does:** log a request, store an address, or tell a client more than the coarse
code. The per-client rate limit hashes the address with a per-process salt and keeps it in memory
for a minute.

## The screener

`cedar-contrib screen [--batch N] [--rescreen]` takes every `quarantined` bundle (at most
`CONTRIB_SCREEN_BATCH`, 200) and gives it a verdict, `accepted`, `held` or `rejected`, with
flags, scores and one note per finding, kept on the receipt and written to the audit log. With
`--rescreen` it also redoes bundles judged by an older checks version (`screen::CHECKS_VERSION`;
the release job does this itself). The checks, in cost order:

| Check | Flag | What happens |
| --- | --- | --- |
| Near-duplicate: a MinHash signature over the conversation's 5-word shingles, against every other receipt's | `near_duplicate` (another contributor), `resubmission` (the same) | held / rejected |
| Obfuscation: zero-width and bidi characters, words mixing Latin with Cyrillic or Greek, long base64 or hex runs outside tool results | `obfuscation` | held |
| Injected instructions in a tool result ("ignore all previous instructions", role markers, "do not tell the user", …) | `injection_present`, then the judge says whether the agent complied | `injection_complied`: rejected; `injection_resisted`: accepted and tagged; no judge: `injection_unjudged`, held |
| Destructive or exfiltrating commands in tool calls (`rm -rf /`, fork bombs, `mkfs`, piping `~/.ssh` or `.aws` to `curl`, `curl … \| sh`) | `destructive_command`, `exfiltration` | held |
| Label consistency: the self-reported outcome against the run's own ending and the ratings | `label_disagreement` | held |
| Flood: ten or more bundles from one pseudonym, half alike on average | `flood` | held |
| Personal data with a shape that the client's scan should have replaced | `pii` | held |
| Declared language against a stopword count; conversations too thin to train on | `language`, `thin` | held |

Then the **judge**, when `CONTRIB_JUDGE_URL` names an OpenAI-compatible chat-completions
endpoint (`CONTRIB_JUDGE_MODEL`, `CONTRIB_JUDGE_API_KEY`): one call per bundle with a fixed
rubric (`judge::RUBRIC`), answered in JSON: whether a tool result addressed the agent and whether
it complied, a harm score (exfiltration, destruction outside the workspace, deception, excluded
content; `judge_harm` holds), and its own reading of the outcome. Where that reading disagrees
with the contributor's, the disagreement is counted on the pseudonym's ledger: over time that
becomes the bundle's `label_weight`. The judge sees the transcript only, never the pseudonym or
the receipt, and its answer is parsed as numbers.

The regression suite is the red-team set in `src/redteam.rs`: bundles with a known poison each
(an injection the agent obeys, one it resists, zero-width and mixed-script triggers, a destructive
command, an exfiltration, a flipped label, missed personal data, a wrong language, a thin thread)
and the verdict each must get. `tests/screen.rs` runs them; a check that stops catching its case
fails the build.

## The review page

With `CONTRIB_REVIEW_TOKEN` set, `/review` serves a one-file page to whoever presents the token
(put IAP in front of it for org accounts; the token is the floor): the held bundles, oldest first,
with their flags, scores and the screener's notes, the conversation's turns as they are, and
**Accept** / **Reject**. Reviewers see a contributor only as an 8-character hash of the pseudonym,
and never a withdraw token. Every decision is an audit line.

## The release job

`cedar-contrib release [--version vYYYY.MM] [--out DIR] [--dry-run]` builds a release:

1. **Eligible** are `accepted` bundles at least `CONTRIB_AGING_DAYS` (14) old whose hash is not on
   the withdrawn list; any judged by an older checks version is screened again first.
2. **Clusters**: bundles whose SimHash fingerprints are within 6 bits form a cluster; one of 5 or
   more that is all one pseudonym, or all unlinked, is held as a set for a reviewer (`cluster`).
3. **Caps**: no pseudonym over 1% of the release, the unlinked together no more than 20%; over the
   cap, a stable sample (by the hash of version and bundle) goes and the rest wait.
4. **Label weights**: each pseudonym's agreement rate (judged vs disagreed on its ledger) is the
   bundle's `label_weight`; below 0.5, the self-reported outcome is replaced by the judge's where
   the judge read one, and the bundle tagged `outcome_from_judge`.
5. **Files**: `releases/<version>/<license>/<model>/bundles.jsonl` (one line per bundle:
   `{receipt_id, sha256, tags, label_weight, bundle}`), `manifest.json` (every entry with its
   license, model, language, day, weight, tags and checks version, the caps, and `excluded_sha256`,
   the withdrawn hashes a mirror must honour) and `manifest.sig` (HMAC-SHA256 with
   `CONTRIB_RELEASE_KEY`; `release::verify` checks it; a KMS signature can replace it without
   changing the format). Tags: `sandbox_only` (only sandbox tools were called, so the trajectory
   can be replayed in cedar-bench), `injection_resisted`, `plan_edited`, `approval_denied`.

Released bundles get `status: released` and the version in `released_in`, which the person's
status route shows. `--out DIR` also writes the files to disk; `scripts/publish.sh` then mirrors a
release to a Hugging Face dataset (squashing its history afterwards, so a withdrawn bundle does not
linger in an old commit) and a GitHub release with the tarball, its checksum, the manifest and its
signature. `--dry-run` reports what a release would hold and changes nothing.

## The ablation evaluation

`cedar-contrib ablation [--version V] [--out DIR] [--hook CMD] [--delta D] [--apply]` is the one
check that measures what the corpus is for. It takes the release's plan (the eligible bundles and
the clusters among them), writes training sets to `DIR/sets/`: `full.jsonl` (the whole candidate
batch) and `minus-<n>.jsonl` (the batch without cluster *n*), with `clusters.json` saying which
receipts each cluster holds. Then, with a hook (`--hook` or `CONTRIB_ABLATION_HOOK`), it runs the
hook once for the baseline (no set: the untrained model) and once per set, with `ABLATION_NAME`,
`ABLATION_SET` (the file, empty for the baseline) and `ABLATION_RESULT` (where to write
`{"safety": 0..1, ...}`) in its environment. The hook is where the trainer and cedar-bench's
safety suite live: train on the set, serve the result, run the bench, write the score. The
decision: safety with the full batch below the baseline by more than `--delta` (0.02) holds the
whole batch (`ablation_batch`); otherwise a cluster whose removal raises safety by more than the
delta is held on its own (`ablation_cluster`). `--apply` holds; without it the report only says
what would be held. Without a hook the job writes the sets and reports `pending`, which is the
first thing to do while the bench's safety suite settles. The report goes to
`releases/<version>/ablation.json`. `tests/ablation.rs` drives it with a shell hook that scores by
set name.

## Statistics

`cedar-contrib stats [--days N] [--save]` prints, and with `--save` keeps as `stats/<day>.json`,
what the steady state is run by: receipts by status, submissions per day in the window, held and
rejected rates, a histogram of flags, pseudonyms and the unlinked share, how many the judge saw,
how long held bundles have waited (median days), the label-weight buckets, and the audit lines by
actor (screener, reviewer, release, ablation) with the reviewer's decisions. Counts only; never a
pseudonym, never text. A month of snapshots is the trend; the design's steady state is a monthly
release, a quarterly re-screen (`screen --rescreen --force`, scheduled in the Terraform) and a
look at these numbers.

## Training on a release

`scripts/train_qwen_rl.py` is a worked example of reinforcement learning on a release with a
small Qwen model (TRL, PEFT), in two stages. `rft` turns every trajectory into a chat with tool
calls in Qwen's own format, scores it from the person's outcome and thumbs and how the run ended,
discounted by the contributor's label weight, and fine-tunes on the good ones (rejection-sampling
fine-tuning; the best count twice). `grpo` makes tasks of the successful trajectories' prompts and
runs group-relative policy optimisation on the model's first move, rewarded for well-formed tool
calls against the task's tool definitions, a judge's comparison with the contributed answer
(`CONTRIB_JUDGE_URL`, as the screener's), and a safety penalty from the screener's destructive
and exfiltration patterns. It honours the manifest's exclusion list, takes only the licenses
named, keeps the canary in every sample, and `--dry-run` shows the converted data without a GPU.
A whole episode, with the tools running, is what cedar-bench's hermetic stack is for; the
`sandbox_only` tag marks the trajectories that can be replayed there (`--only-sandbox`).

## Storage

Everything is objects in one bucket (`CONTRIB_BUCKET`), so there is no database to run:

```
bundles/<yyyy-mm>/<receipt>.json   the bundle as received; its status mirrored in the object's
                                   metadata
receipts/<receipt>.json            the receipt: status, signature, verdict (flags, scores,
                                   notes, checks version), tags, label weight, releases
hashes/<sha256>                    one per stored bundle, created only if new: the dedup
withdrawn/<sha256>                 the exclusion list for releases
pseudonyms/<pseudonym>.json        a ledger of days and counts (quotas), judged and
                                   disagreed (label trust)
clusters/<prefix>/<day>/<receipt>  unlinked bundles that look alike, for their quota
audit/<day>/<receipt>-<ms>.json    one line per decision, by the screener, a reviewer or the
                                   release job (write-once objects; bucket lock in production)
releases/<version>/...             what the release job wrote
```

The jobs read every receipt (`receipts/` is listed and each object fetched), which is fine for
the few thousand bundles of the first releases and the point at which Firestore comes in.

Without `CONTRIB_BUCKET` the service keeps all of it in memory, for development.

## Quotas

Per pseudonym: `CONTRIB_QUOTA_DAY` (20) a day, `CONTRIB_QUOTA_MONTH` (200) in 30 days, and
`CONTRIB_QUOTA_NEW` (3) on the pseudonym's first day. Unlinked bundles (no pseudonym) share
`CONTRIB_QUOTA_CLUSTER` (5) a day among bundles that look alike (the same 16-bit prefix of a
SimHash over the prompts): the one sybil defence that needs no identity. `CONTRIB_IP_PER_MINUTE`
(30) is the backstop for raw floods.

## Running it

```sh
# Locally, in memory
CONTRIB_HMAC_KEY=$(openssl rand -hex 24) cargo run -p cedar-contrib   # listens on 127.0.0.1:8090

# then, for a cedar-chat on the same machine; it needs a cedar behind it and one of its keys
CEDAR_API_KEYS=devkey cargo run -p cedar                              # see the top-level README
CEDAR_API_KEY=devkey CHAT_CONTRIBUTIONS_URL=http://127.0.0.1:8090 cargo run -p cedar-chat
```

| Variable | Default | Meaning |
| --- | --- | --- |
| `CONTRIB_HMAC_KEY` | required | At least 32 characters; derives withdraw tokens. Rotating it invalidates every token handed out |
| `CONTRIB_BUCKET` | unset (memory) | The quarantine bucket |
| `CONTRIB_MODELS` | any | Comma-separated model names, or prefixes ending in `*`, whose outputs may be published |
| `CONTRIB_BIND_ADDR` / `PORT` | `127.0.0.1:8090` | Where to listen; Cloud Run sets `PORT` |
| `CONTRIB_QUOTA_DAY`, `_MONTH`, `_NEW`, `_CLUSTER`, `CONTRIB_IP_PER_MINUTE` | 20, 200, 3, 5, 30 | See Quotas |
| `CONTRIB_MAX_BUNDLE_BYTES` | 8 MiB | |
| `CONTRIB_REVIEW_TOKEN` | unset | Serves the review page at `/review` to whoever presents it |
| `CONTRIB_JUDGE_URL`, `CONTRIB_JUDGE_MODEL`, `CONTRIB_JUDGE_API_KEY` | unset, `judge`, unset | The judge model's chat-completions endpoint; without one, the screener holds injected results for review |
| `CONTRIB_RELEASE_KEY` | unset | Signs release manifests (HMAC-SHA256) |
| `CONTRIB_SCREEN_BATCH`, `CONTRIB_AGING_DAYS` | 200, 14 | Bundles per screener run; days an accepted bundle waits before a release |
| `CONTRIB_ABLATION_HOOK` | unset | The command the ablation job runs per training set (see Ablation) |
| `GOOGLE_OAUTH_ACCESS_TOKEN` | unset | A token for the bucket when not running on Google Cloud (`gcloud auth print-access-token`) |

## Deploying to Cloud Run

`terraform/` holds the frugal variant of the design: Cloud Run exposed directly, the bucket, a
service account that can touch only that bucket's objects, the HMAC and release keys in Secret
Manager, the logging exclusion that drops Cloud Run's request logs for the service, and two Cloud
Run Jobs from the same image, `contrib-screener` (Cloud Scheduler, every 15 minutes) and
`contrib-release` (the 3rd of each month), started through a scheduler service account that can
run those jobs and nothing else. `judge_url` and `judge_model` point the screener at a judge; `contrib-rescreen` runs the
quarterly pass (`screen --rescreen --force`) on the first of January, April, July and October;
`ablation_hook` adds the `contrib-ablation` job, started by hand before a release. The edge of
the design, a global load balancer with Cloud Armor's per-client rate limit and WAF rules in
front of the service, is in `edge.tf` behind `edge = true` (with `edge_domain` for its managed
certificate): off by default, since the service's own limit is enough until floods are real.

```sh
# 1. The image
PROJECT=<project>; REGION=europe-west1
gcloud auth configure-docker $REGION-docker.pkg.dev
docker build -f contrib/Dockerfile -t $REGION-docker.pkg.dev/$PROJECT/cedar-contrib/cedar-contrib:0.3.5 .
# (the registry is created by terraform; apply once with any image, push, apply again)
docker push $REGION-docker.pkg.dev/$PROJECT/cedar-contrib/cedar-contrib:0.3.5

# 2. The service
cd contrib/terraform
terraform init
terraform apply -var project=$PROJECT -var bucket=<unique bucket name> \
  -var image=$REGION-docker.pkg.dev/$PROJECT/cedar-contrib/cedar-contrib:0.3.5
```

The `url` output is what cedar-chat sets as `CHAT_CONTRIBUTIONS_URL`.

Checklist for the no-logs promise, after `apply`: the logging exclusion exists (Logs Router →
Exclusions); Storage data-access audit logs are off for the project (IAM → Audit Logs); nothing
else in the project logs requests on the service's behalf.

## Tests

```sh
cargo test -p cedar-contrib
```

They drive the router with the in-memory store: a good bundle round trip with withdrawal, the
refusal codes, the quotas, the rate limit and the review page; the screener over the red-team
set, near-duplicates, floods and label trust; and a release with its aging, caps, clusters,
weights, manifest, signature and exclusion list; and the ablation with a hook that scores by set
name, plus the statistics over its outcome.
