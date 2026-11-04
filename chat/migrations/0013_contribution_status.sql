-- What the corpus last said about a contribution (Settings → Contribute → Refresh):
-- `quarantined`, `accepted`, `held`, `rejected`, `released` or `withdrawn`, the releases it is
-- in (a JSON array of versions), and when that was asked.
ALTER TABLE contributions ADD COLUMN status TEXT;
ALTER TABLE contributions ADD COLUMN released_in TEXT;
ALTER TABLE contributions ADD COLUMN checked_at INTEGER; -- unix milliseconds
