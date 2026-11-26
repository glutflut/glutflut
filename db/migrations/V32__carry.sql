-- Carrying (carry.rs): each side's open site per region with what was
-- delivered (time, fill, builders' game names), and the buildings made there
-- (a source of the side for good, with the builders' names). Loads being
-- carried or lying at a junction stay in memory.
CREATE TABLE game.carry_sites (
    region text NOT NULL,
    faction text NOT NULL CHECK (faction IN ('fire', 'water')),
    node bigint NOT NULL,
    opened timestamptz NOT NULL,
    got jsonb NOT NULL DEFAULT '[]',
    PRIMARY KEY (region, faction)
);

CREATE TABLE game.carry_builds (
    region text NOT NULL,
    node bigint NOT NULL,
    faction text NOT NULL CHECK (faction IN ('fire', 'water')),
    names text[] NOT NULL DEFAULT '{}',
    built_at timestamptz NOT NULL,
    PRIMARY KEY (region, node)
);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, UPDATE, DELETE ON game.carry_sites TO elements_app;
    GRANT SELECT, INSERT, DELETE ON game.carry_builds TO elements_app;
  END IF;
END $$;
