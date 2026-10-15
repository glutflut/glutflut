-- Simple natural sources for a region (data job): fire sources on the high
-- third of the junctions, water sources on the low third, spread out by
-- k-means (one source per cluster, the junction nearest its centre). A
-- starting point for a new region; place them by hand or with your own
-- script for a better game.
--   psql "$ELEMENTS_DB" -v slug=region-x -v fire=10 -v water=20 -f db/import/simple-sources.sql
\set ON_ERROR_STOP on
BEGIN;
CREATE TEMP TABLE n ON COMMIT DROP AS
  SELECT n.id, n.geom, n.elevation_m,
         percent_rank() OVER (ORDER BY n.elevation_m) AS pr
    FROM geo.nodes n JOIN geo.regions r ON r.id = n.region_id
   WHERE r.slug = :'slug' AND n.is_active AND n.elevation_m IS NOT NULL;
UPDATE geo.nodes SET node_type = 'crossing'
 WHERE id IN (SELECT id FROM n) AND node_type IN ('fire_source', 'water_source', 'spring_source');
CREATE TEMP TABLE pick ON COMMIT DROP AS
  WITH c AS (
    SELECT id, geom, side,
           ST_ClusterKMeans(ST_Transform(geom, 3857), k) OVER (PARTITION BY side) AS cl
      FROM (SELECT id, geom, 'fire_source' AS side,
                   least(:fire, (SELECT count(*) FROM n WHERE pr >= 2.0 / 3))::int AS k FROM n WHERE pr >= 2.0 / 3
            UNION ALL
            SELECT id, geom, 'water_source',
                   least(:water, (SELECT count(*) FROM n WHERE pr <= 1.0 / 3))::int FROM n WHERE pr <= 1.0 / 3) s
  ), centre AS (
    SELECT side, cl, ST_Centroid(ST_Collect(geom)) AS g FROM c GROUP BY side, cl
  )
  SELECT DISTINCT ON (c.side, c.cl) c.id, c.side
    FROM c JOIN centre m USING (side, cl)
   ORDER BY c.side, c.cl, c.geom <-> m.g, c.id;
UPDATE geo.nodes g SET node_type = p.side FROM pick p WHERE g.id = p.id;
SELECT 'sources: ' || count(*) FILTER (WHERE side = 'fire_source') || ' fire / '
       || count(*) FILTER (WHERE side = 'water_source') || ' water' FROM pick;
COMMIT;
