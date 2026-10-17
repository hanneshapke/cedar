-- Browsers that turned on Web Push (Settings -> Notifications -> Browser): the push service's
-- endpoint for the browser and the keys the payload is encrypted with. An endpoint the push
-- service reports gone (404 or 410) is deleted.
CREATE TABLE push_subscriptions (
    id           TEXT PRIMARY KEY NOT NULL,
    endpoint     TEXT NOT NULL UNIQUE,
    p256dh       TEXT NOT NULL,
    auth         TEXT NOT NULL,
    user_agent   TEXT,
    created_at   INTEGER NOT NULL, -- unix milliseconds
    last_used_at INTEGER
);
