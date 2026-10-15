-- Delete a region and everything keyed to it (data job, not a migration):
--   psql "$ELEMENTS_DB" -v slug=region-x -f db/import/drop-region.sql
-- keep=1 replaces only the geodata: players, season results and real
-- analytics stay under the slug for a new region renamed to it (zones and
-- synthetic demo rows go; old edge ids in dwell rows then point nowhere).
\set ON_ERROR_STOP on
SELECT set_config('elements.slug', :'slug', false);
\if :{?keep}
SELECT set_config('elements.keep', :'keep', false);
\else
SELECT set_config('elements.keep', '0', false);
\endif
BEGIN;
DO $$
DECLARE
  s   text := current_setting('elements.slug');
  keep boolean := current_setting('elements.keep') = '1';
  rid uuid;
  t   record;
  n   bigint;
BEGIN
  SELECT id INTO rid FROM geo.regions WHERE slug = s;
  IF rid IS NULL THEN RAISE EXCEPTION 'no region %', s; END IF;

  CREATE TEMP TABLE dn ON COMMIT DROP AS SELECT id FROM geo.nodes WHERE region_id = rid;
  CREATE TEMP TABLE de ON COMMIT DROP AS SELECT id FROM geo.edges WHERE region_id = rid;

  DELETE FROM terrain.edge_samples WHERE edge_id IN (SELECT id FROM de);
  DELETE FROM terrain.node_samples WHERE node_id IN (SELECT id FROM dn);
  DELETE FROM geo.territories WHERE region_id = rid;
  DELETE FROM terrain.dem_tiles WHERE region_id = rid;
  DELETE FROM ops.import_jobs WHERE region_id = rid;
  DELETE FROM geo.edges WHERE region_id = rid;
  GET DIAGNOSTICS n = ROW_COUNT; RAISE NOTICE 'geo.edges: %', n;
  DELETE FROM geo.nodes WHERE region_id = rid;
  GET DIAGNOSTICS n = ROW_COUNT; RAISE NOTICE 'geo.nodes: %', n;

  -- Tables keyed by slug (analytics, game, zones).
  FOR t IN SELECT c.table_schema, c.table_name,
                  EXISTS (SELECT 1 FROM information_schema.columns x WHERE x.table_schema = c.table_schema
                          AND x.table_name = c.table_name AND x.column_name = 'synthetic') AS syn
           FROM information_schema.columns c
           WHERE c.column_name = 'region' AND c.data_type = 'text'
             AND c.table_schema NOT IN ('pg_catalog', 'information_schema')
  LOOP
    IF keep AND NOT (t.table_schema = 'geo' AND t.table_name = 'zones') THEN
      CONTINUE WHEN NOT t.syn;
      EXECUTE format('DELETE FROM %I.%I WHERE region = $1 AND synthetic', t.table_schema, t.table_name) USING s;
    ELSE
      EXECUTE format('DELETE FROM %I.%I WHERE region = $1', t.table_schema, t.table_name) USING s;
    END IF;
    GET DIAGNOSTICS n = ROW_COUNT;
    IF n > 0 THEN RAISE NOTICE '%.%: %', t.table_schema, t.table_name, n; END IF;
  END LOOP;

  DELETE FROM geo.regions WHERE id = rid;
  RAISE NOTICE 'dropped % (keep=%)', s, keep;
END $$;
COMMIT;
