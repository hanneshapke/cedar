CREATE TABLE conversations (
    id         TEXT PRIMARY KEY NOT NULL,
    title      TEXT,             -- NULL until the first message (or a rename) names it
    model      TEXT,             -- model used most recently, so the picker can restore it
    created_at INTEGER NOT NULL, -- unix milliseconds
    updated_at INTEGER NOT NULL
);

CREATE INDEX conversations_updated ON conversations (updated_at);

-- One user message and the cedar run that answers it.
CREATE TABLE turns (
    id              TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    seq             INTEGER NOT NULL,
    run_id          TEXT NOT NULL,
    prompt          TEXT NOT NULL, -- what the user typed (the run's prompt also carries history)
    status          TEXT NOT NULL, -- cedar run status, refreshed until terminal
    final_text      TEXT,
    error           TEXT,
    created_at      INTEGER NOT NULL,
    UNIQUE (conversation_id, seq)
);
