-- Keys from the city (editor): to a hotspot site's junction instead of a
-- permanent source, either side; handed out in batches with a label.
ALTER TABLE game.keep_keys ALTER COLUMN keep DROP NOT NULL;
ALTER TABLE game.keep_keys ADD COLUMN spot bigint, ADD COLUMN batch text, ADD COLUMN label text;
ALTER TABLE game.keep_keys ADD CONSTRAINT keep_keys_target CHECK ((keep IS NULL) <> (spot IS NULL));
