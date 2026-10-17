-- Whether a run plans before it starts ('off', 'on' or 'auto'), and whether its client answers
-- what the run asks while it goes (the plan, and approvals for risky tool calls). A run that isn't
-- interactive shows its plan without waiting and needs no approvals.
ALTER TABLE runs ADD COLUMN plan TEXT NOT NULL DEFAULT 'off';
ALTER TABLE runs ADD COLUMN interactive INTEGER NOT NULL DEFAULT 0;

-- "Always allow" answers to approvals, per API key: the kind of call (`rule`, e.g. 'downloads')
-- and what was allowed for it (`scope`: a host, a package name, ...).
CREATE TABLE approval_grants (
    owner      TEXT NOT NULL,
    rule       TEXT NOT NULL,
    scope      TEXT NOT NULL,
    created_at INTEGER NOT NULL, -- unix milliseconds
    PRIMARY KEY (owner, rule, scope)
);
