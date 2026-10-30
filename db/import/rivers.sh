#!/usr/bin/env bash
# Rivers for the Flaschenpost (data job): the big rivers of Natural Earth
# (public domain) through the active regions of a shard, ordered from source
# to mouth by the height of each region's junctions along the river
# (geo.river_regions), and the junctions at the water of such a river: near
# osm.water (local OSM copy) that lies along it (geo.node_river).
# Replaces the shard's rows; run it again after new regions (or with regions
# as arguments: only their junctions, the order is always redone).
#   ELEMENTS_DB=... db/import/rivers.sh eu [region-konstanz ...]
set -euo pipefail
: "${ELEMENTS_DB:?set ELEMENTS_DB}"
SHARD="${1:?shard (eu, sa, na)}"
shift
[[ "$SHARD" =~ ^[a-z]+$ ]] || { echo "shard: lowercase"; exit 2; }
# How close a junction is to the water, and the water to the (coarse) river line.
SHORE_M="${SHORE_M:-60}"
ALONG_DEG="${ALONG_DEG:-0.02}"
. "$(dirname "$0")/../tunnel.sh"
db_tunnel
export PGOPTIONS="${DB_TUNNEL_OPTS:-}"

CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/elements/naturalearth"
mkdir -p "$CACHE"
for f in ne_10m_rivers_lake_centerlines ne_10m_rivers_europe ne_10m_rivers_north_america; do
  [[ -s "$CACHE/$f.zip" ]] || curl -fsSL -o "$CACHE/$f.zip" "https://naciscdn.org/naturalearth/10m/physical/$f.zip"
done

# All river lines into a scratch table (dropped at the end).
psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 -q -c "DROP TABLE IF EXISTS geo.ne_rivers_tmp"
for f in ne_10m_rivers_lake_centerlines ne_10m_rivers_europe ne_10m_rivers_north_america; do
  ogr2ogr --config PG_USE_COPY YES -gt 65536 -f PostgreSQL "PG:$ELEMENTS_DB" "/vsizip/$CACHE/$f.zip/$f.shp" \
    -nln geo.ne_rivers_tmp -append -nlt MULTILINESTRING -lco GEOMETRY_NAME=geom \
    -sql "SELECT name FROM $f WHERE name IS NOT NULL AND name <> ''" -t_srs EPSG:4326 -q
done

psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 -q -At -v shard="$SHARD" <<'SQL'
CREATE INDEX ON geo.ne_rivers_tmp USING gist (geom);
ANALYZE geo.ne_rivers_tmp;

-- One line per river (the datasets spell some differently), cut by each
-- region of the shard.
CREATE TEMP TABLE r AS
SELECT river, geom, (SELECT d.geom FROM ST_Dump(ST_LineMerge(geom)) d ORDER BY ST_Length(d.geom) DESC LIMIT 1) AS main
FROM (
  SELECT CASE WHEN name IN ('Rhein', 'Rhin', 'Rijn') THEN 'Rhine' ELSE name END AS river,
         ST_Multi(ST_Union(geom)) AS geom
  FROM geo.ne_rivers_tmp GROUP BY 1
) x;
CREATE TEMP TABLE hit AS
SELECT g.slug AS region, g.id AS region_id, r.river,
       ST_Multi(ST_CollectionExtract(ST_Intersection(r.geom, g.boundary_geom), 2)) AS geom,
       ST_LineLocatePoint(r.main, ST_PointOnSurface(ST_Intersection(r.geom, g.boundary_geom))) AS frac
FROM r JOIN geo.regions g ON g.active AND g.shard = :'shard' AND ST_Intersects(r.geom, g.boundary_geom);
DELETE FROM hit WHERE ST_IsEmpty(geom);

-- Height of each region along the river: its junctions within ~1 km of the
-- line (0 m = no terrain, left out). The order is the position along the
-- line; the heights only say which end is the source (rivers run downhill).
CREATE TEMP TABLE elev AS
SELECT h.*, (SELECT avg(NULLIF(n.elevation_m, 0)) FROM geo.nodes n
             WHERE n.region_id = h.region_id AND n.is_active AND ST_DWithin(n.geom, h.geom, 0.01)) AS e
FROM hit h;
CREATE TEMP TABLE flow AS
SELECT river, coalesce(corr(frac, e), 0) <= 0 AS along FROM elev GROUP BY river;

DELETE FROM geo.river_regions rr USING geo.regions g WHERE g.slug = rr.region AND g.shard = :'shard';
INSERT INTO geo.river_regions (region, river, pos, geom)
SELECT region, river, CASE WHEN f.along THEN frac ELSE 1 - frac END, geom
FROM elev JOIN flow f USING (river);
SELECT 'rivers: ' || count(DISTINCT river) || ' through ' || count(DISTINCT region) || ' regions'
FROM geo.river_regions rr JOIN geo.regions g ON g.slug = rr.region WHERE g.shard = :'shard';
SQL

# The water of the per-country OSM copies directly (their gist indexes; the
# osm.water view's DISTINCT ON keeps the planner from using them).
water=$(psql "$ELEMENTS_DB" -At -c "SELECT string_agg(format('SELECT kind, geometry FROM %I.osm_water', nspname), ' UNION ALL ')
  FROM pg_namespace WHERE nspname LIKE 'osm\_%' AND to_regclass(nspname || '.osm_water') IS NOT NULL")
[[ -n "$water" ]] || water="SELECT kind, geometry FROM osm.water"

# Junctions at the water, region by region (restartable; a few seconds each).
if [[ $# -gt 0 ]]; then
  regions=("$@")
else
  mapfile -t regions < <(psql "$ELEMENTS_DB" -At -v shard="$SHARD" \
    -c "SELECT DISTINCT region FROM geo.river_regions rr JOIN geo.regions g ON g.slug = rr.region WHERE g.shard = :'shard' ORDER BY 1")
fi
for slug in "${regions[@]}"; do
  [[ "$slug" =~ ^region-[a-z0-9-]+$ ]] || { echo "skip $slug"; continue; }
  psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 -q -At -v slug="$slug" -v shore="$SHORE_M" -v along="$ALONG_DEG" <<SQL
CREATE TEMP VIEW wat AS $water;
-- Water along the river, cut into small pieces (a lake is one huge polygon).
CREATE TEMP TABLE w AS
SELECT rr.river, ST_Subdivide(w.geometry, 64) AS g
FROM geo.river_regions rr JOIN wat w ON w.kind IN ('water', 'riverbank')
  AND w.geometry && ST_Expand(rr.geom, :along) AND ST_DWithin(w.geometry, rr.geom, :along)
WHERE rr.region = :'slug';
CREATE INDEX ON w USING gist (g);
ANALYZE w;
DELETE FROM geo.node_river nr USING geo.nodes n, geo.regions g
WHERE nr.node_id = n.id AND n.region_id = g.id AND g.slug = :'slug';
INSERT INTO geo.node_river (node_id, river)
SELECT DISTINCT ON (n.id) n.id, w.river
FROM geo.regions g
JOIN geo.nodes n ON n.region_id = g.id AND n.is_active
JOIN w ON w.g && ST_Expand(n.geom, 0.001) AND ST_DWithin(n.geom::geography, w.g::geography, :shore)
WHERE g.slug = :'slug'
ORDER BY n.id, w.river
ON CONFLICT (node_id) DO NOTHING;
SELECT :'slug' || ': ' || count(*) || ' junctions at a river'
FROM geo.node_river nr JOIN geo.nodes n ON n.id = nr.node_id JOIN geo.regions g ON g.id = n.region_id
WHERE g.slug = :'slug';
SQL
done
psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 -q -c "DROP TABLE IF EXISTS geo.ne_rivers_tmp"
