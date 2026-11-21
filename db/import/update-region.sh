#!/usr/bin/env bash
# OSM update of a running region (data job): clones the region's graph and
# blocks into a local database, rebuilds them there from that database's OSM
# copy with the same scripts as an import (clip = the stored outline), reports
# what changes and, with --write, writes only the changed rows back:
#
#   ELEMENTS_DB=… ELEMENTS_LOCAL_DB="$(db/local/db.sh url)" \
#     db/import/update-region.sh --dry-run region-konstanz   # nothing written
#   … db/import/update-region.sh --write region-konstanz     # within the limits
#
# ELEMENTS_LOCAL_DB: schema at the same Flyway version as ELEMENTS_DB, and the
# OSM copy of the region's countries (import-osm-extract.sh with fresh
# extracts). Report: $UPDATE_OUT (~/elements-dryrun)/<slug>-<date>.md, plus a
# GeoJSON of gone and new junctions next to it.
# Limits (--write stops, FORCE=1 overrides): MAX_GONE_PCT (3) of the
# junctions, MAX_SOURCES_PCT (5) of the natural sources without a successor
# within SUCCESSOR_M (50) metres.
# --write: terrain of the new junctions locally (Copernicus, then
# `$ELEMENTS_REGION_HOOK terrain <slug>` for finer models), natural sources
# onto their successor, then in one transaction on ELEMENTS_DB: nodes, edges,
# blocks, region bounds, geo.node_moves (gone -> successor; the server moves
# permanent sources, their keys and saved claims when it loads the region),
# hotspot sites and plans. Stops if the region changed since the clone. After:
# POST /api/ops/reload?region=<slug> for a loaded region, basemap refresh.
set -euo pipefail
export LC_ALL=C
case "${1:-}" in --dry-run) MODE=dry ;; --write) MODE=write ;; *) echo "$0 --dry-run|--write <slug>"; exit 2 ;; esac
SLUG="${2:?slug}"
[[ "$SLUG" =~ ^region-[a-z0-9-]+$ ]] || { echo "slug: region-<lowercase>"; exit 2; }
: "${ELEMENTS_DB:?set ELEMENTS_DB (source, read only)}" "${ELEMENTS_LOCAL_DB:?set ELEMENTS_LOCAL_DB}"
MAX_GONE_PCT="${MAX_GONE_PCT:-3}" MAX_SOURCES_PCT="${MAX_SOURCES_PCT:-5}" SUCCESSOR_M="${SUCCESSOR_M:-50}"
OUT="${UPDATE_OUT:-$HOME/elements-dryrun}"
mkdir -p "$OUT"
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/../tunnel.sh"
db_tunnel
cd "$HERE/../.."
src() { psql "$ELEMENTS_DB" -XAt -v ON_ERROR_STOP=1 "$@"; }
loc() { psql "$ELEMENTS_LOCAL_DB" -XAt -v ON_ERROR_STOP=1 "$@"; }
v() { "$1" -c "SELECT max(version::int) FROM flyway_schema_history WHERE success AND version ~ '^[0-9]+$'"; }
[ "$(v src)" = "$(v loc)" ] || { echo "Flyway versions differ: source V$(v src), local V$(v loc)"; exit 1; }
UUID="$(src -c "SELECT id FROM geo.regions WHERE slug = '$SLUG'")"
[ -n "$UUID" ] || { echo "no region $SLUG"; exit 1; }
NAME="$(src -c "SELECT name FROM geo.regions WHERE id = '$UUID'")"
T0=$(date +%s)

echo ">> clone $SLUG into the local database $(date +%T)"
loc -q -v slug="$SLUG" -f db/import/drop-region.sql >/dev/null 2>&1 || true
# Same column order on both sides (same Flyway version).
copy() { # table where
  src -c "\\copy (SELECT * FROM $1 WHERE $2) TO STDOUT" | loc -c "\\copy $1 FROM STDIN"
}
copy geo.regions "id = '$UUID'"
copy geo.nodes "region_id = '$UUID'"
copy geo.edges "region_id = '$UUID'"
copy geo.blocks "region = '$SLUG'"
copy geo.finds "region = '$SLUG'"
# What hangs on junctions (live state stays where it is: only ids and kinds).
loc -q <<SQL
DROP SCHEMA IF EXISTS dry CASCADE; CREATE SCHEMA dry;
CREATE TABLE dry.nodes AS SELECT id, geom, is_active, node_type FROM geo.nodes WHERE region_id = '$UUID';
CREATE TABLE dry.edges AS SELECT id, is_active FROM geo.edges WHERE region_id = '$UUID';
CREATE TABLE dry.blocks AS SELECT id, nodes, area_m2 FROM geo.blocks WHERE region = '$SLUG';
CREATE TABLE dry.refs (kind text, ref text, node_id bigint);
SQL
src -c "\\copy (
  SELECT 'keep', faction, node_id FROM game.keeps WHERE region = '$SLUG'
  UNION ALL SELECT 'keep key', coalesce(label, origin), spot FROM game.keep_keys
    WHERE region = '$SLUG' AND spot IS NOT NULL AND (expires IS NULL OR expires > now())
  UNION ALL SELECT 'hotspot site', name, node_id FROM geo.hotspot_sites WHERE region = '$SLUG' AND node_id IS NOT NULL
  UNION ALL SELECT 'hotspot plan', name, node_id FROM game.hotspot_plans
    WHERE region = '$SLUG' AND node_id IS NOT NULL AND ends > now()) TO STDOUT" \
  | loc -c "\\copy dry.refs FROM STDIN"

echo ">> rebuild from the local OSM copy $(date +%T)"
SINCE="$(loc -c "SELECT now()")"
CLIP="$OUT/$SLUG-clip.geojson"
loc -c "SELECT ST_AsGeoJSON(boundary_geom) FROM geo.regions WHERE id = '$UUID'" > "$CLIP"
OFFSET="$(loc -c "SELECT (min(id) / 1000000) * 1000000 FROM geo.nodes WHERE region_id = '$UUID'")"
ELEMENTS_DB="$ELEMENTS_LOCAL_DB" OSM_CLIP="$CLIP" REGION_SLUG="$SLUG" REGION_NAME="$NAME" REGION_UUID="$UUID" \
  ID_OFFSET="$OFFSET" db/import/import-osm-imposm.sh 2>&1 | grep -E "^>>" || true
BLOCKS="$(ELEMENTS_DB="$ELEMENTS_LOCAL_DB" REGION_SLUG="$SLUG" db/import/import-blocks.sh 2>&1 | grep -E "^blocks:" || true)"

echo ">> report $(date +%T)"
DAY="$(date +%F)"
REP="$OUT/$SLUG-$DAY.md"
OSMDATE="$(loc -c "SELECT string_agg(DISTINCT schema_name, ', ') FROM information_schema.schemata WHERE schema_name LIKE 'osm\\_%'")"
loc -q -v uuid="$UUID" -v succ="$SUCCESSOR_M" <<'SQL'
CREATE TABLE dry.n AS
  SELECT o.id, o.node_type, o.is_active AS was, coalesce(g.is_active, false) AS now,
         o.geom AS old_geom, g.geom AS new_geom
  FROM dry.nodes o LEFT JOIN geo.nodes g ON g.id = o.id;
INSERT INTO dry.n SELECT g.id, g.node_type, false, true, NULL, g.geom
  FROM geo.nodes g WHERE g.region_id = :'uuid' AND NOT EXISTS (SELECT 1 FROM dry.nodes o WHERE o.id = g.id);
CREATE TABLE dry.gone AS
  SELECT n.id, n.node_type, n.old_geom, s.id AS succ, s.d
  FROM dry.n n LEFT JOIN LATERAL (
    SELECT g.id, ST_Distance(g.geom::geography, n.old_geom::geography) AS d FROM geo.nodes g
    WHERE g.region_id = :'uuid' AND g.is_active AND g.id <> n.id
      AND ST_DWithin(g.geom::geography, n.old_geom::geography, :succ)
    ORDER BY g.geom <-> n.old_geom LIMIT 1) s ON true
  WHERE n.was AND NOT n.now;
SQL
q() { loc -F' | ' -c "$1"; }
# Natural sources onto a plain successor (else they stay behind, inactive).
SMOVED="$(loc -c "WITH x AS (UPDATE geo.nodes s SET node_type = g.node_type FROM dry.gone g
  WHERE s.id = g.succ AND s.node_type = 'crossing' AND g.node_type NOT IN ('crossing', 'screen') RETURNING 1)
  SELECT count(*) FROM x")"
# Reasons (private rules, hook): sources without theirs go, finds of gone objects hide.
REASONS=""
if [ -n "${ELEMENTS_REGION_HOOK:-}" ]; then
  REASONS="$(ELEMENTS_DB="$ELEMENTS_LOCAL_DB" REGION_UUID="$UUID" NODES=0 "$ELEMENTS_REGION_HOOK" reasons "$SLUG" "")"
fi
RLOST="$(awk '/^reason /{n += $3} END{print n + 0}' <<< "$REASONS")"
FHIDE="$(awk '/^finds hidden /{print $3}' <<< "$REASONS")"
read -r ACT GONE NEW MOVED <<< "$(loc -F' ' -c "SELECT count(*) FILTER (WHERE was), count(*) FILTER (WHERE was AND NOT now),
  count(*) FILTER (WHERE NOT was AND now), count(*) FILTER (WHERE was AND now AND NOT ST_Equals(old_geom, new_geom)) FROM dry.n")"
read -r EACT EGONE ENEW <<< "$(loc -F' ' -c "SELECT (SELECT count(*) FROM dry.edges WHERE is_active),
  (SELECT count(*) FROM dry.edges o JOIN geo.edges g USING (id) WHERE o.is_active AND NOT g.is_active),
  (SELECT count(*) FROM geo.edges g WHERE g.region_id = '$UUID' AND g.is_active
     AND NOT EXISTS (SELECT 1 FROM dry.edges o WHERE o.id = g.id AND o.is_active))")"
read -r SRC SGONE SLOST <<< "$(loc -F' ' -c "SELECT (SELECT count(*) FROM dry.nodes WHERE is_active AND node_type NOT IN ('crossing', 'screen')),
  count(*), count(*) FILTER (WHERE succ IS NULL) FROM dry.gone WHERE node_type NOT IN ('crossing', 'screen')")"
PG=$(awk -v a="$GONE" -v b="$ACT" 'BEGIN{printf "%.2f", b ? 100*a/b : 0}')
SLOST=$(( SLOST + RLOST ))
PS=$(awk -v a="$SLOST" -v b="$SRC" 'BEGIN{printf "%.2f", b ? 100*a/b : 0}')
VERDICT="within the limits"
awk -v p="$PG" -v m="$MAX_GONE_PCT" 'BEGIN{exit !(p > m)}' && VERDICT="STOP: $PG % of the junctions gone (limit $MAX_GONE_PCT %)"
awk -v p="$PS" -v m="$MAX_SOURCES_PCT" 'BEGIN{exit !(p > m)}' && VERDICT="STOP: $PS % of the natural sources without successor (limit $MAX_SOURCES_PCT %)"

{
  echo "# Dry run: $SLUG ($NAME), $DAY"
  echo
  echo "Source: \`geo.*\` of the live database (read only). OSM copy: $OSMDATE in the local database. Clip: stored outline. $(( $(date +%s) - T0 )) s."
  echo
  echo "**Verdict: $VERDICT.**"
  echo
  echo "| | before | gone | new | moved (id kept) |"
  echo "| --- | ---: | ---: | ---: | ---: |"
  echo "| Junctions | $ACT | $GONE ($PG %) | $NEW | $MOVED |"
  echo "| Edges | $EACT | $EGONE | $ENEW | |"
  echo "| Natural sources | $SRC | $SGONE on gone junctions ($SMOVED onto a successor ≤ $SUCCESSOR_M m), $RLOST without their reason; lost: $SLOST ($PS %) | | |"
  echo
  echo "Blocks: ${BLOCKS#blocks: }"
  echo
  echo "Reasons (fresh OSM): $(grep '^reason ' <<< "$REASONS" | sed 's/^reason //' | paste -sd, - | sed 's/,/, /g')${REASONS:+; }finds hidden: ${FHIDE:-not checked (no hook)}."
  echo
  if grep -q '^find ' <<< "$REASONS"; then
    echo "## Finds to hide (OSM object gone or no longer a place)"
    echo
    grep '^find ' <<< "$REASONS" | sed 's/^find \([0-9]*\) \([a-z]*\) \([^ ]*\) \(.*\)$/- #\1 \4 (\2, \3)/'
    echo
  fi
  echo "## On gone junctions"
  echo
  echo "Kept for live state and the city's places; the successor is the nearest active junction within $SUCCESSOR_M m."
  echo
  echo "| Kind | What | Junction | Successor | Distance |"
  echo "| --- | --- | ---: | ---: | ---: |"
  q "SELECT r.kind, r.ref, g.id, coalesce(g.succ::text, '–'), coalesce(round(g.d)::text || ' m', '–')
     FROM dry.refs r JOIN dry.gone g ON g.id = r.node_id
     UNION ALL SELECT 'source', g.node_type, g.id, coalesce(g.succ::text, '–'), coalesce(round(g.d)::text || ' m', '–')
     FROM dry.gone g WHERE g.node_type NOT IN ('crossing', 'screen') ORDER BY 1, 2" | sed 's/^/| /; s/$/ |/'
  echo
  echo "Map: \`$SLUG-$DAY.geojson\` (gone junctions red, new green)."
} > "$REP"
loc -c "SELECT json_build_object('type', 'FeatureCollection', 'features', coalesce(json_agg(f), '[]'))
  FROM (SELECT json_build_object('type', 'Feature', 'geometry', ST_AsGeoJSON(coalesce(old_geom, new_geom))::json,
          'properties', json_build_object('id', id, 'state', CASE WHEN was THEN 'gone' ELSE 'new' END,
            'node_type', node_type, 'marker-color', CASE WHEN was THEN '#d33' ELSE '#2a2' END)) AS f
        FROM dry.n WHERE was <> now) x" > "$OUT/$SLUG-$DAY.geojson"
echo ">> $VERDICT: $GONE of $ACT junctions gone, $NEW new; report $REP"

[ "$MODE" = write ] || exit 0
if [[ "$VERDICT" == STOP* ]] && [ "${FORCE:-}" != 1 ]; then echo ">> not written ($VERDICT; FORCE=1 overrides)"; exit 3; fi

FRESH="$(loc -c "SELECT count(*) FROM geo.nodes WHERE region_id = '$UUID' AND created_at >= '$SINCE'")"
echo ">> terrain of the $FRESH new junctions $(date +%T)"
[ "$FRESH" = 0 ] || ELEMENTS_DB="$ELEMENTS_LOCAL_DB" DEM_FILES= REGION_SLUG="$SLUG" NEW_SINCE="$SINCE" \
  db/import/import-terrain-region.sh 2>&1 | grep -E "^>>" || true
if [ "$FRESH" != 0 ] && [ -n "${ELEMENTS_REGION_HOOK:-}" ]; then
  ELEMENTS_DB="$ELEMENTS_LOCAL_DB" REGION_UUID="$UUID" NODES=0 NEW_SINCE="$SINCE" \
    "$ELEMENTS_REGION_HOOK" terrain "$SLUG" ""
fi
MISSING="$(loc -c "SELECT count(*) FROM geo.nodes WHERE region_id = '$UUID' AND is_active AND elevation_m IS NULL")"
[ "$MISSING" = 0 ] || { echo ">> not written: $MISSING active junctions without height"; exit 1; }


W="$OUT/$SLUG-$DAY"
mkdir -p "$W"
NCOLS="id, region_id, geom, lat, lon, elevation_m, node_type, is_active, created_at"
ECOLS="id, region_id, source, target, geom, length_m, road_type, one_way, elev_from_m, elev_to_m, grade_forward, grade_reverse, water_cost_forward, water_cost_reverse, fire_cost_forward, fire_cost_reverse, base_travel_cost, is_active, created_at, bridge"
loc -c "\\copy (SELECT $NCOLS FROM geo.nodes WHERE region_id = '$UUID') TO '$W/nodes.tsv'"
loc -c "\\copy (SELECT $ECOLS FROM geo.edges WHERE region_id = '$UUID') TO '$W/edges.tsv'"
loc -c "\\copy (SELECT region, id, geom, area_m2, nodes FROM geo.blocks WHERE region = '$SLUG') TO '$W/blocks.tsv'"
loc -c "\\copy (SELECT id, succ FROM dry.gone WHERE succ IS NOT NULL) TO '$W/moves.tsv'"
loc -c "\\copy (SELECT id FROM geo.finds WHERE region = '$SLUG' AND hidden) TO '$W/finds-hidden.tsv'"
loc -c "\\copy (SELECT ST_AsEWKB(centroid_geom), bounds_min_lat, bounds_min_lon, bounds_max_lat, bounds_max_lon FROM geo.regions WHERE id = '$UUID') TO '$W/region.tsv'"
read -r CACT CMAX <<< "$(loc -F' ' -c "SELECT count(*) FILTER (WHERE is_active), max(id) FROM dry.nodes")"

echo ">> write to ELEMENTS_DB $(date +%T)"
eset() { local a="" c; for c in ${1//,/ }; do [ "$c" = id ] && continue; a+="${a:+, }$c = u.$c"; done; echo "$a"; }
erow() { local a="" c; for c in ${2//,/ }; do [ "$c" = id ] && continue; a+="${a:+, }$1.$c"; done; echo "($a)"; }
src -v uuid="$UUID" -v slug="$SLUG" -v cact="$CACT" -v cmax="$CMAX" <<SQL | tee "$W/written.txt"
BEGIN;
-- Nothing changed under the clone (another import, a reload of the graph).
DO \$\$ BEGIN
  IF (SELECT count(*) FILTER (WHERE is_active) || '/' || max(id) FROM geo.nodes WHERE region_id = '$UUID') <> '$CACT/$CMAX'
  THEN RAISE EXCEPTION 'region changed since the clone: run again'; END IF;
END \$\$;
CREATE TEMP TABLE u_nodes AS SELECT $NCOLS FROM geo.nodes LIMIT 0;
CREATE TEMP TABLE u_edges AS SELECT $ECOLS FROM geo.edges LIMIT 0;
CREATE TEMP TABLE u_blocks AS SELECT region, id, geom, area_m2, nodes FROM geo.blocks LIMIT 0;
CREATE TEMP TABLE u_moves (old_id bigint PRIMARY KEY, new_id bigint);
CREATE TEMP TABLE u_region (c bytea, a float8, b float8, d float8, e float8);
CREATE TEMP TABLE u_hide (id bigint PRIMARY KEY);
\\copy u_nodes FROM '$W/nodes.tsv'
\\copy u_edges FROM '$W/edges.tsv'
\\copy u_blocks FROM '$W/blocks.tsv'
\\copy u_moves FROM '$W/moves.tsv'
\\copy u_region FROM '$W/region.tsv'
\\copy u_hide FROM '$W/finds-hidden.tsv'
CREATE INDEX ON u_nodes (id); CREATE INDEX ON u_edges (id); CREATE INDEX ON u_blocks (id);
ANALYZE u_nodes; ANALYZE u_edges; ANALYZE u_blocks;
\\set QUIET on
WITH x AS (UPDATE geo.nodes g SET $(eset "$NCOLS") FROM u_nodes u
  WHERE g.id = u.id AND $(erow g "$NCOLS") IS DISTINCT FROM $(erow u "$NCOLS") RETURNING 1)
SELECT 'nodes changed ' || count(*) FROM x;
WITH x AS (INSERT INTO geo.nodes ($NCOLS) SELECT $NCOLS FROM u_nodes u
  WHERE NOT EXISTS (SELECT 1 FROM geo.nodes g WHERE g.id = u.id) RETURNING 1)
SELECT 'nodes new ' || count(*) FROM x;
-- Same geometry, same length (another PostGIS differs in the last bits).
UPDATE u_edges u SET length_m = g.length_m FROM geo.edges g WHERE g.id = u.id AND ST_Equals(g.geom, u.geom);
WITH x AS (UPDATE geo.edges g SET $(eset "$ECOLS") FROM u_edges u
  WHERE g.id = u.id AND $(erow g "$ECOLS") IS DISTINCT FROM $(erow u "$ECOLS") RETURNING 1)
SELECT 'edges changed ' || count(*) FROM x;
WITH x AS (INSERT INTO geo.edges ($ECOLS) SELECT $ECOLS FROM u_edges u
  WHERE NOT EXISTS (SELECT 1 FROM geo.edges g WHERE g.id = u.id) RETURNING 1)
SELECT 'edges new ' || count(*) FROM x;
WITH x AS (DELETE FROM geo.blocks b WHERE b.region = :'slug'
  AND NOT EXISTS (SELECT 1 FROM u_blocks u WHERE u.id = b.id) RETURNING 1)
SELECT 'blocks gone ' || count(*) FROM x;
UPDATE u_blocks u SET area_m2 = b.area_m2 FROM geo.blocks b
  WHERE b.region = :'slug' AND b.id = u.id AND abs(b.area_m2 - u.area_m2) <= 1 AND ST_Equals(b.geom, u.geom);
WITH x AS (UPDATE geo.blocks b SET geom = u.geom, area_m2 = u.area_m2, nodes = u.nodes FROM u_blocks u
  WHERE b.region = :'slug' AND b.id = u.id AND (b.geom, b.area_m2, b.nodes) IS DISTINCT FROM (u.geom, u.area_m2, u.nodes) RETURNING 1)
SELECT 'blocks changed ' || count(*) FROM x;
WITH x AS (INSERT INTO geo.blocks (region, id, geom, area_m2, nodes) SELECT region, id, geom, area_m2, nodes FROM u_blocks u
  WHERE NOT EXISTS (SELECT 1 FROM geo.blocks b WHERE b.region = :'slug' AND b.id = u.id) RETURNING 1)
SELECT 'blocks new ' || count(*) FROM x;
UPDATE geo.regions SET centroid_geom = ST_GeomFromEWKB(u.c), bounds_min_lat = u.a, bounds_min_lon = u.b,
  bounds_max_lat = u.d, bounds_max_lon = u.e, updated_at = now() FROM u_region u WHERE id = :'uuid';
-- Moves: junctions back in the graph lose theirs, chains collapse to the newest.
DELETE FROM geo.node_moves m USING u_nodes u WHERE m.region = :'slug' AND m.old_id = u.id AND u.is_active;
UPDATE geo.node_moves m SET new_id = u.new_id, moved_at = now() FROM u_moves u WHERE m.region = :'slug' AND m.new_id = u.old_id AND m.old_id <> u.new_id;
WITH x AS (INSERT INTO geo.node_moves (region, old_id, new_id) SELECT :'slug', old_id, new_id FROM u_moves
  ON CONFLICT (region, old_id) DO UPDATE SET new_id = EXCLUDED.new_id, moved_at = now()
  WHERE geo.node_moves.new_id <> EXCLUDED.new_id RETURNING 1)
SELECT 'moves ' || count(*) FROM x;
WITH x AS (UPDATE geo.hotspot_sites s SET node_id = u.new_id FROM u_moves u WHERE s.region = :'slug' AND s.node_id = u.old_id RETURNING 1)
SELECT 'hotspot sites moved ' || count(*) FROM x;
WITH x AS (UPDATE game.hotspot_plans s SET node_id = u.new_id FROM u_moves u WHERE s.region = :'slug' AND s.node_id = u.old_id RETURNING 1)
SELECT 'hotspot plans moved ' || count(*) FROM x;
-- Finds only ever get hidden here (players keep them in their albums).
WITH x AS (UPDATE geo.finds f SET hidden = true, updated_at = now() FROM u_hide u
  WHERE f.region = :'slug' AND f.id = u.id AND NOT f.hidden RETURNING 1)
SELECT 'finds hidden ' || count(*) FROM x;
COMMIT;
SQL
{
  echo
  echo "## Written $(date '+%F %T')"
  echo
  echo "Rows (only changed ones; natural sources are in the node rows):"
  echo
  grep -E '^[a-z][a-z ]+ [0-9]+$' "$W/written.txt" | sed 's/^/- /'
} >> "$REP"
echo ">> written (report $REP); next: POST /api/ops/reload?region=$SLUG if loaded, basemap refresh"
