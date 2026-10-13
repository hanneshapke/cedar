-- Preferences set in the Settings dialog (one JSON document per key; today only `preferences`:
-- the default tools, model, system prompt and turn limit for new runs).
CREATE TABLE settings (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL -- JSON
);

-- The tools a conversation, or a schedule, uses instead of the defaults: a JSON array of cedar
-- tool groups and tool names. NULL means the defaults.
ALTER TABLE conversations ADD COLUMN tools TEXT;
ALTER TABLE schedules ADD COLUMN tools TEXT;
