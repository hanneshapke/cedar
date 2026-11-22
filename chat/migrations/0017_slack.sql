-- Talking to Cedar from Slack (see src/slack.rs).

-- Which person here a Slack account is. Made when the person enters, in Settings -> Slack, the
-- code the Slack app sent that account, so each side has proven it is who it says. A Slack
-- account is one person here; a person may link more than one (other workspaces).
CREATE TABLE slack_links (
    team       TEXT NOT NULL,              -- the Slack workspace's id (T...)
    slack_user TEXT NOT NULL,              -- the Slack account's id (U...)
    user_id    TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name       TEXT,                       -- the account's name in Slack, as last seen
    team_name  TEXT,
    created_at INTEGER NOT NULL,           -- unix milliseconds
    PRIMARY KEY (team, slack_user)
);

CREATE INDEX slack_links_user ON slack_links (user_id);

-- A Slack thread is one conversation: its first message started it, each reply is a turn.
-- Only the person who started it continues it from Slack.
CREATE TABLE slack_threads (
    channel         TEXT NOT NULL,
    thread_ts       TEXT NOT NULL,         -- the ts of the thread's first message
    conversation_id TEXT NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    team            TEXT NOT NULL,
    slack_user      TEXT NOT NULL,         -- who started it
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (channel, thread_ts)
);

CREATE INDEX slack_threads_conversation ON slack_threads (conversation_id);

-- The turns a Slack message started: where the answer goes, and whether it went.
CREATE TABLE slack_turns (
    turn_id    TEXT PRIMARY KEY NOT NULL REFERENCES turns (id) ON DELETE CASCADE,
    channel    TEXT NOT NULL,
    thread_ts  TEXT NOT NULL,
    message_ts TEXT NOT NULL,              -- the message that asked, for the reactions on it
    answered   INTEGER NOT NULL DEFAULT 0
);

-- What the bot posted for a turn while its run went: the plan to agree to (`plan`) and each
-- approval (`approval:<id>`), so the post is made once and updated when it is answered.
CREATE TABLE slack_prompts (
    turn_id TEXT NOT NULL REFERENCES turns (id) ON DELETE CASCADE,
    key     TEXT NOT NULL,
    ts      TEXT NOT NULL,
    data    TEXT,                          -- JSON: what was asked, to show again with the answer
    PRIMARY KEY (turn_id, key)
);
