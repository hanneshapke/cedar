-- Models registered by a service cedar connects to (Cedar Private Inference), so they can be told
-- apart from models the user set up, offered a top-up link when their credit runs out, and given a
-- moment to activate after their key changes.
ALTER TABLE models ADD COLUMN service TEXT;      -- 'cedar-private-inference', or NULL
ALTER TABLE models ADD COLUMN account_url TEXT;  -- where to see the balance and top up
