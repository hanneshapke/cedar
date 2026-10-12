-- Scheduled agents: a prompt (compacted from a conversation) that runs on its own at a local
-- time of day, daily, on weekdays, or weekly. Every firing is a cedar run in a conversation of
-- its own, kept out of the normal conversation list and reached through the schedule.

CREATE TABLE schedules (
    id                     TEXT PRIMARY KEY NOT NULL,
    title                  TEXT NOT NULL,
    prompt                 TEXT NOT NULL,
    model                  TEXT,
    source_conversation_id TEXT,              -- the chat it was made from; informational only
    frequency              TEXT NOT NULL,     -- 'daily' | 'weekdays' | 'weekly'
    weekday                INTEGER,           -- 0 = Monday .. 6 = Sunday; weekly only
    time_of_day            TEXT NOT NULL,     -- 'HH:MM', local to `timezone`
    timezone               TEXT NOT NULL,     -- IANA name, e.g. Europe/Berlin
    enabled                INTEGER NOT NULL DEFAULT 1,
    next_run_at            INTEGER NOT NULL,  -- unix milliseconds
    last_run_at            INTEGER,
    created_at             INTEGER NOT NULL,
    updated_at             INTEGER NOT NULL
);

CREATE INDEX schedules_due ON schedules (enabled, next_run_at);

-- One firing of a schedule. The conversation and its single turn hold the cedar run; both are
-- NULL when the run could not be started (error says why).
CREATE TABLE schedule_runs (
    id              TEXT PRIMARY KEY NOT NULL,
    schedule_id     TEXT NOT NULL REFERENCES schedules (id) ON DELETE CASCADE,
    conversation_id TEXT REFERENCES conversations (id) ON DELETE SET NULL,
    turn_id         TEXT,
    due_at          INTEGER NOT NULL,
    started_at      INTEGER NOT NULL,
    manual          INTEGER NOT NULL DEFAULT 0, -- 1 for "run now"
    error           TEXT
);

CREATE INDEX schedule_runs_schedule ON schedule_runs (schedule_id, started_at);

-- Conversations created by a schedule belong to it and go when it goes.
ALTER TABLE conversations ADD COLUMN schedule_id TEXT REFERENCES schedules (id) ON DELETE CASCADE;
