-- Private conversations: for something the person wants neither remembered nor kept. One is
-- never in the conversation list, the Starred list, a search or the inbox, its runs are cedar's
-- private runs (no memory, no records, no trace), nothing about it is sent out of the app, and it
-- is deleted when the person leaves it or, failing that, once it has been idle for
-- `CHAT_PRIVATE_TTL_SECS`. A conversation is private from its start or not at all.
ALTER TABLE conversations ADD COLUMN private INTEGER NOT NULL DEFAULT 0;

CREATE INDEX conversations_private ON conversations (updated_at) WHERE private = 1;
