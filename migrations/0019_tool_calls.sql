-- Tools called outside a run (`POST /v1/tools/{name}/call`): who called which, when, how long it
-- took and whether it failed. A run's calls are kept with the run; these have no run, so this is
-- their only trace. Rows older than a month are dropped as new ones come in.
CREATE TABLE tool_calls (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    owner       TEXT NOT NULL,
    tool        TEXT NOT NULL,
    started_at  INTEGER NOT NULL, -- unix milliseconds
    duration_ms INTEGER NOT NULL,
    error       TEXT
);

CREATE INDEX tool_calls_started ON tool_calls (started_at);
CREATE INDEX tool_calls_owner ON tool_calls (owner, id);
