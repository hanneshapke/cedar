-- Handing work over between a run and its user.

-- How often an owner plainly allowed a kind of call for one thing (`scope`: a host, a package
-- name, ...), since they last declined it. An approval offers "always allow" only once every
-- scope it covers was allowed `ALWAYS_ALLOW_AFTER` times; declining starts the count again.
CREATE TABLE approval_counts (
    owner      TEXT NOT NULL,
    rule       TEXT NOT NULL,
    scope      TEXT NOT NULL,
    allows     INTEGER NOT NULL,
    updated_at INTEGER NOT NULL, -- unix milliseconds
    PRIMARY KEY (owner, rule, scope)
) WITHOUT ROWID;

-- Whether what the run would send or change in the user's accounts is held for them to approve
-- instead of done, when nobody answers while it goes (see `held_calls`).
ALTER TABLE runs ADD COLUMN hold_writes INTEGER NOT NULL DEFAULT 0;

-- The outbox: tool calls a run wanted to make that wait for the owner's decision. Each is the
-- call exactly as the model wrote it (`tool`, `input`); approving it makes that call, or the one
-- the owner edited it into (`input` is then what was sent, `edited` set), outside any run.
CREATE TABLE held_calls (
    id         TEXT PRIMARY KEY,
    run_id     TEXT NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    owner      TEXT NOT NULL,
    call_id    TEXT NOT NULL,            -- the tool call's id in the run
    tool       TEXT NOT NULL,
    input      TEXT NOT NULL,            -- JSON
    reason     TEXT NOT NULL,            -- one line on what the call does
    why        TEXT NOT NULL,            -- 'unattended' | 'timeout'
    status     TEXT NOT NULL,            -- 'pending' | 'sending' | 'sent' | 'failed' | 'discarded'
    edited     INTEGER NOT NULL DEFAULT 0,
    output     TEXT,                     -- what the tool returned, or why it failed
    created_at INTEGER NOT NULL,         -- unix milliseconds
    decided_at INTEGER
);

CREATE INDEX held_calls_owner ON held_calls (owner, status, created_at);
CREATE INDEX held_calls_run ON held_calls (run_id);
