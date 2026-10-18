-- The most tokens one reply of a registered model may take, a reasoning model's thinking
-- included. A reply that reaches it ends with `stop_reason` `max_tokens`. Replaces the server-wide
-- MAX_TOKENS setting; entries registered so far get the new default.
ALTER TABLE models ADD COLUMN max_tokens INTEGER NOT NULL DEFAULT 16384;
