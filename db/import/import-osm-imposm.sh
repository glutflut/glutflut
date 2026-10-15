#!/usr/bin/env bash
# Import a whole city's walkable ways from an OSM PBF (Geofabrik extract) into
# `geo.*` as a node/edge graph, with imposm3 and SQL instead of pgRouting.
#
# imposm writes the ways (exact OSM coordinates, EPSG:4326) to a staging
# schema; the graph comes from the OSM topology itself: a junction is a vertex
# several ways share (or a way's end), ways are split there. Bridges and
# tunnels don't share a node with what they cross, so they stay apart (unlike
# pgRouting's geometric noding). Scales to cities
# with hundreds of thousands of nodes.
#
# Usage:
#   ELEMENTS_DB='postgresql://…' OSM_FILE=hamburg-latest.osm.pbf OSM_CLIP=clip.geojson \
#   REGION_SLUG=region-hamburg REGION_NAME=Hamburg \
#   REGION_UUID=a1b2c3d4-0000-0000-0000-000000000010 ID_OFFSET=7000000 \
#     db/import/import-osm-imposm.sh
#
# OSM_CLIP: polygon (GeoJSON, EPSG:4326) the ways are cut to (imposm -limitto).
# Re-running keeps settings and ids: junctions within 1 m of an existing node
# and edges between the same nodes keep theirs, new ones get the next free ids,
# vanished ones are deactivated. Needs psql and imposm (downloaded to
# ~/.cache/elements/bin if missing).
set -euo pipefail

: "${ELEMENTS_DB:?set ELEMENTS_DB to the Postgres connection string}"
# Prod via the VM with ELEMENTS_DB_VIA=user@host.
. "$(dirname "$0")/../tunnel.sh"
db_tunnel
# Without OSM_FILE: the ways come from the local OSM copy (osm.ways, see
# import-osm-extract.sh), cut to OSM_CLIP or to the admin areas in OSM_AREA
# (Overpass syntax, e.g. '["name"="Bamberg"]["admin_level"="6"]', several
# joined with ';area').
OSM_FILE="${OSM_FILE:-}"
OSM_CLIP="${OSM_CLIP:-}"
OSM_AREA="${OSM_AREA:-}"
[ -n "$OSM_FILE$OSM_CLIP$OSM_AREA" ] || { echo "set OSM_FILE+OSM_CLIP, or OSM_CLIP or OSM_AREA for the local copy"; exit 2; }
[ -z "$OSM_FILE" ] || [ -n "$OSM_CLIP" ] || { echo "OSM_FILE needs OSM_CLIP"; exit 2; }
: "${REGION_SLUG:?}" "${REGION_NAME:?}" "${REGION_UUID:?}" "${ID_OFFSET:?}"

IMPOSM_VERSION=0.14.2
BIN="${XDG_CACHE_HOME:-$HOME/.cache}/elements/bin"
IMPOSM="$BIN/imposm-$IMPOSM_VERSION-linux-x86-64/imposm"
if [[ ! -x "$IMPOSM" ]]; then
  mkdir -p "$BIN"
  curl -sL "https://github.com/omniscale/imposm3/releases/download/v$IMPOSM_VERSION/imposm-$IMPOSM_VERSION-linux-x86-64.tar.gz" | tar -xz -C "$BIN"
fi
HERE="$(cd "$(dirname "$0")" && pwd)"
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/elements/imposm/$REGION_SLUG"
mkdir -p "$CACHE"

# imposm (lib/pq) wants postgis://, doesn't know channel_binding and sends no
# SNI, so Neon needs its endpoint id as an option.
PGIS="$(python3 -c '
import sys, urllib.parse as u
p = u.urlparse(sys.argv[1])
q = [(k, v) for k, v in u.parse_qsl(p.query) if k != "channel_binding"]
host = p.hostname or ""
if host.endswith(".neon.tech"):
    q.append(("options", "endpoint=" + host.split(".")[0].removesuffix("-pooler")))
print(p._replace(scheme="postgis", query=u.urlencode(q)).geturl())' "$ELEMENTS_DB")"

if [ -n "$OSM_CLIP" ]; then
  CLIP_SQL="ST_SetSRID(ST_GeomFromGeoJSON('$(python3 -c '
import json, sys
d = json.load(open(sys.argv[1]))
g = d["features"][0]["geometry"] if d.get("type") == "FeatureCollection" else d
print(json.dumps(g))' "$OSM_CLIP")'), 4326)"
else
  CLIP_SQL="$(cd "$HERE" && python3 -c 'import osm_local, sys; print(osm_local.reach_sql(sys.argv[1]))' "$OSM_AREA")"
fi
psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 -q \
  -c "DROP SCHEMA IF EXISTS osm_import CASCADE; CREATE SCHEMA osm_import;" \
  -c "CREATE TABLE osm_import.clip AS SELECT $CLIP_SQL AS g;"
# Hand list of closed areas (<slug>.geojson in ELEMENTS_CLOSED_DIR, default
# db/import/closed; see closed-areas.sql).
HAND="${ELEMENTS_CLOSED_DIR:-$HERE/closed}/$REGION_SLUG.geojson"
python3 -c '
import json, sys
print("CREATE TABLE osm_import.hand (g geometry);")
if len(sys.argv) > 1:
    for f in json.load(open(sys.argv[1]))["features"]:
        g = json.dumps(f["geometry"]).replace("\x27", "\x27\x27")
        print(f"INSERT INTO osm_import.hand VALUES (ST_MakeValid(ST_SetSRID(ST_GeomFromGeoJSON(\x27{g}\x27), 4326)));")
' $([ -f "$HAND" ] && echo "$HAND") | psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 -q
if [ -n "$OSM_FILE" ]; then
  echo ">> [1/3] imposm: $OSM_FILE, cut to $OSM_CLIP -> osm_import.osm_roads"
  "$IMPOSM" import -connection "$PGIS" -mapping "$HERE/mapping-city-graph.yaml" \
    -read "$OSM_FILE" -write -srid 4326 -limitto "$OSM_CLIP" \
    -cachedir "$CACHE" -overwritecache -dbschema-import osm_import -quiet
else
  echo ">> [1/3] local OSM copy (osm.ways) -> osm_import.osm_roads"
  psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 -q <<SQL
DO \$\$ BEGIN IF (SELECT g IS NULL FROM osm_import.clip) THEN RAISE EXCEPTION 'clip area not found'; END IF; END \$\$;
CREATE TABLE osm_import.osm_roads AS
SELECT row_number() OVER () AS id, d.geom AS geometry, w.highway, w.oneway, w.bridge, w.man_made, w.access, w.foot
FROM osm.ways w, osm_import.clip c,
     LATERAL ST_Dump(ST_CollectionExtract(ST_Intersection(w.geometry, c.g), 2)) d
WHERE w.geometry && c.g AND ST_Intersects(w.geometry, c.g);
-- Closed areas.
CREATE TABLE osm_import.osm_closed AS
SELECT k.* FROM osm.closed k, osm_import.clip c WHERE k.geometry && c.g AND ST_Intersects(k.geometry, c.g);
SQL
fi

echo ">> [2/3] graph from shared vertices"
psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 -q -v closed="$HERE/closed-areas.sql" <<'SQL'
SET search_path = osm_import, public;
SET work_mem = '256MB';
-- Piers (landing stages, jetties) are no streets to play on; a path on a pier
-- area (e.g. a mole) has no man_made of its own and stays.
-- imposm writes '' for a missing tag, other paths NULL; both mean none.
DELETE FROM osm_roads WHERE man_made = 'pier';
-- Every vertex of every way, in order.
CREATE TABLE v AS
SELECT r.id AS way, d.path[1] AS i, ST_X(d.geom) AS x, ST_Y(d.geom) AS y,
       ST_NPoints(r.geometry) AS n
FROM osm_roads r, ST_DumpPoints(r.geometry) d;
-- Junctions: used more than once (by several ways, or twice by one) or a way's end.
CREATE TABLE shared AS SELECT x, y FROM v GROUP BY x, y HAVING count(*) > 1;
CREATE INDEX ON shared (x, y);
CREATE TABLE vj AS
SELECT v.way, v.i, v.x, v.y,
       (v.i = 1 OR v.i = v.n OR s.x IS NOT NULL) AS j
FROM v LEFT JOIN shared s ON s.x = v.x AND s.y = v.y;
-- Segment number = junctions before the vertex; a junction also starts the next one.
CREATE TABLE seg AS
SELECT way, i, x, y, j,
       coalesce(sum(j::int) OVER (PARTITION BY way ORDER BY i
                ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 0) AS s
FROM vj;
CREATE TABLE edge AS
SELECT way, s,
       ST_MakeLine(ST_SetSRID(ST_MakePoint(x, y), 4326) ORDER BY i) AS geom
FROM (SELECT way, s, i, x, y FROM seg
      UNION ALL
      SELECT way, s + 1, i, x, y FROM seg WHERE j) q
GROUP BY way, s
HAVING count(*) >= 2;
CREATE TABLE node AS
SELECT row_number() OVER (ORDER BY x, y) AS id, x, y
FROM (SELECT DISTINCT x, y FROM seg WHERE j) q;
CREATE INDEX ON node (x, y);
CREATE TABLE edge2 AS
SELECT row_number() OVER (ORDER BY e.way, e.s) AS id, a.id AS source, b.id AS target,
       e.geom, r.highway, r.oneway, coalesce(nullif(r.bridge, ''), 'no') <> 'no' AS bridge, r.access, r.foot
FROM edge e
JOIN osm_roads r ON r.id = e.way
JOIN node a ON a.x = ST_X(ST_StartPoint(e.geom)) AND a.y = ST_Y(ST_StartPoint(e.geom))
JOIN node b ON b.x = ST_X(ST_EndPoint(e.geom)) AND b.y = ST_Y(ST_EndPoint(e.geom))
WHERE ST_Length(e.geom) > 0;
-- Private ways and closed areas out (closed-areas.sql).
\i :closed
SET search_path = osm_import, public;
-- Nodes no edge uses (zero-length pieces, closed streets).
CREATE TABLE used AS SELECT source AS id FROM edge2 UNION SELECT target FROM edge2;
DELETE FROM node n WHERE NOT EXISTS (SELECT 1 FROM used u WHERE u.id = n.id);
SQL

echo ">> [3/3] region/nodes/edges into geo.*"
psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 \
  -v rid="$REGION_UUID" -v slug="$REGION_SLUG" -v rname="$REGION_NAME" -v off="$ID_OFFSET" <<'SQL'
SET search_path = osm_import, public;
BEGIN;
-- Region: outline = the clip, bounds + centroid of the nodes; settings of an earlier import stay.
WITH g AS (
  SELECT ST_Collect(ST_SetSRID(ST_MakePoint(x, y), 4326)) AS pts FROM node
), e AS (
  SELECT ST_Envelope(pts) AS env, ST_Centroid(pts) AS cen FROM g
)
INSERT INTO geo.regions (id, slug, name, boundary_geom, centroid_geom,
                         bounds_min_lat, bounds_min_lon, bounds_max_lat, bounds_max_lon, active)
SELECT :'rid', :'slug', :'rname',
       coalesce(ST_Multi(ST_CollectionExtract(ST_MakeValid((SELECT g FROM clip)), 3)), ST_Multi(env)), cen,
       ST_YMin(env), ST_XMin(env), ST_YMax(env), ST_XMax(env), true
FROM e
ON CONFLICT (id) DO UPDATE SET
  slug = EXCLUDED.slug, name = EXCLUDED.name, boundary_geom = EXCLUDED.boundary_geom,
  centroid_geom = EXCLUDED.centroid_geom,
  bounds_min_lat = EXCLUDED.bounds_min_lat, bounds_min_lon = EXCLUDED.bounds_min_lon,
  bounds_max_lat = EXCLUDED.bounds_max_lat, bounds_max_lon = EXCLUDED.bounds_max_lon,
  active = true;
CREATE TABLE lim AS SELECT :off + 1000000 AS hi;

-- Nodes: the nearest existing node within 1 m keeps its id (each once, closest wins).
ALTER TABLE node ADD COLUMN geom geometry(Point, 4326);
UPDATE node SET geom = ST_SetSRID(ST_MakePoint(x, y), 4326);
CREATE TABLE old_n AS SELECT id, geom FROM geo.nodes WHERE region_id = :'rid';
CREATE INDEX ON old_n USING gist (geom);
CREATE TABLE nmatch AS
SELECT DISTINCT ON (o.id) n.id AS nid, o.id AS old_id
FROM node n CROSS JOIN LATERAL (
  SELECT o.id, ST_Distance(o.geom::geography, n.geom::geography) AS d FROM old_n o
  WHERE o.geom && ST_Expand(n.geom, 0.00002) ORDER BY o.geom <-> n.geom LIMIT 1) o
WHERE o.d < 1
ORDER BY o.id, o.d;
CREATE TABLE nid AS
SELECT n.id AS nid, n.x, n.y, n.geom, m.old_id IS NOT NULL AS matched,
       coalesce(m.old_id, (SELECT greatest(max(id), :off) FROM geo.nodes WHERE region_id = :'rid')
                + row_number() OVER (PARTITION BY m.old_id IS NULL ORDER BY n.x, n.y)) AS id
FROM node n LEFT JOIN nmatch m ON m.nid = n.id;
DO $$ BEGIN
  IF (SELECT max(id) FROM osm_import.nid) >= (SELECT hi FROM osm_import.lim) THEN
    RAISE EXCEPTION 'node ids beyond the region''s block';
  END IF;
END $$;
UPDATE geo.nodes g SET geom = n.geom, lat = n.y, lon = n.x, is_active = true
FROM nid n WHERE n.matched AND g.id = n.id;
INSERT INTO geo.nodes (id, region_id, geom, lat, lon)
SELECT id, :'rid', geom, y, x FROM nid WHERE NOT matched;
UPDATE geo.nodes g SET is_active = false
WHERE g.region_id = :'rid' AND g.is_active AND NOT EXISTS (SELECT 1 FROM nid n WHERE n.id = g.id);

-- Edges: an existing edge between the same nodes keeps its id (closest shape, each once).
CREATE TABLE e AS
SELECT e.id AS eid, a.id AS source, b.id AS target, e.geom, e.highway, e.oneway, e.bridge
FROM edge2 e JOIN nid a ON a.nid = e.source JOIN nid b ON b.nid = e.target;
CREATE TABLE old_e AS SELECT id, source, target, geom FROM geo.edges WHERE region_id = :'rid';
CREATE INDEX ON old_e (least(source, target), greatest(source, target));
CREATE TABLE ecand AS
SELECT e.eid, o.id AS old_id, o.source <> o.target AND o.source = e.target AS flipped,
       ST_HausdorffDistance(e.geom, o.geom) AS h
FROM e JOIN old_e o ON least(o.source, o.target) = least(e.source, e.target)
                   AND greatest(o.source, o.target) = greatest(e.source, e.target);
CREATE TABLE ematch AS
SELECT DISTINCT ON (old_id) eid, old_id, flipped
FROM (SELECT DISTINCT ON (eid) * FROM ecand WHERE h < 0.0002 ORDER BY eid, h) best
ORDER BY old_id, h;
CREATE TABLE eid AS
SELECT e.*, m.old_id IS NOT NULL AS matched, coalesce(m.flipped, false) AS flipped,
       coalesce(m.old_id, (SELECT greatest(max(id), :off) FROM geo.edges WHERE region_id = :'rid')
                + row_number() OVER (PARTITION BY m.old_id IS NULL ORDER BY e.eid)) AS id
FROM e LEFT JOIN ematch m ON m.eid = e.eid;
DO $$ BEGIN
  IF (SELECT max(id) FROM osm_import.eid) >= (SELECT hi FROM osm_import.lim) THEN
    RAISE EXCEPTION 'edge ids beyond the region''s block';
  END IF;
END $$;
-- Terrain values stay; a reversed edge swaps its directional ones.
UPDATE geo.edges g SET
  source = n.source, target = n.target, geom = n.geom, length_m = ST_Length(n.geom::geography),
  road_type = n.highway, one_way = coalesce(n.oneway, 0) <> 0, bridge = n.bridge, is_active = true,
  elev_from_m = CASE WHEN n.flipped THEN g.elev_to_m ELSE g.elev_from_m END,
  elev_to_m = CASE WHEN n.flipped THEN g.elev_from_m ELSE g.elev_to_m END,
  grade_forward = CASE WHEN n.flipped THEN g.grade_reverse ELSE g.grade_forward END,
  grade_reverse = CASE WHEN n.flipped THEN g.grade_forward ELSE g.grade_reverse END,
  water_cost_forward = CASE WHEN n.flipped THEN g.water_cost_reverse ELSE g.water_cost_forward END,
  water_cost_reverse = CASE WHEN n.flipped THEN g.water_cost_forward ELSE g.water_cost_reverse END,
  fire_cost_forward = CASE WHEN n.flipped THEN g.fire_cost_reverse ELSE g.fire_cost_forward END,
  fire_cost_reverse = CASE WHEN n.flipped THEN g.fire_cost_forward ELSE g.fire_cost_reverse END
FROM eid n WHERE n.matched AND g.id = n.id;
INSERT INTO geo.edges (id, region_id, source, target, geom, length_m, road_type, one_way, bridge)
SELECT id, :'rid', source, target, geom, ST_Length(geom::geography), highway, coalesce(oneway, 0) <> 0, bridge
FROM eid WHERE NOT matched;
UPDATE geo.edges g SET is_active = false
WHERE g.region_id = :'rid' AND g.is_active AND NOT EXISTS (SELECT 1 FROM eid n WHERE n.id = g.id);

SELECT (SELECT count(*) FILTER (WHERE matched) FROM nid) AS nodes_kept,
       (SELECT count(*) FILTER (WHERE NOT matched) FROM nid) AS nodes_new,
       (SELECT count(*) FROM old_n o WHERE NOT EXISTS (SELECT 1 FROM nid n WHERE n.id = o.id)) AS nodes_inactive,
       (SELECT count(*) FILTER (WHERE matched) FROM eid) AS edges_kept,
       (SELECT count(*) FILTER (WHERE NOT matched) FROM eid) AS edges_new,
       (SELECT count(*) FROM old_e o WHERE NOT EXISTS (SELECT 1 FROM eid n WHERE n.id = o.id)) AS edges_inactive;
COMMIT;

DROP SCHEMA IF EXISTS osm_import CASCADE;

SELECT slug, name,
       (SELECT count(*) FROM geo.nodes WHERE region_id = :'rid' AND is_active) AS nodes,
       (SELECT count(*) FROM geo.edges WHERE region_id = :'rid' AND is_active) AS edges
FROM geo.regions WHERE id = :'rid';
SQL
