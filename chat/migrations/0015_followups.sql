-- What the user decided about a follow-up the model suggested (cedar's `suggest_follow_up` tool):
-- accepted, which made a one-off agent, or dismissed. A suggestion is one tool call of a turn's
-- run, so it is named by the turn and the call's id; one with no row here is still open.
CREATE TABLE followups (
    turn_id     TEXT NOT NULL REFERENCES turns (id) ON DELETE CASCADE,
    call_id     TEXT NOT NULL,
    status      TEXT NOT NULL,   -- 'accepted' | 'dismissed'
    -- The agent an accepted suggestion became; NULL once that agent is deleted.
    schedule_id TEXT REFERENCES schedules (id) ON DELETE SET NULL,
    decided_at  INTEGER NOT NULL, -- unix milliseconds
    PRIMARY KEY (turn_id, call_id)
) WITHOUT ROWID;
