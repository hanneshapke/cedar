-- Whether a run's CSV and JSON attachments are imported into record sets when it starts (see
-- `records::import`); resolved from the request and RECORDS_IMPORT_ATTACHMENTS when it is created.
ALTER TABLE runs ADD COLUMN import_attachments INTEGER NOT NULL DEFAULT 0;
