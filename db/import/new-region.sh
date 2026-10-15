#!/usr/bin/env bash
# A new region in one go (data job), from the local OSM copy
# (import-osm-extract.sh) and the Copernicus GLO-30 terrain:
#
#   ELEMENTS_DB=… db/import/new-region.sh <slug> "<name>" '<osm-area>' [<hook args> ...]
#   ELEMENTS_DB=… db/import/new-region.sh region-freiburg "Freiburg im Breisgau" \
#     '["name"="Freiburg im Breisgau"]["admin_level"="6"]'
#
# <osm-area>: admin areas in Overpass syntax, several joined with ';area'
# (e.g. a city and its twin across the border). The region starts active and
# unlisted (by link only); list it with
#   UPDATE geo.regions SET listed = true WHERE slug = '<slug>';
# Finer terrain: DEM_FILES (+ DEM_SRS, DEM_RES_M) as in import-terrain-region.sh,
# sampled after GLO-30. Sources: simple-sources.sql (high/low junctions).
# ELEMENTS_REGION_HOOK=<script>: runs instead of simple-sources.sql, after the
# terrain, as `<script> new <slug> '<osm-area>' [<hook args> ...]` with
# ELEMENTS_DB, REGION_UUID and NODES set (your own terrain, sources, zones, finds).
# Env: OSM_PARENT (admin areas the others lie in, for repeated names),
# LANGUAGE (de), TIMEZONE (Europe/Berlin), SHARD (eu; picks the node id band).
# Re-running redoes everything for the region (players and results stay).
set -euo pipefail
: "${ELEMENTS_DB:?set ELEMENTS_DB}"
# Prod via the VM with ELEMENTS_DB_VIA=user@host.
. "$(dirname "$0")/../tunnel.sh"
db_tunnel
SLUG="${1:?slug, e.g. region-freiburg}"
NAME="${2:?name}"
AREA="${3:?admin area, e.g. [\"name\"=\"Freiburg im Breisgau\"][\"admin_level\"=\"6\"]}"
shift 3
[[ "$SLUG" =~ ^region-[a-z0-9-]+$ ]] || { echo "slug: region-<lowercase>"; exit 2; }
cd "$(dirname "$0")/../.."
q() { psql "$ELEMENTS_DB" -XAt -v ON_ERROR_STOP=1 -c "$1"; }
quiet() { "$@" 2>&1 >/dev/null | grep -vE "NOTICE|DETAIL|drop cascades" >&2 || true; }

# Each area exactly once (names repeat: OSM_PARENT narrows them down).
if [ "$(q "SELECT to_regclass('osm.boundaries') IS NOT NULL")" = t ]; then
  bad="$(q "$(python3 db/import/osm_local.py check "$AREA")" | tr '\n' ' ')"
  [ -z "$bad" ] || { echo "areas found not exactly once: $bad(set OSM_PARENT)"; exit 1; }
fi
# Region id and node id block: its own if it exists, else a new id and the next free block.
UUID="$(q "SELECT id FROM geo.regions WHERE slug = '$SLUG'")"
if [ -z "$UUID" ]; then
  # Random: unique across shards (counted ids clashed when two shards imported at once).
  UUID="$(q "SELECT gen_random_uuid()")"
  q "INSERT INTO geo.regions (id, slug, name, active, listed, language, timezone, shard)
     VALUES ('$UUID', '$SLUG', \$\$${NAME}\$\$, false, false, '${LANGUAGE:-de}', '${TIMEZONE:-Europe/Berlin}', '${SHARD:-eu}')" >/dev/null
  echo ">> new region $SLUG ($UUID), unlisted"
fi
# Each shard its own id band, so regions can move between databases without
# clashes: eu below 1e9, na 1e9-2e9, sa 2e9-3e9.
case "${SHARD:-eu}" in
  eu) LO=1000000 ;; na) LO=1000000000 ;; sa) LO=2000000000 ;;
  *) echo "SHARD: eu, na or sa (add its id band here)"; exit 2 ;;
esac
HI=$(( LO < 1000000000 ? 1000000000 : LO + 1000000000 ))
OFFSET="$(q "SELECT coalesce((SELECT (min(n.id) / 1000000) * 1000000 FROM geo.nodes n WHERE n.region_id = '$UUID'),
                               (SELECT (max(id) / 1000000 + 1) * 1000000 FROM geo.nodes WHERE id >= $LO AND id < $HI), $LO)")"
echo ">> $SLUG: node ids from $OFFSET $(date +%T)"

OSM_AREA="$AREA" REGION_SLUG="$SLUG" REGION_NAME="$NAME" REGION_UUID="$UUID" ID_OFFSET="$OFFSET" \
  db/import/import-osm-imposm.sh | grep -E "^>>|slug|region-" || true
NODES="$(q "SELECT count(*) FROM geo.nodes WHERE region_id = '$UUID'")"
[ "$NODES" -gt 0 ] || { echo "no nodes: is the area in the local OSM copy?"; exit 1; }
echo ">> $NODES junctions; terrain $(date +%T)"
DEM_FILES= REGION_SLUG="$SLUG" quiet db/import/import-terrain-region.sh
if [ -n "${DEM_FILES:-}" ]; then
  echo ">> terrain from DEM_FILES $(date +%T)"
  REGION_SLUG="$SLUG" quiet db/import/import-terrain-region.sh
fi
if [ -n "${ELEMENTS_REGION_HOOK:-}" ]; then
  echo ">> hook $(date +%T)"
  REGION_UUID="$UUID" NODES="$NODES" "$ELEMENTS_REGION_HOOK" new "$SLUG" "$AREA" "$@"
else
  SRC=$(( NODES / 400 > 10 ? NODES / 400 : 10 ))
  psql "$ELEMENTS_DB" -XAtq -v slug="$SLUG" -v fire="$SRC" -v water="$((SRC * 2))" -f db/import/simple-sources.sql
fi
echo ">> blocks $(date +%T)"
REGION_SLUG="$SLUG" db/import/import-blocks.sh
q "SELECT '>> done: ' || count(*) || ' junctions, ' || min(elevation_m)::int || '–' || max(elevation_m)::int || ' m' FROM geo.nodes WHERE region_id = '$UUID'"
echo "   next: basemap (docs/deploy.md); list it when ready (see the top of this script)"
