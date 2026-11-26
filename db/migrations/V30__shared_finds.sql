-- Shared finds (finds.rs): a find with a quorum stays hidden from everyone
-- (no hint, not counted) until that many trusted players have been within
-- reach_m of it (rule default if NULL), on at least [finds] shared_days days.
-- Then it is revealed: those who helped get it, everyone else can find it.
-- Seekers are stored only until the reveal (pseudonymous id, side, day).
ALTER TABLE geo.finds
    ADD COLUMN quorum int CHECK (quorum IS NULL OR quorum BETWEEN 2 AND 500),
    ADD COLUMN reach_m int CHECK (reach_m IS NULL OR reach_m BETWEEN 40 AND 500),
    ADD COLUMN revealed timestamptz;

CREATE TABLE game.find_seekers (
    find bigint NOT NULL REFERENCES geo.finds (id) ON DELETE CASCADE,
    id bytea NOT NULL,
    faction text NOT NULL CHECK (faction IN ('fire', 'water')),
    -- The region's local day number (score.rs day_of).
    day int NOT NULL,
    PRIMARY KEY (find, id)
);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, DELETE ON game.find_seekers TO elements_app;
  END IF;
END $$;
