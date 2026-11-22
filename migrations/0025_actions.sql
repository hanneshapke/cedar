-- Actions: how to do something, as the person behind an API key showed it in a screen recording
-- (see `actions`). A model that takes video (or images) turns the recording into steps; the
-- person reads and corrects them and saves them here, and later runs read them back with the
-- `actions_*` tools. A password that is typed in the recording can be kept with the action: its
-- value lives in the encrypted `secrets` table, and this row only names it.
CREATE TABLE actions (
    id           TEXT PRIMARY KEY NOT NULL, -- UUIDv7, so id order is creation order
    owner        TEXT NOT NULL,
    name         TEXT NOT NULL,             -- a lowercase identifier the tools and the browser's `secret` use
    title        TEXT NOT NULL,
    summary      TEXT NOT NULL,             -- when to use it: what it gets done
    site         TEXT,                      -- the host it happens on, when it is a website
    steps        TEXT NOT NULL,             -- JSON array of the steps, one string each
    secrets      TEXT NOT NULL,             -- JSON array of {name, label}: what is kept in `secrets` for it
    source       TEXT NOT NULL,             -- JSON: where the steps came from (a recording's length, frames, model)
    uses         INTEGER NOT NULL DEFAULT 0, -- how often a run read it
    last_used_at INTEGER,                   -- unix milliseconds
    created_at   INTEGER NOT NULL,          -- unix milliseconds
    updated_at   INTEGER NOT NULL,          -- unix milliseconds
    UNIQUE (owner, name)
);

-- Whether a registered model accepts a video with a prompt (a `video_url` content part, as vLLM
-- and OpenRouter take it). A recording is sent whole to such a model; any other model that takes
-- images is shown the recording's key frames instead.
ALTER TABLE models ADD COLUMN video INTEGER NOT NULL DEFAULT 0;
