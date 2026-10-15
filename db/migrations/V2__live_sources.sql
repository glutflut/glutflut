-- Live sources (claims, anchors, sparks) of a region when its app stops
-- (deploy, idle unload), put back on the map at the next start (live.rs).
-- One row per region, taken (deleted) when loaded. Anonymous: junction, side,
-- age and strength, never who placed it (that would be a stored position).
CREATE TABLE game.live_sources (
    region text PRIMARY KEY,
    saved_at timestamptz NOT NULL DEFAULT now(),
    sources jsonb NOT NULL
);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, UPDATE, DELETE ON game.live_sources TO elements_app;
  END IF;
END $$;
