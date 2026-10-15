-- The local OSM copy as one set of views over all imported extracts
-- (schemas osm_<code>, see import-osm-extract.sh). Extracts overlap at their
-- borders; the views keep one row per OSM object.
CREATE SCHEMA IF NOT EXISTS osm;
DO $$
DECLARE
  t text;
  parts text;
BEGIN
  FOREACH t IN ARRAY ARRAY['ways', 'pois', 'trees', 'water', 'coastline', 'boundaries', 'places', 'closed'] LOOP
    -- Copies of different ages may lack newer columns (ways.bridge/man_made,
    -- 2026-10-10; ways.access/foot, 2026-10-14): every copy gets every
    -- column, NULL where it has none.
    WITH s AS (
      SELECT nspname FROM pg_namespace
       WHERE nspname ~ '^osm_[a-z]{2,3}$' AND to_regclass(format('%I.osm_%s', nspname, t)) IS NOT NULL
    ), c AS (
      SELECT a.attname, format_type(a.atttypid, a.atttypmod) AS typ, min(a.attnum) AS ord
        FROM s JOIN pg_attribute a ON a.attrelid = format('%I.osm_%s', s.nspname, t)::regclass
       WHERE a.attnum > 0 AND NOT a.attisdropped
       GROUP BY a.attname, typ
    )
    SELECT string_agg(
             format('SELECT %s FROM %I.osm_%s', (
               SELECT string_agg(CASE WHEN EXISTS (
                        SELECT 1 FROM pg_attribute a
                         WHERE a.attrelid = format('%I.osm_%s', s.nspname, t)::regclass
                           AND a.attname = c.attname AND NOT a.attisdropped)
                      THEN quote_ident(c.attname)
                      ELSE format('NULL::%s AS %I', c.typ, c.attname) END, ', ' ORDER BY c.ord, c.attname)
                 FROM c), s.nspname, t),
             ' UNION ALL ')
      INTO parts
      FROM s;
    EXECUTE format('DROP VIEW IF EXISTS osm.%I', t);
    IF parts IS NOT NULL THEN
      EXECUTE format('CREATE VIEW osm.%I AS SELECT DISTINCT ON (osm_id) * FROM (%s) u', t, parts);
    END IF;
    -- Without DISTINCT ON the spatial indexes work; border duplicates stay
    -- (count distinct osm_id). Only for points of interest so far.
    IF t = 'pois' THEN
      EXECUTE 'DROP VIEW IF EXISTS osm.pois_all';
      IF parts IS NOT NULL THEN
        EXECUTE format('CREATE VIEW osm.pois_all AS %s', parts);
      END IF;
    END IF;
  END LOOP;
END $$;

-- The game server counts gastronomy near hotspot sites (editor).
DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') AND to_regclass('osm.pois') IS NOT NULL THEN
    GRANT USAGE ON SCHEMA osm TO elements_app;
    GRANT SELECT ON osm.pois, osm.pois_all TO elements_app;
  END IF;
END $$;
