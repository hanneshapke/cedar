# cedar-contrib

The ingest that [cedar-chat](../chat/README.md#contributing-a-conversation) sends contributed
conversations to. The design is in [docs/design/contributions.md](../docs/design/contributions.md)
(the client side and the bundle) and
[docs/design/contrib-ingest-cloud-run.md](../docs/design/contrib-ingest-cloud-run.md) (this
service, the screener and the release job). This crate is the first of those three: the request
path. The screener and the release job are not built yet.

## What it does

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

## Storage

Everything is objects in one bucket (`CONTRIB_BUCKET`), so there is no database to run:

```
bundles/<yyyy-mm>/<receipt>.json   the bundle as received; status, sha256, pseudonym, day,
                                   fingerprint in its metadata (the screener and release job
                                   read and update these)
receipts/<receipt>.json            what the status route answers from
hashes/<sha256>                    one per stored bundle, created only if new: the dedup
withdrawn/<sha256>                 the exclusion list for releases
pseudonyms/<pseudonym>.json        a ledger of days and counts, for the quotas
clusters/<prefix>/<day>/<receipt>  unlinked bundles that look alike, for their quota
```

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
# then, for a cedar-chat on the same machine:
CHAT_CONTRIBUTIONS_URL=http://127.0.0.1:8090 cargo run -p cedar-chat
```

| Variable | Default | Meaning |
| --- | --- | --- |
| `CONTRIB_HMAC_KEY` | required | At least 32 characters; derives withdraw tokens. Rotating it invalidates every token handed out |
| `CONTRIB_BUCKET` | unset (memory) | The quarantine bucket |
| `CONTRIB_MODELS` | any | Comma-separated model names, or prefixes ending in `*`, whose outputs may be published |
| `CONTRIB_BIND_ADDR` / `PORT` | `127.0.0.1:8090` | Where to listen; Cloud Run sets `PORT` |
| `CONTRIB_QUOTA_DAY`, `_MONTH`, `_NEW`, `_CLUSTER`, `CONTRIB_IP_PER_MINUTE` | 20, 200, 3, 5, 30 | See Quotas |
| `CONTRIB_MAX_BUNDLE_BYTES` | 8 MiB | |
| `GOOGLE_OAUTH_ACCESS_TOKEN` | unset | A token for the bucket when not running on Google Cloud (`gcloud auth print-access-token`) |

## Deploying to Cloud Run

`terraform/` holds the frugal variant of the design: Cloud Run exposed directly, the bucket, a
service account that can touch only that bucket's objects, the HMAC key in Secret Manager, and
the logging exclusion that drops Cloud Run's request logs for the service. The load balancer and
Cloud Armor are not in it; add them when floods are a real problem.

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
refusal codes, the quotas and the rate limit.
