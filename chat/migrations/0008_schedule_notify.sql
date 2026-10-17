-- How a scheduled agent tells the user its run finished: one of the methods set up under
-- Settings -> Notifications ('browser', 'pushover', 'email'), 'none' for not at all, or NULL to
-- follow the default method there.
ALTER TABLE schedules ADD COLUMN notify TEXT;
