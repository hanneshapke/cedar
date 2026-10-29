-- What the agent remembers about the person behind an API key across conversations (see `memory`).
-- Every run whose tools allow the `memory` group is shown the owner's whole list; the `remember` and
-- `forget` tools and `/v1/memory` change it. Deleting the run a memory was saved in keeps the memory.
CREATE TABLE memories (
    id         TEXT PRIMARY KEY NOT NULL, -- UUIDv7, so id order is creation order
    owner      TEXT NOT NULL,
    text       TEXT NOT NULL,
    run_id     TEXT REFERENCES runs (id) ON DELETE SET NULL, -- the run that saved it, if any
    created_at INTEGER NOT NULL, -- unix milliseconds
    updated_at INTEGER NOT NULL  -- unix milliseconds
);
CREATE INDEX memories_owner ON memories (owner, id);
