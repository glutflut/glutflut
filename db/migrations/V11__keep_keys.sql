-- Keys to a permanent source for others ([keep] gift_*; later finds by the
-- water and the wind): only the key's hash, the source's ticket hash, uses left.
CREATE TABLE game.keep_keys (
  key bytea PRIMARY KEY,
  region text NOT NULL,
  keep bytea NOT NULL,
  origin text NOT NULL CHECK (origin IN ('gift', 'bottle', 'ember', 'city')),
  uses integer NOT NULL,
  made timestamptz NOT NULL,
  expires timestamptz NOT NULL
);
CREATE INDEX keep_keys_region_idx ON game.keep_keys (region);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, UPDATE, DELETE ON game.keep_keys TO elements_app;
  END IF;
END $$;
