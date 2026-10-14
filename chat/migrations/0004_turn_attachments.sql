-- What the user attached to the message, without the bytes: a JSON array of
-- {name, media_type, size}. NULL for none. The files themselves live in cedar's run.
ALTER TABLE turns ADD COLUMN attachments TEXT;
