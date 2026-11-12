-- Agents: a schedule is now one kind of trigger among several. What starts an agent's run is
-- `trigger_kind` with its settings in `trigger` (JSON):
--   time       daily, on weekdays or weekly at a local time (the columns from 0002; `trigger` is
--              NULL for the schedules that were there before)
--   once       one date and time
--   interval   every N minutes
--   monthly    a day of the month at a local time
--   condition  something watched on an interval (a web address, a tool, a record set) and a test
--              on what it returned
--   webhook    a POST to /api/hooks/{hook_id}
--   email      new mail matching a Gmail search
--   calendar   an event about to start, or a new one
--
-- Columns are only added: `schedules` can't be rebuilt, since dropping it would take every run's
-- conversation with it (conversations.schedule_id cascades).
--
-- `next_run_at` stays NOT NULL and means "when to look at this agent next": its next run for the
-- time kinds, its next check for the watching ones, and far in the future (the API says `null`)
-- for a webhook, or a one-off that has run. The old NOT NULL columns hold placeholders on the new
-- kinds (`frequency` the kind, `time_of_day` '00:00'); `timezone` is real for all of them.
ALTER TABLE schedules ADD COLUMN trigger_kind TEXT NOT NULL DEFAULT 'time';
ALTER TABLE schedules ADD COLUMN trigger TEXT;
-- Bumped when the trigger is edited, so a check that was under way doesn't write its result over
-- the new trigger's clean slate.
ALTER TABLE schedules ADD COLUMN trigger_rev INTEGER NOT NULL DEFAULT 0;
-- JSON: what the last check saw (the value, the mail and events already known), and a webhook
-- delivery held back until a run may start.
ALTER TABLE schedules ADD COLUMN state TEXT;
-- A webhook's address is its own random id, not the agent's (which is ordered by time).
ALTER TABLE schedules ADD COLUMN hook_id TEXT;
ALTER TABLE schedules ADD COLUMN hook_secret TEXT;
-- How often an event may start a run; NULL means the defaults.
ALTER TABLE schedules ADD COLUMN min_interval_secs INTEGER;
ALTER TABLE schedules ADD COLUMN max_runs_per_day INTEGER;
ALTER TABLE schedules ADD COLUMN last_checked_at INTEGER;
ALTER TABLE schedules ADD COLUMN last_error TEXT;
ALTER TABLE schedules ADD COLUMN failures INTEGER NOT NULL DEFAULT 0;

CREATE UNIQUE INDEX schedules_hook ON schedules (hook_id) WHERE hook_id IS NOT NULL;

-- JSON: what started the run, `{kind, summary, excerpt?}`. NULL for runs from before, and for
-- "run now".
ALTER TABLE schedule_runs ADD COLUMN cause TEXT;

CREATE INDEX schedule_runs_started ON schedule_runs (started_at);
