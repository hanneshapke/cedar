-- Contributing conversations to a training corpus (see docs/design/contributions.md).

-- The person's thumbs on a response: 1 (good), -1 (bad), or NULL. Sent with a contribution as
-- the turn's rating; also just a note to themselves.
ALTER TABLE turns ADD COLUMN rating INTEGER;

-- One row per conversation sent to the corpus (`CHAT_CONTRIBUTIONS_URL`): the receipt the ingest
-- answered with. `conversation_id` has no foreign key on purpose: deleting the conversation keeps
-- the receipt, so the contribution can still be withdrawn. The withdraw token is the only thing
-- that can withdraw this one contribution and nothing else.
CREATE TABLE contributions (
    id              TEXT PRIMARY KEY NOT NULL, -- the ingest's receipt id
    conversation_id TEXT,
    title           TEXT,
    withdraw_token  TEXT NOT NULL,
    content_sha256  TEXT NOT NULL,
    license         TEXT NOT NULL,
    url             TEXT NOT NULL,             -- the ingest it went to
    submitted_at    INTEGER NOT NULL,          -- unix milliseconds
    withdrawn_at    INTEGER
);

CREATE INDEX contributions_submitted ON contributions (submitted_at);
