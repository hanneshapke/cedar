-- Private runs: a conversation the user asked to be kept out of everything that outlives it.
-- Such a run reads and writes no memory and no records, keeps no trace of its model calls
-- (`llm_calls`, `sent_system`, `sent_tools`) and calls no webhook. Every run that continues it
-- is private too.
ALTER TABLE runs ADD COLUMN private INTEGER NOT NULL DEFAULT 0;
