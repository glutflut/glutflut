-- Post pictures (card_game.rs) draw lakes and rivers from the local OSM copy:
-- each country's osm_<cc>.osm_water (db/import/import-osm-extract.sh, which
-- grants the same for countries added later). The view osm.water can't use
-- their indexes (DISTINCT ON), so the server reads the tables.
DO $$
DECLARE s text;
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    FOR s IN SELECT table_schema FROM information_schema.tables
             WHERE table_name = 'osm_water' AND table_schema LIKE 'osm\_%' LOOP
      EXECUTE format('GRANT USAGE ON SCHEMA %I TO elements_app', s);
      EXECUTE format('GRANT SELECT ON %I.osm_water TO elements_app', s);
    END LOOP;
  END IF;
END $$;
