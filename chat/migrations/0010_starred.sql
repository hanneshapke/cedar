-- Starred conversations: when the user starred one, from any client (the web, the desktop app
-- or the iOS app), or NULL. Scheduled runs' conversations can be starred too, so the Starred list
-- is the one place both kinds show up together.
ALTER TABLE conversations ADD COLUMN starred_at INTEGER; -- unix milliseconds

CREATE INDEX conversations_starred ON conversations (starred_at) WHERE starred_at IS NOT NULL;
