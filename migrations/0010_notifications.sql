-- Notifying clients when a run ends, without them holding an event stream open.

-- Where to POST when the run reaches a terminal state, and the secret its deliveries are signed
-- with (see `webhooks`). Both are the client's; the secret is never returned.
ALTER TABLE runs ADD COLUMN callback_url TEXT;
ALTER TABLE runs ADD COLUMN callback_secret TEXT;

-- One row per finished run, written in the same transaction as its terminal event. It feeds two
-- things: the per-key stream `GET /v1/events` (every row of the owner, in `id` order, so a client
-- resumes from the last id it saw) and the webhook worker (rows with a `callback_url`, until
-- `delivered_at` is set or the attempts run out, in which case `next_attempt_at` is cleared and
-- `last_error` says why).
CREATE TABLE run_notifications (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id          TEXT NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    owner           TEXT NOT NULL,
    seq             INTEGER NOT NULL,          -- the terminal event's seq
    callback_url    TEXT,                      -- NULL: nothing to deliver
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER,                   -- unix milliseconds; NULL once delivered or given up
    delivered_at    INTEGER,
    last_error      TEXT,
    created_at      INTEGER NOT NULL,
    UNIQUE (run_id, seq)
);

CREATE INDEX run_notifications_owner ON run_notifications (owner, id);
CREATE INDEX run_notifications_due ON run_notifications (next_attempt_at) WHERE next_attempt_at IS NOT NULL;
