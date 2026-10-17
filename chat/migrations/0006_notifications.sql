-- Being told when a response finishes while nobody is looking.

-- How the run ended (`ask_user` means it is waiting for an answer), when this service recorded
-- that, and whether the conversation has been looked at since. A turn finished but not seen is
-- what the page's inbox and unread marks show. Turns from before this were looked at long ago.
ALTER TABLE turns ADD COLUMN stop_reason TEXT;
ALTER TABLE turns ADD COLUMN finished_at INTEGER; -- unix milliseconds
ALTER TABLE turns ADD COLUMN seen INTEGER NOT NULL DEFAULT 0;
UPDATE turns SET seen = 1;

-- cedar's notifications name the run; the inbox lists what is unseen, newest first.
CREATE INDEX turns_run ON turns (run_id);
CREATE INDEX turns_unseen ON turns (seen, finished_at);
