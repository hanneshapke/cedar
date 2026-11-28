-- What a wait is about, beyond its reason: a plan's steps (`[{id, title}]`), so the timeline can
-- show the plan and take the go-ahead without opening the conversation.
ALTER TABLE waits ADD COLUMN detail TEXT;

-- Where a response still going is: how many tool calls its run has started, and the latest one's
-- name. `attention` writes them as it follows the run; the timeline shows them under now.
ALTER TABLE turns ADD COLUMN progress_steps INTEGER NOT NULL DEFAULT 0;
ALTER TABLE turns ADD COLUMN progress_doing TEXT;
