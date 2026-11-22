-- What a saved action is carried out with (see `actions`). Until now an action was a website
-- worked through the browser tool; with the `computer` tool it can also be something in an
-- application, on any desktop or only on the user's own computer.
ALTER TABLE actions ADD COLUMN app TEXT;                               -- the application it happens in, when not a website
ALTER TABLE actions ADD COLUMN runs_on TEXT NOT NULL DEFAULT 'browser'; -- browser | computer | own_computer
-- What each picture kept from the recording shows, in order: a JSON array of strings. The
-- pictures themselves are in `action_frames`, so listing actions reads none of them.
ALTER TABLE actions ADD COLUMN frames TEXT NOT NULL DEFAULT '[]';

-- Pictures from an action's recording that show what to look for (a button, a menu, a dialog),
-- made small. A run whose model reads images is shown them with the action's steps.
CREATE TABLE action_frames (
    action_id  TEXT NOT NULL,
    position   INTEGER NOT NULL,   -- from 0, the order of `actions.frames`
    media_type TEXT NOT NULL,
    data       TEXT NOT NULL,      -- base64
    PRIMARY KEY (action_id, position)
);
