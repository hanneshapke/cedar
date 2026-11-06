-- Memory, revised (see docs/design/memory.md and `memory`): typed rows with provenance, event
-- time separate from transaction time, versions that supersede rather than overwrite, archiving
-- instead of silent deletion, an audit log, a per-owner profile that is always in the prompt, and
-- the counters the background consolidation pass runs on.
CREATE TABLE memories_v2 (
    id               TEXT PRIMARY KEY NOT NULL, -- UUIDv7, so id order is creation order
    owner            TEXT NOT NULL,
    seq              INTEGER NOT NULL,          -- per-owner number, shown to the model as m<seq>
    kind             TEXT NOT NULL CHECK (kind IN ('fact', 'preference', 'instruction', 'temporary', 'episode', 'procedure')),
    text             TEXT NOT NULL,
    subject          TEXT,                      -- with relation: the key a fact supersedes on (home city, employer)
    relation         TEXT,
    topics           TEXT NOT NULL DEFAULT '[]', -- JSON array of short tags
    source           TEXT NOT NULL CHECK (source IN ('user', 'agent', 'consolidation', 'api')),
    support          TEXT,                      -- the user's own words a user-sourced row is taken from
    hedged           INTEGER NOT NULL DEFAULT 0, -- the user said "I think" or "probably"
    sensitive        INTEGER NOT NULL DEFAULT 0, -- health, diet, or the like
    pinned           INTEGER NOT NULL DEFAULT 0, -- always in the prompt; never decays or is archived
    mentions         INTEGER NOT NULL DEFAULT 1, -- how often the user has said it; preferences strengthen
    meta             TEXT NOT NULL DEFAULT '{}', -- JSON, kind-specific: an episode's outcome and tools, a procedure's rationale and record
    run_id           TEXT REFERENCES runs (id) ON DELETE SET NULL, -- the run that saved it, if any
    valid_from       INTEGER,                   -- event time, unix milliseconds: when it became true
    valid_until      INTEGER,                   -- event time: when it stopped being true, or will
    created_at       INTEGER NOT NULL,          -- transaction time: when cedar learned it
    updated_at       INTEGER NOT NULL,
    superseded_by    TEXT REFERENCES memories_v2 (id) ON DELETE SET NULL,
    archived_at      INTEGER,                   -- out of the active set; still listed, restorable
    recall_count     INTEGER NOT NULL DEFAULT 0,
    last_recalled_at INTEGER,
    UNIQUE (owner, seq)
);

INSERT INTO memories_v2 (id, owner, seq, kind, text, source, run_id, valid_from, created_at, updated_at)
SELECT id, owner,
       ROW_NUMBER() OVER (PARTITION BY owner ORDER BY id),
       'fact', text, 'user', run_id, created_at, created_at, updated_at
FROM memories;

DROP TABLE memories;
ALTER TABLE memories_v2 RENAME TO memories;
CREATE INDEX memories_owner ON memories (owner, archived_at, kind);

-- Every change to a memory, by whom: a run's id, 'user' (the API), or 'consolidation'.
CREATE TABLE memory_ops (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    owner     TEXT NOT NULL,
    memory_id TEXT NOT NULL, -- 'profile' for the profile
    op        TEXT NOT NULL, -- add, update, supersede, archive, restore, pin, unpin, delete, expire, merge, recall, profile
    actor     TEXT NOT NULL,
    at        INTEGER NOT NULL,
    before    TEXT,
    after     TEXT
);
CREATE INDEX memory_ops_memory ON memory_ops (owner, memory_id, id);

-- The profile: one short summary of the user, rewritten by consolidation unless the user edited
-- it, and in every run's prompt.
CREATE TABLE memory_profiles (
    owner       TEXT PRIMARY KEY NOT NULL,
    text        TEXT NOT NULL,
    user_edited INTEGER NOT NULL DEFAULT 0,
    updated_at  INTEGER NOT NULL
);

-- When the consolidation pass last ran for an owner, and how many runs have finished since.
CREATE TABLE memory_owners (
    owner           TEXT PRIMARY KEY NOT NULL,
    pending_runs    INTEGER NOT NULL DEFAULT 0,
    last_run_at     INTEGER,
    consolidated_at INTEGER
);

-- The user's verdict on a run (1 or -1), which promotes or archives the procedures it learned.
ALTER TABLE runs ADD COLUMN rating INTEGER;
