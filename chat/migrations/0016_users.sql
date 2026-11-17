-- More than one person on a server. Everything a person makes here is theirs: conversations (and
-- with them turns and follow-ups), agents (and their runs), settings, the devices their
-- notifications go to, and their contribution receipts.

-- `token_sha256` is the hex SHA-256 of the user's own access token (the token itself is shown
-- once, when it is made). The primary user is who this server belonged to before it had users:
-- it has no token of its own and is whoever presents CHAT_ACCESS_TOKEN (or anyone, without one),
-- and its cedar runs, memories and records stay under cedar-chat's API key, where they always
-- were. Every other user's are under the user's id (`X-Cedar-User`, see cedar's
-- CEDAR_SERVICE_KEYS).
CREATE TABLE users (
    id           TEXT PRIMARY KEY NOT NULL,   -- UUID
    name         TEXT NOT NULL,
    token_sha256 TEXT UNIQUE,
    is_primary   INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL             -- unix milliseconds
);

CREATE UNIQUE INDEX users_primary ON users (is_primary) WHERE is_primary = 1;

-- A random (version 4) UUID for the primary user, so ids from two servers never collide.
INSERT INTO users (id, name, is_primary, created_at)
VALUES (
    lower(printf('%s-%s-4%s-%s%s-%s',
        hex(randomblob(4)), hex(randomblob(2)), substr(hex(randomblob(2)), 2),
        substr('89ab', 1 + (abs(random()) % 4), 1), substr(hex(randomblob(2)), 2),
        hex(randomblob(6)))),
    'Owner', 1, CAST(strftime('%s', 'now') AS INTEGER) * 1000
);

-- Columns are only added (see 0014 on why `schedules` can't be rebuilt), so they are nullable in
-- the schema; every row gets one, and what was here before belongs to the primary user. Turns,
-- follow-ups and schedule runs are their conversation's or agent's.
ALTER TABLE conversations ADD COLUMN user_id TEXT REFERENCES users (id) ON DELETE CASCADE;
ALTER TABLE schedules ADD COLUMN user_id TEXT REFERENCES users (id) ON DELETE CASCADE;
ALTER TABLE push_subscriptions ADD COLUMN user_id TEXT REFERENCES users (id) ON DELETE CASCADE;
ALTER TABLE apns_devices ADD COLUMN user_id TEXT REFERENCES users (id) ON DELETE CASCADE;
ALTER TABLE android_devices ADD COLUMN user_id TEXT REFERENCES users (id) ON DELETE CASCADE;
ALTER TABLE contributions ADD COLUMN user_id TEXT REFERENCES users (id) ON DELETE CASCADE;

UPDATE conversations SET user_id = (SELECT id FROM users WHERE is_primary = 1);
UPDATE schedules SET user_id = (SELECT id FROM users WHERE is_primary = 1);
UPDATE push_subscriptions SET user_id = (SELECT id FROM users WHERE is_primary = 1);
UPDATE apns_devices SET user_id = (SELECT id FROM users WHERE is_primary = 1);
UPDATE android_devices SET user_id = (SELECT id FROM users WHERE is_primary = 1);
UPDATE contributions SET user_id = (SELECT id FROM users WHERE is_primary = 1);

CREATE INDEX conversations_user ON conversations (user_id, updated_at);
CREATE INDEX schedules_user ON schedules (user_id);
CREATE INDEX push_subscriptions_user ON push_subscriptions (user_id);
CREATE INDEX apns_devices_user ON apns_devices (user_id);
CREATE INDEX android_devices_user ON android_devices (user_id);
CREATE INDEX contributions_user ON contributions (user_id, submitted_at);

-- A person's own settings (`preferences`, `notifications`, `contributions`). `settings` keeps
-- what is the server's: the place in cedar's stream of finished runs (`notifications_cursor`).
CREATE TABLE user_settings (
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    key     TEXT NOT NULL,
    value   TEXT NOT NULL, -- JSON
    PRIMARY KEY (user_id, key)
) WITHOUT ROWID;

INSERT INTO user_settings (user_id, key, value)
SELECT (SELECT id FROM users WHERE is_primary = 1), key, value FROM settings
WHERE key IN ('preferences', 'notifications', 'contributions');

DELETE FROM settings WHERE key IN ('preferences', 'notifications', 'contributions');
