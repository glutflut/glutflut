#!/usr/bin/env bash
# Blocks of a region (data job): the faces of the street
# graph from ST_Polygonize, with the junctions on each border. Edges that cross
# another edge without a shared junction (bridges, underpasses) are left out,
# so the faces next to them merge. Faces mostly covered by water (osm.water,
# local OSM copy) are dropped. Run it again after the graph changes: blocks keep
# their ids (analytics.block_day), only what changed is written.
#   ELEMENTS_DB=... REGION_SLUG=region-konstanz db/import/import-blocks.sh
set -euo pipefail
: "${ELEMENTS_DB:?set ELEMENTS_DB}" "${REGION_SLUG:?set REGION_SLUG}"
[[ "$REGION_SLUG" =~ ^region-[a-z0-9-]+$ ]] || { echo "slug: region-<lowercase>"; exit 2; }
WATER_SHARE="${WATER_SHARE:-0.5}"
. "$(dirname "$0")/../tunnel.sh"
db_tunnel

psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 -q -At -v slug="$REGION_SLUG" -v water="$WATER_SHARE" <<'SQL'
SET client_min_messages = warning;
SELECT set_config('blocks.slug', :'slug', false) \g /dev/null
CREATE TEMP TABLE e AS
  SELECT e.id, e.source, e.target, e.geom AS g
  FROM geo.edges e JOIN geo.regions r ON r.id = e.region_id
  WHERE r.slug = :'slug' AND e.is_active;
CREATE INDEX ON e USING gist (g); ANALYZE e;
DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM e) THEN RAISE EXCEPTION 'no edges for region'; END IF; END $$;

-- Crossings without a shared junction.
CREATE TEMP TABLE x AS
  SELECT a.id FROM e a JOIN e b ON a.id <> b.id AND a.g && b.g AND ST_Crosses(a.g, b.g)
    AND a.source NOT IN (b.source, b.target) AND a.target NOT IN (b.source, b.target);

CREATE TEMP TABLE f AS
  SELECT (ST_Dump(ST_Polygonize(g))).geom AS g FROM e WHERE id NOT IN (SELECT id FROM x);
CREATE INDEX ON f USING gist (g); ANALYZE f;

-- Water share per face (local OSM copy; none there: nothing dropped).
CREATE TEMP TABLE w (g geometry);
DO $$ BEGIN
  IF to_regclass('osm.water') IS NOT NULL THEN
    -- Clipped to the faces and cut into small pieces: a lake can have 100 000
    -- vertices (Lake Erie), and every face would intersect all of it.
    INSERT INTO w
      SELECT ST_Subdivide(ST_MakeValid(ST_ClipByBox2D(geometry, b.box)), 256)
      FROM osm.water, (SELECT ST_SetSRID(ST_Extent(g), 4326)::box2d AS box FROM f) b
      WHERE geometry && b.box;
  END IF;
END $$;
CREATE INDEX ON w USING gist (g); ANALYZE w;
CREATE TEMP TABLE fw AS
  SELECT f.g, ST_Area(f.g::geography) AS a,
         coalesce((SELECT ST_Area(ST_Intersection(f.g, ST_Union(w.g))) FROM w WHERE w.g && f.g), 0)
           / nullif(ST_Area(f.g), 0) AS wet
  FROM f;

CREATE TEMP TABLE n AS
  SELECT n.id, n.geom AS g FROM geo.nodes n JOIN geo.regions r ON r.id = n.region_id
  WHERE r.slug = :'slug' AND n.is_active;
CREATE INDEX ON n USING gist (g); ANALYZE n;

-- This run's faces with their border junctions.
CREATE TEMP TABLE nb AS
  SELECT row_number() OVER (ORDER BY ST_GeoHash(ST_PointOnSurface(fw.g), 12)) AS fid,
         ST_Multi(CASE WHEN ST_IsValid(fw.g) THEN fw.g ELSE ST_CollectionExtract(ST_MakeValid(fw.g), 3) END) AS g,
         fw.a, b.nodes
  FROM fw
  CROSS JOIN LATERAL (
    SELECT array_agg(n.id ORDER BY n.id) AS nodes FROM n
    WHERE ST_DWithin(ST_Boundary(fw.g), n.g, 1e-7)) b
  WHERE fw.a > 0 AND fw.wet <= :water AND b.nodes IS NOT NULL;
CREATE INDEX ON nb USING gist (g); CREATE INDEX ON nb USING hash (nodes); ANALYZE nb;
-- Old rows may be invalid (written before the check): repaired for matching only.
CREATE TEMP TABLE o AS
  SELECT id, CASE WHEN ST_IsValid(geom) THEN geom ELSE ST_CollectionExtract(ST_MakeValid(geom), 3) END AS g,
         nodes, area_m2 AS a
  FROM geo.blocks WHERE region = :'slug';
CREATE INDEX ON o USING gist (g); CREATE INDEX ON o USING hash (nodes); ANALYZE o;

-- Ids stay (analytics.block_day counts by them): same border junctions, else
-- mostly the same area; each id once, best match first.
CREATE TEMP TABLE m AS
  SELECT DISTINCT ON (o.id) nb.fid, o.id
  FROM nb JOIN o ON o.nodes = nb.nodes
  ORDER BY o.id, nb.fid;
DELETE FROM m a USING m b WHERE a.fid = b.fid AND a.id > b.id;
CREATE INDEX ON m (fid); CREATE INDEX ON m (id); ANALYZE m;
INSERT INTO m
  SELECT DISTINCT ON (c.id) c.fid, c.id
  FROM (SELECT DISTINCT ON (fid) * FROM (
          SELECT nb.fid, o.id,
                 i / nullif(ST_Area(nb.g) + ST_Area(o.g) - i, 0) AS iou
          FROM nb JOIN o ON nb.g && o.g
            -- IoU > 0.5 needs areas within a factor of 2: skips huge faces fast
            AND nb.a BETWEEN o.a * 0.45 AND o.a / 0.45
          CROSS JOIN LATERAL (SELECT ST_Area(ST_Intersection(nb.g, o.g)) AS i) x
          WHERE NOT EXISTS (SELECT 1 FROM m WHERE m.fid = nb.fid)
            AND NOT EXISTS (SELECT 1 FROM m WHERE m.id = o.id)) q
        WHERE iou > 0.5 ORDER BY fid, iou DESC) c
  ORDER BY c.id, c.iou DESC;

-- New ids never reuse one that history still counts.
CREATE TEMP TABLE top (id bigint);
INSERT INTO top SELECT coalesce(max(id), 0) FROM o;
DO $$ BEGIN
  IF to_regclass('analytics.block_day') IS NOT NULL THEN
    EXECUTE format('INSERT INTO top SELECT coalesce(max(block_id), 0) FROM analytics.block_day WHERE region = %L',
                   current_setting('blocks.slug'));
  END IF;
END $$;

BEGIN;
DELETE FROM geo.blocks b WHERE b.region = :'slug' AND NOT EXISTS (SELECT 1 FROM m WHERE m.id = b.id);
UPDATE geo.blocks b SET geom = nb.g, area_m2 = nb.a, nodes = nb.nodes
  FROM m JOIN nb USING (fid)
  WHERE b.region = :'slug' AND b.id = m.id
    AND (b.nodes IS DISTINCT FROM nb.nodes OR abs(b.area_m2 - nb.a) > 1 OR NOT ST_IsValid(b.geom));
INSERT INTO geo.blocks (region, id, geom, area_m2, nodes)
  SELECT :'slug', (SELECT max(id) FROM top) + row_number() OVER (ORDER BY nb.fid), nb.g, nb.a, nb.nodes
  FROM nb WHERE NOT EXISTS (SELECT 1 FROM m WHERE m.fid = nb.fid);
COMMIT;

SELECT format('blocks: %s (%s km², median %s m², dropped %s wet), %s crossing edges left out; ids: %s kept, %s new, %s gone',
  count(*), round((sum(area_m2) / 1e6)::numeric, 2),
  round(percentile_cont(0.5) WITHIN GROUP (ORDER BY area_m2)::numeric),
  (SELECT count(*) FROM fw WHERE wet > :water), (SELECT count(DISTINCT id) FROM x),
  (SELECT count(*) FROM m), (SELECT count(*) FROM nb) - (SELECT count(*) FROM m),
  (SELECT count(*) FROM o) - (SELECT count(*) FROM m))
FROM geo.blocks WHERE region = :'slug';
SQL
