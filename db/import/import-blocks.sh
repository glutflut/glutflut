#!/usr/bin/env bash
# Blocks of a region (data job): the faces of the street
# graph from ST_Polygonize, with the junctions on each border. Edges that cross
# another edge without a shared junction (bridges, underpasses) are left out,
# so the faces next to them merge. Faces mostly covered by water (osm.water,
# local OSM copy) are dropped. Replaces the region's rows in geo.blocks; run it
# again after the graph changes (node ids).
#   ELEMENTS_DB=... REGION_SLUG=region-konstanz db/import/import-blocks.sh
set -euo pipefail
: "${ELEMENTS_DB:?set ELEMENTS_DB}" "${REGION_SLUG:?set REGION_SLUG}"
[[ "$REGION_SLUG" =~ ^region-[a-z0-9-]+$ ]] || { echo "slug: region-<lowercase>"; exit 2; }
WATER_SHARE="${WATER_SHARE:-0.5}"
. "$(dirname "$0")/../tunnel.sh"
db_tunnel

psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 -q -At -v slug="$REGION_SLUG" -v water="$WATER_SHARE" <<'SQL'
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

BEGIN;
DELETE FROM geo.blocks WHERE region = :'slug';
INSERT INTO geo.blocks (region, id, geom, area_m2, nodes)
  SELECT :'slug', row_number() OVER (ORDER BY ST_GeoHash(ST_PointOnSurface(fw.g), 12)),
         ST_Multi(fw.g), fw.a, b.nodes
  FROM fw
  CROSS JOIN LATERAL (
    SELECT array_agg(n.id ORDER BY n.id) AS nodes FROM n
    WHERE ST_DWithin(ST_Boundary(fw.g), n.g, 1e-7)) b
  WHERE fw.a > 0 AND fw.wet <= :water AND b.nodes IS NOT NULL;
COMMIT;

SELECT format('blocks: %s (%s km², median %s m², dropped %s wet), %s crossing edges left out',
  count(*), round((sum(area_m2) / 1e6)::numeric, 2),
  round(percentile_cont(0.5) WITHIN GROUP (ORDER BY area_m2)::numeric),
  (SELECT count(*) FROM fw WHERE wet > :water), (SELECT count(DISTINCT id) FROM x))
FROM geo.blocks WHERE region = :'slug';
SQL
