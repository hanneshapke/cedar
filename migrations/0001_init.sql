CREATE TABLE runs (
    id            TEXT PRIMARY KEY NOT NULL,
    owner         TEXT NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
    prompt        TEXT NOT NULL,
    system        TEXT NOT NULL,
    model         TEXT NOT NULL,
    max_turns     INTEGER NOT NULL,
    turns         INTEGER NOT NULL DEFAULT 0,
    final_text    TEXT,
    stop_reason   TEXT,
    error         TEXT,
    input_tokens  INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL, -- unix milliseconds
    started_at    INTEGER,
    finished_at   INTEGER
);

CREATE INDEX runs_owner_id ON runs (owner, id);
CREATE INDEX runs_status ON runs (status);

-- The Messages API conversation, kept verbatim (content blocks are stored as sent/received).
CREATE TABLE messages (
    run_id  TEXT NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    seq     INTEGER NOT NULL,
    role    TEXT NOT NULL,
    content TEXT NOT NULL, -- JSON
    PRIMARY KEY (run_id, seq)
) WITHOUT ROWID;

-- Append-only log that feeds the SSE stream. `seq` is per-run, starting at 1.
CREATE TABLE events (
    run_id     TEXT NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    type       TEXT NOT NULL,
    data       TEXT NOT NULL, -- JSON
    created_at INTEGER NOT NULL,
    PRIMARY KEY (run_id, seq)
) WITHOUT ROWID;
