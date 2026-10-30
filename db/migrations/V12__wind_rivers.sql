-- Funkenflug: the day's strongest wind and where it came from.
ALTER TABLE game.weather_days
  ADD COLUMN wind_kmh real,
  ADD COLUMN wind_from real;

-- Flaschenpost: the big rivers through a region (Natural Earth, public
-- domain), with where the region lies along each river (0 at the source end,
-- 1 at the mouth), so a bottle drifts to regions further downstream.
CREATE TABLE geo.river_regions (
  region text NOT NULL,
  river text NOT NULL,
  pos real NOT NULL,
  geom geometry(MultiLineString, 4326) NOT NULL,
  PRIMARY KEY (region, river)
);
CREATE INDEX river_regions_river_idx ON geo.river_regions (river, pos);

-- Junctions at the water of such a river (near `osm.water` along it): where
-- a bottle can be found.
CREATE TABLE geo.node_river (
  node_id bigint PRIMARY KEY REFERENCES geo.nodes(id) ON DELETE CASCADE,
  river text NOT NULL
);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT ON geo.river_regions, geo.node_river TO elements_app;
  END IF;
END $$;
