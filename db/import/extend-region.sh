#!/usr/bin/env bash
# Grow a region by more admin areas, keeping ids, player state and history
# (data job, from the local OSM copy like new-region.sh):
#
#   ELEMENTS_DB=… db/import/extend-region.sh <slug> '<osm-area>' [<hook args> ...]
#
# <osm-area>: all areas of the region, old and new, joined with ';area'.
# Terrain and sources only add to what is there. DEM_FILES and
# ELEMENTS_REGION_HOOK as in new-region.sh; the hook gets `extend` and
# NEW_SINCE (junctions created since then are the new ones).
# Env: OSM_PARENT (osm_local.py), REGION_NAME (unchanged);
# NEW_SINCE=<time of the first run> resumes after the graph step.
set -euo pipefail
: "${ELEMENTS_DB:?set ELEMENTS_DB}"
. "$(dirname "$0")/../tunnel.sh"
db_tunnel
SLUG="${1:?slug}"
[[ "$SLUG" =~ ^region-[a-z0-9-]+$ ]] || { echo "slug: region-<lowercase>"; exit 2; }
AREA="${2:?admin areas, old and new}"
shift 2
cd "$(dirname "$0")/../.."
q() { psql "$ELEMENTS_DB" -XAt -v ON_ERROR_STOP=1 -c "$1"; }
quiet() { "$@" 2>&1 >/dev/null | grep -vE "NOTICE|DETAIL|drop cascades" >&2 || true; }

# Each area exactly once (names repeat: OSM_PARENT narrows them down).
if [ "$(q "SELECT to_regclass('osm.boundaries') IS NOT NULL")" = t ]; then
  bad="$(q "$(python3 db/import/osm_local.py check "$AREA")" | tr '\n' ' ')"
  [ -z "$bad" ] || { echo "areas found not exactly once: $bad(set OSM_PARENT)"; exit 1; }
fi
UUID="$(q "SELECT id FROM geo.regions WHERE slug = '$SLUG'")"
[ -n "$UUID" ] || { echo "no region $SLUG (new ones: new-region.sh)"; exit 1; }
NAME="${REGION_NAME:-$(q "SELECT name FROM geo.regions WHERE id = '$UUID'")}"
OFFSET="$(q "SELECT (min(id) / 1000000) * 1000000 FROM geo.nodes WHERE region_id = '$UUID'")"
active() { q "SELECT count(*) FROM geo.nodes WHERE region_id = '$UUID' AND is_active $1"; }
SINCE="${NEW_SINCE:-$(q "SELECT now()")}"
old() { q "SELECT count(*) FROM geo.nodes WHERE region_id = '$UUID' AND is_active AND created_at < '$SINCE' $1"; }
OLD="$(old "")"
FIRE="$(old "AND node_type = 'fire_source'")"
WATER="$(old "AND node_type = 'water_source'")"

if [ -z "${NEW_SINCE:-}" ]; then
  echo ">> $SLUG: graph $(date +%T)"
  OSM_AREA="$AREA" REGION_SLUG="$SLUG" REGION_NAME="$NAME" REGION_UUID="$UUID" ID_OFFSET="$OFFSET" \
    db/import/import-osm-imposm.sh | grep -vE "^\(|^-|^$|^ *slug|NOTICE|DETAIL|drop cascades" || true
fi
NODES="$(active "")"
echo ">> $OLD -> $NODES junctions; terrain of the new ones $(date +%T)"
DEM_FILES= REGION_SLUG="$SLUG" NEW_SINCE="$SINCE" quiet db/import/import-terrain-region.sh
if [ -n "${DEM_FILES:-}" ]; then
  echo ">> terrain from DEM_FILES"
  REGION_SLUG="$SLUG" NEW_SINCE="$SINCE" quiet db/import/import-terrain-region.sh
fi
if [ -n "${ELEMENTS_REGION_HOOK:-}" ]; then
  echo ">> hook $(date +%T)"
  REGION_UUID="$UUID" NODES="$NODES" NEW_SINCE="$SINCE" "$ELEMENTS_REGION_HOOK" extend "$SLUG" "$AREA" "$@"
elif [ "$FIRE" -eq 0 ] && [ "$WATER" -eq 0 ]; then
  SRC=$(( NODES / 400 > 10 ? NODES / 400 : 10 ))
  psql "$ELEMENTS_DB" -XAtq -v slug="$SLUG" -v fire="$SRC" -v water="$((SRC * 2))" -f db/import/simple-sources.sql
else
  echo ">> sources kept ($FIRE fire, $WATER water); rerun simple-sources.sql to spread them anew"
fi
echo ">> blocks $(date +%T)"
REGION_SLUG="$SLUG" db/import/import-blocks.sh
q "SELECT '>> done: ' || count(*) || ' junctions, ' || min(elevation_m)::int || '–' || max(elevation_m)::int || ' m' FROM geo.nodes WHERE region_id = '$UUID' AND is_active"
echo "   next: basemap for the new bounds (docs/deploy.md), then POST /api/ops/reload?region=$SLUG"
