-- Loads put down at a junction (carry.rs), so they survive a restart: each
-- waits until `until` for another hand of the side, not the last one (`last`,
-- the player id; gone with the load, hours at most).
CREATE TABLE game.carry_lying (
    region text NOT NULL,
    node bigint NOT NULL,
    faction text NOT NULL CHECK (faction IN ('fire', 'water')),
    fill real NOT NULL,
    hands text[] NOT NULL DEFAULT '{}',
    last bytea NOT NULL,
    until timestamptz NOT NULL
);
CREATE INDEX carry_lying_region ON game.carry_lying (region);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, DELETE ON game.carry_lying TO elements_app;
  END IF;
END $$;
