-- What a registered model costs, in US dollars per million input and output tokens. Both are set
-- or neither; without them a run's `usage` events carry no `cost_usd`.
ALTER TABLE models ADD COLUMN price_input_usd_per_m REAL;
ALTER TABLE models ADD COLUMN price_output_usd_per_m REAL;
