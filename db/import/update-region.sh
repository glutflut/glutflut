#!/usr/bin/env bash
# OSM update of a running region, dry run (data job; writes nothing to
# ELEMENTS_DB): clones the region's graph and blocks into a local database,
# rebuilds them there from that database's OSM copy with the same scripts as an
# import (clip = the stored outline), and reports what would change:
#
#   ELEMENTS_DB=… ELEMENTS_LOCAL_DB="$(db/local/db.sh url)" \
#     db/import/update-region.sh --dry-run region-konstanz
#
# ELEMENTS_LOCAL_DB: schema at the same Flyway version as ELEMENTS_DB, and the
# OSM copy of the region's countries (import-osm-extract.sh with fresh
# extracts). Report: $UPDATE_OUT (~/elements-dryrun)/<slug>-<date>.md, plus a
# GeoJSON of gone and new junctions next to it. Terrain is left out (new
# junctions have no height yet in the local copy).
# Limits (stop instead of write, later): MAX_GONE_PCT (3) of the junctions,
# MAX_SOURCES_PCT (5) of the natural sources without a successor within
# SUCCESSOR_M (50) metres.
set -euo pipefail
export LC_ALL=C
[ "${1:-}" = --dry-run ] || { echo "only --dry-run so far: $0 --dry-run <slug>"; exit 2; }
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
read -r ACT GONE NEW MOVED <<< "$(loc -F' ' -c "SELECT count(*) FILTER (WHERE was), count(*) FILTER (WHERE was AND NOT now),
  count(*) FILTER (WHERE NOT was AND now), count(*) FILTER (WHERE was AND now AND NOT ST_Equals(old_geom, new_geom)) FROM dry.n")"
read -r EACT EGONE ENEW <<< "$(loc -F' ' -c "SELECT (SELECT count(*) FROM dry.edges WHERE is_active),
  (SELECT count(*) FROM dry.edges o JOIN geo.edges g USING (id) WHERE o.is_active AND NOT g.is_active),
  (SELECT count(*) FROM geo.edges g WHERE g.region_id = '$UUID' AND g.is_active
     AND NOT EXISTS (SELECT 1 FROM dry.edges o WHERE o.id = g.id AND o.is_active))")"
read -r SRC SGONE SLOST <<< "$(loc -F' ' -c "SELECT (SELECT count(*) FROM dry.nodes WHERE is_active AND node_type NOT IN ('crossing', 'screen')),
  count(*), count(*) FILTER (WHERE succ IS NULL) FROM dry.gone WHERE node_type NOT IN ('crossing', 'screen')")"
PG=$(awk -v a="$GONE" -v b="$ACT" 'BEGIN{printf "%.2f", b ? 100*a/b : 0}')
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
  echo "| Natural sources | $SRC | $SGONE ($SLOST without successor ≤ $SUCCESSOR_M m, $PS %) | | |"
  echo
  echo "Blocks: ${BLOCKS#blocks: }"
  echo
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
