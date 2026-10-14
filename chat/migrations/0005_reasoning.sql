-- The reasoning level (one of the model's, as cedar registers them) a conversation last used, so
-- the picker can restore it, and the one a schedule's runs ask for. NULL means the model's default.
ALTER TABLE conversations ADD COLUMN reasoning TEXT;
ALTER TABLE schedules ADD COLUMN reasoning TEXT;
