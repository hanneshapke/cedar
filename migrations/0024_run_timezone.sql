-- The user's time zone (an IANA name such as 'America/Los_Angeles'). The run is told the date and
-- time there, so "today" and "tomorrow" are the user's; NULL means it isn't known, and the run is
-- told UTC's date and that the user's may differ. Runs from before it have none.
ALTER TABLE runs ADD COLUMN timezone TEXT;
