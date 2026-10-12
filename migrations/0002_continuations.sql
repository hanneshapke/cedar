-- Continuations: a run can continue an earlier one, picking up its conversation and its workspace.

-- A sandbox workspace (a docker volume) shared by a chain of runs. It outlives the runs' containers
-- and is removed once it has been idle for WORKSPACE_TTL_SECS, or when its last run is deleted.
CREATE TABLE workspaces (
    id           TEXT PRIMARY KEY NOT NULL,
    owner        TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    expired_at   INTEGER -- set when the volume is (being) removed; the workspace can't be reused after
);

ALTER TABLE runs ADD COLUMN parent_id TEXT REFERENCES runs (id) ON DELETE SET NULL;
ALTER TABLE runs ADD COLUMN workspace_id TEXT;
-- 1 if the run picked up its parent's workspace, 0 if it started with an empty one.
ALTER TABLE runs ADD COLUMN workspace_reused INTEGER NOT NULL DEFAULT 0;

-- Earlier runs each had a workspace named after themselves, already removed when they finished.
-- With no `workspaces` row they count as expired.
UPDATE runs SET workspace_id = id;

-- A run is continued at most once, so a chain is a line and two runs never share a live workspace.
CREATE UNIQUE INDEX runs_parent ON runs (parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX runs_workspace ON runs (workspace_id);
