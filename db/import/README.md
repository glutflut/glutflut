# Region import

A region is a city's walkable street graph in `geo.*` (junctions, edges with
terrain grades), its outline, natural sources and blocks. These scripts are
data jobs, not migrations: run them against a database that Flyway has
set up (`db/flyway.sh bootstrap`, later `migrate`).

**Start here:** one command builds a region from a local OpenStreetMap copy and
Copernicus GLO-30 terrain:

```sh
ELEMENTS_DB=… db/import/import-osm-extract.sh de ~/osm/germany-latest.osm.pbf   # once per country
ELEMENTS_DB=… db/import/new-region.sh region-freiburg "Freiburg im Breisgau" \
  '["name"="Freiburg im Breisgau"]["admin_level"="6"]'
```

A small town takes under a minute. Then the basemap for the region
(`web/scripts/fetch-basemap.sh` with `BBOX`, see `docs/deploy.md`),
and when it is ready:
`UPDATE geo.regions SET listed = true WHERE slug = 'region-freiburg';`

## Requirements

- `psql`, GDAL (`ogr2ogr`, `gdalbuildvrt`, `gdalwarp`, `gdallocationinfo`),
  `python3`; `imposm` is fetched to `~/.cache/elements/bin`.
- PostgreSQL with `postgis` and `pgrouting` (`h3` is created by `V1__init.sql`).
- Outbound HTTPS to Geofabrik (or another PBF source), to
  `copernicus-dem-30m.s3.amazonaws.com` (public, no credentials) and, for
  regions outside the local copy, to Overpass.

## The pieces

| Script | What |
| --- | --- |
| `import-osm-extract.sh <code> <pbf>` | Loads a Geofabrik extract with imposm (`mapping-osm-copy.yaml`: walkable ways, POIs, named trees, water, coastline, admin boundaries, places) into schema `osm_<code>`; `osm-views.sql` joins all copies as views in `osm`. Refresh now and then by running it again. |
| `new-region.sh <slug> "<name>" '<area>'` | Region row (random id, next free node id block per `SHARD`), graph, terrain, sources, blocks. Starts active and unlisted. |
| `extend-region.sh <slug> '<areas>'` | Grows a region by more admin areas, keeping ids, players and history (junctions within 1 m keep their ids, vanished ones go inactive). |
| `import-osm-imposm.sh` | The graph: a junction is a vertex several ways share (or a way's end); ways split there. Bridges and tunnels stay apart; scales to whole cities. Reads the PBF given in `OSM_FILE`/`OSM_CLIP` or the local copy. |
| `import-terrain-region.sh` | Samples ground elevation onto the junctions (Copernicus GLO-30 via `/vsicurl`, or your own rasters in `DEM_FILES`), then edge grades and directional fire/water costs (`terrain-derive.sql`). |
| `simple-sources.sql` | Natural sources: fire on the high third of the junctions, water on the low third, spread by k-means. A starting point; place them by hand or with your own script for a better game. |
| `import-blocks.sh` | `geo.blocks`: faces of the street graph with their border junctions (area rules). |
| `closed-areas.sql` | Streets nobody may simply walk on stay out (private ways, fee-paying parks, zoos, military land …); hand lists in `closed/` (`ELEMENTS_CLOSED_DIR`). |
| `drop-region.sql` | Deletes a region and everything keyed to it. |

## Options

- Areas are Overpass-style filters; join several with `;area`, e.g. a twin
  city: `'["name"="Görlitz"]["admin_level"="8"];area["name"="Zgorzelec"]["admin_level"="7"]'`.
  Names repeat: the scripts stop unless each area matches exactly one
  boundary; `OSM_PARENT` (same syntax) keeps only the areas inside it.
- `LANGUAGE` (de), `TIMEZONE` (Europe/Berlin), `SHARD` (eu, na, sa: picks the
  node id band, so regions can move between databases).
- Finer terrain: `DEM_FILES="$(ls dem/*.tif)" DEM_SRS=EPSG:25832 DEM_RES_M=5`
  is sampled after GLO-30 (junctions outside it keep the GLO-30 height).
  GLO-30 is a surface model (roofs, trees); a 1 m terrain model gives better
  street grades where one is available under an open licence. Credit it on the
  legal page.
- `ELEMENTS_REGION_HOOK=<script>`: your own steps instead of
  `simple-sources.sql` (sources, zones, finds, other terrain). It runs after
  the terrain as `<script> new|extend <slug> '<areas>' [extra args]` with
  `ELEMENTS_DB`, `REGION_UUID`, `NODES` (and `NEW_SINCE` for `extend`) set;
  extra arguments after the area are passed through.
- Re-running `new-region.sh` redoes the region; players and results stay.
  `extend-region.sh` with `NEW_SINCE=<time of the first run>` resumes after the
  graph step.
- Through an SSH host (a database that only admits that host):
  `ELEMENTS_DB_VIA=user@host`; every script tunnels, sub-scripts reuse the
  tunnel (`db/tunnel.sh`; `db/via.sh <command>` for anything else).

## Terrain model

Grade is `Δelev / max(length, DEM_RES_M)`, clamped to ±`MAX_GRADE`. Costs tilt
by grade (`base_travel_cost × clamp(1 ∓ k·grade, COST_MULT_MIN,
COST_MULT_MAX)`): fire spreads more cheaply uphill, water downhill, neither is
ever blocked. Override via env: `MARGIN_DEG`, `DEM_RES_M`, `MAX_GRADE`,
`GRADE_K_FIRE`, `GRADE_K_WATER`, `COST_MULT_MIN`, `COST_MULT_MAX`. Only the
values land in `geo.*`; no raster is stored.

## Zones and finds

The server also reads `geo.zones` (named quarters for the dashboard) and
`geo.finds` (named places to discover). The public import doesn't fill them;
add them in the editor (`/editor.html`, "Orte") or with your own hook.
