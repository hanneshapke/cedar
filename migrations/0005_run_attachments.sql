-- Files sent with a run's prompt (images and PDFs), base64 as received. They become the
-- image/document blocks of the run's opening user message when it starts, and are deleted with
-- the run. A continuation gets none of its own: the parent's opening message, which it copies,
-- already carries the blocks.
CREATE TABLE run_attachments (
    run_id     TEXT NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    name       TEXT NOT NULL,
    media_type TEXT NOT NULL,
    data       TEXT NOT NULL, -- base64
    PRIMARY KEY (run_id, seq)
);
