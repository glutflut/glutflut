-- Link keys are stored with origin 'link' (V18, [keep] link_keys).
ALTER TABLE game.keep_keys DROP CONSTRAINT keep_keys_origin_check;
ALTER TABLE game.keep_keys ADD CONSTRAINT keep_keys_origin_check
  CHECK (origin IN ('gift', 'bottle', 'ember', 'city', 'link'));
