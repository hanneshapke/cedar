-- The tools a run may use: a JSON array of tool groups and tool names (the sandbox tools are always
-- included). NULL means every tool the server offers.
ALTER TABLE runs ADD COLUMN tools TEXT;
