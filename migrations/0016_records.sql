-- Records: the data an agent keeps for the person behind an API key across conversations (a
-- running log, weekly measurements, anything to compare with later), in named sets (see
-- `records`). The `records_*` tools and `/v1/records` change them. Deleting the run a record was
-- saved in keeps the record.
CREATE TABLE record_sets (
    id          TEXT PRIMARY KEY NOT NULL, -- UUIDv7, so id order is creation order
    owner       TEXT NOT NULL,
    name        TEXT NOT NULL,             -- the table name records_lookup sees it as
    description TEXT NOT NULL,
    key_fields  TEXT NOT NULL,             -- JSON array of the fields that identify a record; [] when none do
    health_data INTEGER NOT NULL DEFAULT 0, -- it holds personal health data: runs that see it get the health rules
    created_at  INTEGER NOT NULL,          -- unix milliseconds
    updated_at  INTEGER NOT NULL,          -- unix milliseconds: the last save or drop of a record
    UNIQUE (owner, name)
);

CREATE TABLE records (
    id         TEXT PRIMARY KEY NOT NULL, -- UUIDv7
    set_id     TEXT NOT NULL REFERENCES record_sets (id) ON DELETE CASCADE,
    key        TEXT NOT NULL,             -- the key fields' values, or the id when the set has none
    data       TEXT NOT NULL,             -- the record, a JSON object
    run_id     TEXT REFERENCES runs (id) ON DELETE SET NULL, -- the run that saved it, if any
    created_at INTEGER NOT NULL,          -- unix milliseconds
    updated_at INTEGER NOT NULL,          -- unix milliseconds
    UNIQUE (set_id, key)
);
CREATE INDEX records_set ON records (set_id, id);
