-- Whether a registered model accepts images and PDFs with a prompt. Runs on a model without it
-- refuse attachments. Entries registered so far are text-only until an admin says otherwise.
ALTER TABLE models ADD COLUMN multimodal INTEGER NOT NULL DEFAULT 0;
