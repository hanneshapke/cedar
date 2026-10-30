-- What a run sent to the model and got back, for the trace `GET /v1/runs/{id}/trace` shows. The
-- conversation itself is in `messages`: an agent call names how many of the run's messages it sent
-- rather than copying them, and the system prompt and tool definitions, the same for every call of
-- a run, are kept once on the run.
ALTER TABLE runs ADD COLUMN sent_system TEXT; -- the system prompt with the environment note, as sent
ALTER TABLE runs ADD COLUMN sent_tools TEXT;  -- JSON: the tool definitions, as sent

CREATE TABLE llm_calls (
    run_id      TEXT NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    turn        INTEGER NOT NULL, -- 0 for the planning call
    purpose     TEXT NOT NULL,    -- 'agent' or 'plan'
    request     TEXT NOT NULL,    -- JSON
    response    TEXT,             -- JSON; NULL when the call failed
    error       TEXT,
    started_at  INTEGER NOT NULL, -- unix milliseconds
    duration_ms INTEGER NOT NULL,
    PRIMARY KEY (run_id, seq)
) WITHOUT ROWID;
