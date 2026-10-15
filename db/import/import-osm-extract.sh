#!/usr/bin/env bash
# Local OSM copy (data job): one Geofabrik extract into its own schema with
# imposm (mapping-osm-copy.yaml), then the `osm` views over all of them
# (osm-views.sql). The region imports read these instead of Overpass.
#
#   ELEMENTS_DB=… db/import/import-osm-extract.sh de ~/.cache/elements/osm/pbf/germany-latest.osm.pbf
#
# Re-running replaces that schema (osm_<code>); the others stay. Refresh now
# and then (e.g. before adding a batch of regions): the data only has to be
# about current, not live.
set -euo pipefail
: "${ELEMENTS_DB:?set ELEMENTS_DB}"
# Prod via the VM with ELEMENTS_DB_VIA=user@host.
. "$(dirname "$0")/../tunnel.sh"
db_tunnel
CODE="${1:?extract code, e.g. de}"
PBF="${2:?path to the .osm.pbf}"
[[ "$CODE" =~ ^[a-z]{2,3}$ ]] || { echo "code: 2-3 lowercase letters"; exit 2; }
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${XDG_CACHE_HOME:-$HOME/.cache}/elements/bin"
IMPOSM="$(ls -d "$BIN"/imposm-*/imposm 2>/dev/null | head -1)"
[ -x "$IMPOSM" ] || { echo "imposm missing: run import-osm-imposm.sh once, it fetches it"; exit 1; }
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/elements/imposm/extract-$CODE"
SCHEMA="osm_$CODE"
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

echo ">> $PBF -> $SCHEMA $(date +%T)"
psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 -q -c "DROP SCHEMA IF EXISTS $SCHEMA CASCADE; CREATE SCHEMA $SCHEMA;"
"$IMPOSM" import -connection "$PGIS" -mapping "$HERE/mapping-osm-copy.yaml" \
  -read "$PBF" -write -srid 4326 -cachedir "$CACHE" -overwritecache \
  -dbschema-import "$SCHEMA" -quiet
echo ">> indexes, views $(date +%T)"
psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 -q <<SQL
CREATE INDEX IF NOT EXISTS ${SCHEMA}_ways_g ON $SCHEMA.osm_ways USING gist (geometry);
CREATE INDEX IF NOT EXISTS ${SCHEMA}_pois_g ON $SCHEMA.osm_pois USING gist (geometry);
CREATE INDEX IF NOT EXISTS ${SCHEMA}_water_g ON $SCHEMA.osm_water USING gist (geometry);
CREATE INDEX IF NOT EXISTS ${SCHEMA}_coast_g ON $SCHEMA.osm_coastline USING gist (geometry);
CREATE INDEX IF NOT EXISTS ${SCHEMA}_bound_g ON $SCHEMA.osm_boundaries USING gist (geometry);
CREATE INDEX IF NOT EXISTS ${SCHEMA}_bound_n ON $SCHEMA.osm_boundaries (name, admin_level);
CREATE INDEX IF NOT EXISTS ${SCHEMA}_trees_g ON $SCHEMA.osm_trees USING gist (geometry);
CREATE INDEX IF NOT EXISTS ${SCHEMA}_places_g ON $SCHEMA.osm_places USING gist (geometry);
CREATE INDEX IF NOT EXISTS ${SCHEMA}_closed_g ON $SCHEMA.osm_closed USING gist (geometry);
ANALYZE $SCHEMA.osm_ways; ANALYZE $SCHEMA.osm_pois; ANALYZE $SCHEMA.osm_boundaries;
SQL
psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 -q -f "$HERE/osm-views.sql"
rm -rf "$CACHE"
psql "$ELEMENTS_DB" -At -c "SELECT '$SCHEMA', (SELECT count(*) FROM $SCHEMA.osm_ways) AS ways, (SELECT count(*) FROM $SCHEMA.osm_pois) AS pois, pg_size_pretty(sum(pg_total_relation_size(c.oid))) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = '$SCHEMA'"
echo ">> done $(date +%T)"
