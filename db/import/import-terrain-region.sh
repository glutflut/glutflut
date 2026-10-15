#!/usr/bin/env bash
# Sample terrain elevation onto an already-imported region's road graph, and
# derive the elevation-driven edge attributes the element simulation (S3) needs.
# The terrain half of the import (new-region.sh runs it after the graph):
# it fills columns the schema already has (geo.nodes.elevation_m and
# geo.edges.{elev_from_m,elev_to_m,grade_forward/reverse,
# water_cost_forward/reverse,fire_cost_forward/reverse,base_travel_cost}).
#
# Pipeline:
#   read the region's bounds from geo.regions  ->  mosaic the Copernicus GLO-30
#   DEM tiles covering it (read remotely via /vsicurl, no full download) and clip
#   to bounds+margin  ->  gdallocationinfo samples each node's ground elevation
#   locally  ->  the samples land in a terrain_import staging table  ->  edge
#   grades and directional water/fire costs are derived from the node
#   elevations  ->  the staging schema is dropped (elevation is materialized
#   into geo.*; the raster never goes to the database).
#
# Idempotent per region: re-running re-samples and overwrites the region's rows.
#
# Requires: gdalbuildvrt/gdalwarp/gdallocationinfo (GDAL) + psql on PATH; a
# Postgres with postgis; outbound HTTPS to the AWS Open Data bucket
# `copernicus-dem-30m` (public, no credentials). The region must already exist
# (import-osm-imposm.sh, or new-region.sh, first).
#
# Usage (defaults: region-konstanz):
#   ELEMENTS_DB='postgresql://…' db/import/import-terrain-region.sh
#   # or: just import-terrain
#
# NEW_SINCE=<timestamp>: sample only the nodes added since (extend-region.sh).
#
# Local DEM instead of Copernicus: set DEM_FILES to a space-separated list of
# raster files GDAL can read (e.g. a 1 m terrain model as GeoTIFF or XYZ tiles)
# and DEM_SRS to their CRS if the files don't carry one (XYZ doesn't):
#   DEM_FILES="$(ls /path/to/dem/*.tif)" DEM_SRS=EPSG:25832 DEM_RES_M=5 \
#     db/import/import-terrain-region.sh
# A terrain model (DTM) is ground only, unlike the Copernicus DSM, which
# includes buildings and trees.
#
# Override via env: REGION_SLUG, MARGIN_DEG, and the cost-model params
#   GRADE_K_FIRE / GRADE_K_WATER (grade sensitivity) and COST_MULT_MIN /
#   COST_MULT_MAX (clamp — keeps costs "preferred, not strict": never 0, never
#   blocking).
set -euo pipefail
# awk/printf must write '.' decimals regardless of the caller's locale (a
# de_DE locale turns 9.17 into "9,17", which GDAL only accepts by accident).
export LC_ALL=C

: "${ELEMENTS_DB:?set ELEMENTS_DB to the Postgres connection string}"
# Prod via the VM with ELEMENTS_DB_VIA=user@host.
. "$(dirname "$0")/../tunnel.sh"
db_tunnel
REGION_SLUG="${REGION_SLUG:-region-konstanz}"
MARGIN_DEG="${MARGIN_DEG:-0.01}"           # ~1.1 km pad so boundary nodes sample cleanly
# Grade model. The 30 m Copernicus DSM can't resolve slope on a few-meter noding
# segment (and its rooftop bias makes raw short-edge grades wild — up to ~250%),
# so grade is resolved over no less than the DEM footprint (`Δelev / max(len,
# DEM_RES_M)`) and hard-clamped to ±MAX_GRADE.
DEM_RES_M="${DEM_RES_M:-30}"
MAX_GRADE="${MAX_GRADE:-0.40}"
# Cost model (see derive step). Defaults: at a 5% grade, the favoured element is
# ~20% cheaper and the opposed ~20% dearer, clamped to ±50%.
GRADE_K_FIRE="${GRADE_K_FIRE:-4.0}"
GRADE_K_WATER="${GRADE_K_WATER:-4.0}"
COST_MULT_MIN="${COST_MULT_MIN:-0.5}"
COST_MULT_MAX="${COST_MULT_MAX:-1.5}"
# Copernicus DEM GLO-30 (30 m DSM), public COGs on AWS Open Data.
DEM_BASE="${DEM_BASE:-https://copernicus-dem-30m.s3.amazonaws.com}"
# Optional local DEM (see header). Empty = Copernicus.
DEM_FILES="${DEM_FILES:-}"
DEM_SRS="${DEM_SRS:-}"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"; db_tunnel_close' EXIT

echo ">> [1/5] resolve region bounds for '$REGION_SLUG'"
read -r RID WMIN SMIN EMAX NMAX < <(
  psql "$ELEMENTS_DB" -At -F' ' -v ON_ERROR_STOP=1 -c \
    "SELECT id, bounds_min_lon, bounds_min_lat, bounds_max_lon, bounds_max_lat \
     FROM geo.regions WHERE slug = '$REGION_SLUG' AND active = true"
)
[ -n "${RID:-}" ] || { echo "no active region '$REGION_SLUG' — import its graph first" >&2; exit 1; }
echo "   region $RID  bounds: lon[$WMIN,$EMAX] lat[$SMIN,$NMAX]"

# Margin'd clip window + the integer 1°x1° Copernicus tiles it spans.
read -r CW CS CE CN < <(awk -v w="$WMIN" -v s="$SMIN" -v e="$EMAX" -v n="$NMAX" -v m="$MARGIN_DEG" \
  'BEGIN{ printf "%.6f %.6f %.6f %.6f\n", w-m, s-m, e+m, n+m }')
TILES=()
if [ -n "$DEM_FILES" ]; then
  # Space- or newline-separated (`$(ls …)` gives lines).
  read -r -d '' -a TILES <<< "$DEM_FILES" || true
else
for lon in $(awk -v a="$CW" -v b="$CE" 'BEGIN{for(x=int(a<0?a-1:a);x<=int(b<0?b-1:b);x++)print x}'); do
  for lat in $(awk -v a="$CS" -v b="$CN" 'BEGIN{for(y=int(a<0?a-1:a);y<=int(b<0?b-1:b);y++)print y}'); do
    ns=$(awk -v v="$lat" 'BEGIN{printf "%s%02d", (v<0?"S":"N"), (v<0?-v:v)}')
    ew=$(awk -v v="$lon" 'BEGIN{printf "%s%03d", (v<0?"W":"E"), (v<0?-v:v)}')
    name="Copernicus_DSM_COG_10_${ns}_00_${ew}_00_DEM"
    TILES+=("/vsicurl/${DEM_BASE}/${name}/${name}.tif")
  done
done
fi
echo ">> [2/5] DEM tiles covering region: ${#TILES[@]}"
printf '   %s\n' "${TILES[@]}"

echo ">> [3/5] mosaic + clip to bounds+margin (lon[$CW,$CE] lat[$CS,$CN])"
# Areas without a tile must be nodata, not 0 (a 0 would count as a sample). Nodes
# without a sample keep their height, so a coarse DEM first and a finer one after
# fills the gaps of the finer one.
if [ -n "$DEM_FILES" ]; then
  # All Float32: the XYZ driver reads a tile of whole numbers (lake) as Int16,
  # and gdalbuildvrt skips tiles of another type.
  for i in "${!TILES[@]}"; do
    gdal_translate -q -ot Float32 -of VRT "${TILES[$i]}" "$WORK/t$i.vrt"
    TILES[$i]="$WORK/t$i.vrt"
  done
fi
gdalbuildvrt -q ${DEM_SRS:+-a_srs "$DEM_SRS"} -vrtnodata -9999 "$WORK/dem.vrt" "${TILES[@]}"
if [ -n "$DEM_FILES" ]; then
  # Local tiles: sample the mosaic as is; warping a whole city at 1 m
  # would write gigabytes.
  ln -s "$WORK/dem.vrt" "$WORK/dem_clip.tif"
else
  gdalwarp -q -overwrite -t_srs EPSG:4326 -te "$CW" "$CS" "$CE" "$CN" -r bilinear \
    -dstnodata -9999 -co COMPRESS=DEFLATE "$WORK/dem.vrt" "$WORK/dem_clip.tif"
fi

echo ">> [4/5] sample node elevations locally -> terrain_import.node_elev"
# Sampling happens here with gdallocationinfo, not in the database: only the
# per-node values travel to Postgres (no raster upload, no raster2pgsql).
psql "$ELEMENTS_DB" -At -F' ' -v ON_ERROR_STOP=1 -c \
  "SELECT id, lon, lat FROM geo.nodes WHERE region_id = '$RID' ${NEW_SINCE:+AND created_at >= '$NEW_SINCE'} ORDER BY id" > "$WORK/nodes.txt"
# Exits 1 when a point is off the raster (still prints an empty line for it).
cut -d' ' -f2,3 "$WORK/nodes.txt" \
  | gdallocationinfo -valonly -wgs84 "$WORK/dem_clip.tif" > "$WORK/values.txt" || true
[ "$(wc -l < "$WORK/nodes.txt")" -eq "$(wc -l < "$WORK/values.txt")" ] \
  || { echo "sample count mismatch (nodes vs. values)" >&2; exit 1; }
# Keep only real samples: outside the raster or on nodata stays unset, and so
# does anything no ground can be (an undeclared nodata, e.g. -3.4e38, or a
# blend with one).
paste -d' ' <(cut -d' ' -f1 "$WORK/nodes.txt") "$WORK/values.txt" \
  | awk '$2 != "" && $2 != -9999 && $2 > -500 && $2 < 9000 { print $1 "," $2 }' > "$WORK/node_elev.csv"
echo "   sampled $(wc -l < "$WORK/node_elev.csv") of $(wc -l < "$WORK/nodes.txt") nodes"
psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 -q \
  -c "DROP SCHEMA IF EXISTS terrain_import CASCADE; CREATE SCHEMA terrain_import;" \
  -c "CREATE TABLE terrain_import.node_elev (id bigint PRIMARY KEY, elev double precision);" \
  -c "\\copy terrain_import.node_elev FROM '$WORK/node_elev.csv' WITH (FORMAT csv)"

echo ">> [5/5] sample node elevation + derive edge grades and costs"
psql "$ELEMENTS_DB" -v ON_ERROR_STOP=1 \
  -v rid="$RID" -v kfire="$GRADE_K_FIRE" -v kwater="$GRADE_K_WATER" \
  -v cmin="$COST_MULT_MIN" -v cmax="$COST_MULT_MAX" \
  -v demres="$DEM_RES_M" -v maxgrade="$MAX_GRADE" \
  -v derive="$(dirname "$0")/terrain-derive.sql" -v scope=all <<'SQL'
BEGIN;
-- Nodes: ground elevation = DEM value under the node (sampled in step 4).
UPDATE geo.nodes n
SET elevation_m = round(s.elev::numeric, 2)
FROM terrain_import.node_elev s
WHERE n.id = s.id AND n.region_id = :'rid';

\i :derive
COMMIT;

DROP SCHEMA IF EXISTS terrain_import CASCADE;

-- Report.
SELECT
  count(*)                                   AS nodes,
  count(*) FILTER (WHERE elevation_m <> 0)   AS with_elev,
  round(min(elevation_m)::numeric, 1)        AS min_m,
  round(max(elevation_m)::numeric, 1)        AS max_m,
  round(avg(elevation_m)::numeric, 1)        AS avg_m
FROM geo.nodes WHERE region_id = :'rid';

SELECT
  count(*)                                          AS edges,
  count(*) FILTER (WHERE grade_forward IS NOT NULL) AS with_grade,
  round((100*max(abs(grade_forward)))::numeric, 1)  AS max_grade_pct
FROM geo.edges WHERE region_id = :'rid';
SQL

echo ">> done — reload with: ELEMENTS_DB=… ELEMENTS_REGION=$REGION_SLUG just server"
