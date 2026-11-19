-- Handing work over between an agent and the person it works for.

-- Whether an agent's runs hold what they would send or change in the person's accounts (mail,
-- calendar, files, Slack) for them to approve, instead of doing it (cedar's `hold_writes`).
-- Agents made from now on hold by default; the ones that were here keep acting at once, as they
-- did when they were set up.
ALTER TABLE schedules ADD COLUMN hold_writes INTEGER NOT NULL DEFAULT 0;

-- What a person decided about something the model said it could not do (cedar's
-- `report_limitation` tool): sent as feedback, which keeps the report here for whoever runs the
-- server, or dismissed, which keeps only that it was seen. A report is one tool call of a turn's
-- run, so it is named by the turn and the call's id; one with no row here is still open. A sent
-- report outlives its conversation (`turn_id` is then NULL): it says what was missing, not what
-- was said.
CREATE TABLE feedback (
    id         TEXT PRIMARY KEY NOT NULL,
    user_id    TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    turn_id    TEXT REFERENCES turns (id) ON DELETE SET NULL,
    call_id    TEXT NOT NULL,
    status     TEXT NOT NULL,   -- 'sent' | 'dismissed'
    summary    TEXT,            -- NULL for a dismissed one: nothing of it is kept
    category   TEXT,
    needed     TEXT,
    comment    TEXT,            -- what the person added
    created_at INTEGER NOT NULL -- unix milliseconds
);

CREATE UNIQUE INDEX feedback_call ON feedback (turn_id, call_id) WHERE turn_id IS NOT NULL;
CREATE INDEX feedback_user ON feedback (user_id, status, created_at);
