-- Live-Lage: the last closed 15-minute window of a region, persons per zone.
-- One row per region, replaced by the next window, so it survives a restart
-- and builds no history. Only zones with enough persons (ELEMENTS_LIVE_K).
CREATE TABLE analytics.live_zones (
    region text NOT NULL,
    synthetic boolean NOT NULL,
    window_start timestamptz NOT NULL,
    span_s integer NOT NULL,
    zones jsonb NOT NULL,
    PRIMARY KEY (region, synthetic)
);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, UPDATE, DELETE ON analytics.live_zones TO elements_app;
  END IF;
END $$;
