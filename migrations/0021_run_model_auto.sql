-- Whether cedar chooses the run's models (see `routing`): the run named none while the
-- `model_selection` setting was off. `model_id` and `model` are then the model it last used.
ALTER TABLE runs ADD COLUMN model_auto INTEGER NOT NULL DEFAULT 0;
