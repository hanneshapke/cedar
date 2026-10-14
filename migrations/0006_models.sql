-- The models a run can choose from, each with the OpenAI-compatible server that serves it. An
-- entry's API key, if any, is in `secrets` under `model_api_key:<id>`. `reasoning_levels` is the
-- JSON array of `reasoning_effort` values the model accepts (empty: none offered), and
-- `default_reasoning` the one a run gets when it names none. `listed_models` is what the server
-- listed at `/models` when the entry was last checked.
CREATE TABLE models (
    id                TEXT PRIMARY KEY NOT NULL,
    label             TEXT NOT NULL,
    model             TEXT NOT NULL,
    base_url          TEXT NOT NULL,
    reasoning_levels  TEXT NOT NULL DEFAULT '[]', -- JSON
    default_reasoning TEXT,
    is_default        INTEGER NOT NULL DEFAULT 0,
    position          INTEGER NOT NULL,
    listed_models     TEXT,                       -- JSON
    created_at        INTEGER NOT NULL,           -- unix milliseconds
    updated_at        INTEGER NOT NULL
);

-- Which entry a run uses (runs made before the registry point at the migrated `default` entry),
-- and the reasoning level it asked for.
ALTER TABLE runs ADD COLUMN model_id TEXT NOT NULL DEFAULT 'default';
ALTER TABLE runs ADD COLUMN reasoning TEXT;
