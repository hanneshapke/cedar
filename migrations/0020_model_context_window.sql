-- A registered model's context window in tokens (prompt and reply together), so runs can keep
-- their requests within it. NULL: not known; it is found out from the server where it tells.
ALTER TABLE models ADD COLUMN context_window INTEGER;
