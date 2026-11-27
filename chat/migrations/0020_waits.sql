-- What a turn's run is waiting on from the person: its plan to be agreed to, or a risky call to be
-- approved. `attention` follows the runs and writes these as cedar reports them, so the timeline
-- (`/api/timeline`) can list what needs the person after the page that saw the wait was closed,
-- or after a restart. A wait is resolved when it is answered or its run ends.
CREATE TABLE waits (
    turn_id     TEXT NOT NULL REFERENCES turns (id) ON DELETE CASCADE,
    key         TEXT NOT NULL,   -- 'plan', or 'approval:<id>'
    kind        TEXT NOT NULL,   -- 'plan' | 'approval'
    reason      TEXT,            -- an approval's reason, as cedar gave it
    created_at  INTEGER NOT NULL, -- unix milliseconds
    resolved_at INTEGER,
    PRIMARY KEY (turn_id, key)
) WITHOUT ROWID;

CREATE INDEX waits_open ON waits (turn_id) WHERE resolved_at IS NULL;

-- The timeline reads a day's turns by when they were sent.
CREATE INDEX turns_created ON turns (created_at);
