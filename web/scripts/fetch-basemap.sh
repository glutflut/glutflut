#!/usr/bin/env bash
# Self-hosted basemap: a Protomaps extract of the region plus fonts and
# sprites, into web/public/basemap/ (not in git). Re-run to refresh.
#   BBOX="minlon,minlat,maxlon,maxlat" BUILD=YYYYMMDD web/scripts/fetch-basemap.sh
# Several areas in one file (one image for several cities): separate with ";".
# PMTILES_OUT: the file to write (default <dir>/region.pmtiles); SKIP_ASSETS=1:
# no fonts and sprites, SKIP_EXTRACT=1: only those (docs/deploy.md uses both).
# BBOX=world: the whole earth (with MAXZOOM=6 the zoomed-out overview, ~45 MB).
set -euo pipefail

# BASEMAP_DIR: elsewhere (e.g. the host's static dir, docs/deploy.md).
DIR="${BASEMAP_DIR:-$(cd "$(dirname "$0")/.." && pwd)/public/basemap}"
# Default: Konstanz (with Kreuzlingen). Your regions: BBOX="w,s,e,n;w,s,e,n".
BBOX="${BBOX:-9.05,47.62,9.26,47.78}"
MAXZOOM="${MAXZOOM:-15}"
BUILD="${BUILD:-$(date -u -d yesterday +%Y%m%d)}"
PMTILES_VERSION=1.31.2
BIN="${XDG_CACHE_HOME:-$HOME/.cache}/elements/bin"

OUT="${PMTILES_OUT:-$DIR/region.pmtiles}"
mkdir -p "$DIR" "$BIN" "$(dirname "$OUT")"
if [[ ! -x "$BIN/pmtiles" ]]; then
  curl -sL "https://github.com/protomaps/go-pmtiles/releases/download/v${PMTILES_VERSION}/go-pmtiles_${PMTILES_VERSION}_Linux_x86_64.tar.gz" \
    | tar -xz -C "$BIN" pmtiles
fi

if [ "${SKIP_EXTRACT:-}" != 1 ]; then
echo ">> tiles: build $BUILD, bbox $BBOX"
REGION="$(mktemp --suffix=.geojson)"
# Node, not Python: the image's build stage only has Node. BBOX by environment:
# as an argument, "-74.2,…" (west longitudes) reads as a node option.
[ "$BBOX" = world ] && WORLD_BBOX="-180,-85.05,180,85.05" || WORLD_BBOX="$BBOX"
BBOX="$WORLD_BBOX" node -e '
const polys = process.env.BBOX.split(";").map((b) => {
  const [w, s, e, n] = b.split(",").map(Number);
  return [[[w, s], [e, s], [e, n], [w, n], [w, s]]];
});
console.log(JSON.stringify({ type: "MultiPolygon", coordinates: polys }));
' >"$REGION"
# Sources in turn: the source.coop mirror of the Protomaps planet (a CDN, v4
# schema), then the daily builds (build.protomaps.com answers 500 or drops
# HTTP/2 streams now and then). extract can't resume: merged requests, a few
# attempts each; BASEMAP_THREADS (4) parallel downloads.
SOURCES="${BASEMAP_SRC:-https://data.source.coop/protomaps/openstreetmap/v4.pmtiles}"
for day in 0 1 2; do
  SOURCES="$SOURCES https://build.protomaps.com/$(date -u -d "${BUILD:0:4}-${BUILD:4:2}-${BUILD:6:2} -$day day" +%Y%m%d).pmtiles"
done
ok=
for src in $SOURCES; do
  for attempt in 1 2 3; do
    echo ">> $src, attempt $attempt"
    if "$BIN/pmtiles" extract "$src" "$OUT.part" \
         --region="$REGION" --maxzoom="$MAXZOOM" --download-threads="${BASEMAP_THREADS:-4}" --overfetch=0.2; then
      mv "$OUT.part" "$OUT"; ok=1; break 2
    fi
    sleep $((attempt * 20))
  done
done
rm -f "$REGION" "$OUT.part"
[ -n "$ok" ] || { echo "basemap extract failed"; exit 1; }

fi

if [ "${SKIP_ASSETS:-}" != 1 ]; then
echo ">> fonts + sprites"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
curl -sL https://codeload.github.com/protomaps/basemaps-assets/tar.gz/refs/heads/main | tar -xz -C "$tmp"
src="$tmp/basemaps-assets-main"
rm -rf "$DIR/fonts" "$DIR/sprites"
mkdir -p "$DIR/fonts" "$DIR/sprites"
cp -r "$src/fonts/Noto Sans Regular" "$src/fonts/Noto Sans Medium" "$src/fonts/Noto Sans Italic" "$src/fonts/OFL.txt" "$DIR/fonts/"
cp "$src"/sprites/v4/light* "$src"/sprites/v4/grayscale* "$src"/sprites/v4/dark* "$DIR/sprites/"
du -sh "$DIR"/*
fi
