# Public data files: globe and weather

Elements exchanges plain data files with two outside parts: a 3D globe that shows the game state, and a weather pipeline whose data the game reads. Nothing else couples them: no shared code, no API calls. This page describes the files as Elements writes and reads them.

Everything here is **optional** and off by default. Without the bucket key, the server keeps the globe files to itself (`/api/marble/…`). Without `ELEMENTS_WEATHER_URL` the server reads no weather, the app shows no weather buttons, and the page allows no connection to a weather host. A self-hosted Elements needs none of it.

**The weather files of the Glut & Flut instance are not open data.** They are processed for that instance and are subject to their provider's terms; using them with another instance, through a proxy, or redistributing them needs the provider's permission. The formats below are documented so that the code is understandable and so that anyone can provide their own data in the same format, for example from the original sources listed at the end.

| Direction | Files | Where | Code |
| --- | --- | --- | --- |
| out | game state per region (`GFL1`), shard index | the globe's bucket (`ELEMENTS_MARBLE_S3_*`), prefix `glutflut/` | `crates/server/src/marble.rs` |
| in | rain (`RAD2`): map tiles and values per H3 cell | `<ELEMENTS_WEATHER_URL>/radar/v2/` | `web/src/radar.ts`, `crates/server/src/radar.rs`, `crates/shared/src/radar.rs` |
| in | cloud cover (`CLD1`): map tiles and values per H3 cell | `<ELEMENTS_WEATHER_URL>/clouds/v1/` | `web/src/radar.ts`, `crates/server/src/radar.rs`, `crates/server/src/live_rain.rs` |

Conventions for all of them:
- Times are UTC, in 15-minute steps aligned to :00, :15, :30 and :45, with a 72-hour window.
- Data files carry a content hash in their name and never change (`Cache-Control: immutable`). Manifests are overwritten and cached briefly; a reader loads the manifest first, then the files it names.
- Files may be stored gzip-encoded (`Content-Encoding: gzip`). Browsers decode them; other readers should check for the gzip signature `1f 8b`.
- `bounds` are `[west, south, east, north]` in degrees.
- Readers ignore unknown fields.

## Game state for the globe (`GFL1`, written by Elements)

Every 5 minutes each shard (`eu`, `na`, `sa`, …) computes, for every listed region, the share of junctions held by fire and by water per H3 cell at resolutions 5 to 9. It keeps 288 bins of 15 minutes (72 h) and uploads what changed.

```
glutflut/shards.json                              shard index (written by eu)
glutflut/<shard>/manifest.json                    per shard, max-age=60
glutflut/<shard>/<region-slug>/r<res>_<hash>.bin  per region and resolution, immutable
```

Old data files are deleted 10 minutes after a newer manifest is up, so a reader should reload the manifest when a file returns 404.

### Values per cell and bin

| Channel | Meaning | Range |
| --- | --- | --- |
| `fire` | share of the cell's junctions held by fire | 0..255 (255 = all) |
| `water` | share held by water | 0..255 |
| `front` | share contested | 0..255 (the rest up to 255 is neutral) |
| `act` | claims in this cell and bin | 0..65535 |

**Privacy:** `act` is 0 unless at least 5 different players are behind it, and it only exists at resolutions 5 to 7 (cell edge 1.2 km or more). Territory changes show bin by bin only while at least 5 different players claim in the region; in a quiet region they are held back and appear once a day at 04:00 local time, so a lone player's walk can't be read back as a trail with times. There are no positions, routes or player identities in these files; territory is game state.

### File layout (little-endian, run-length encoded)

```
Header (24 bytes):
  u8[4] magic "GFL1"
  u8    h3_resolution
  u8    flags             (bit 0: act present)
  u16   bin_minutes       (15)
  u16   bin_count         (288)
  u16   reserved (0)
  u32   start_unix_min    (start of bin 0, minutes since 1970, UTC)
  u32   cell_count
  u32   run_count         (sum of all runs, for pre-allocation)
Cells, cell_count times:
  u64   h3_index
  u16   nodes             (junctions in the cell, capped at 65535; a weight)
  u16   runs
  runs times:
    u16 start_bin         (the run lasts until the next run or bin_count)
    u8  fire
    u8  water
    u8  front
    u8  reserved
    u16 act               (only if flags bit 0)
```

- A run that began before the window has `start_bin = 0`.
- Bins before a cell's first run have no data (the region joined later).
- An idle region keeps its last run until the end of the window.
- Cells without junctions are left out.

### Manifest

```json
{
  "format": "GFL1",
  "shard": "eu",
  "generated": "2026-10-08T10:15:00Z",
  "bin_minutes": 15, "bin_count": 288, "start": "2026-10-05T10:30:00Z",
  "resolutions": [5, 6, 7, 8, 9],
  "regions": [
    {
      "slug": "region-konstanz", "name": "Konstanz", "language": "de",
      "center": [9.17, 47.66], "bounds": [9.0, 47.6, 9.3, 47.75],
      "play": "https://glutflut.com/?region=region-konstanz",
      "updated": "2026-10-08T10:15:00Z",
      "files": { "5": "region-konstanz/r5_3fa9c2e1.bin", "9": "region-konstanz/r9_77b0d4a2.bin" }
    }
  ]
}
```

File paths are relative to `glutflut/<shard>/`. All regions of a shard share `start` and the bins, and `start` is always on a bin boundary.

### Shard index

The eu server also writes `glutflut/shards.json` (max-age=300), so that a new shard appears without changes on the reading side:

```json
{
  "format": "glutflut-shards",
  "version": 1,
  "shards": [
    { "shard": "eu", "manifest": "eu/manifest.json", "play": "https://glutflut.com" },
    { "shard": "na", "manifest": "na/manifest.json", "play": "https://na.glutflut.com" }
  ]
}
```

`manifest` is relative to `glutflut/`.

### Configuration and local testing

`ELEMENTS_MARBLE_S3_ENDPOINT`, `_BUCKET`, `_KEY_ID`, `_SECRET` and `_REGION` enable the upload (see `deploy.md`). `ELEMENTS_MARBLE_UNLISTED=1` also publishes beta regions. Without the upload settings, the same files are served at `/api/marble/manifest.json`, `/api/marble/shards.json` and `/api/marble/<slug>/r<res>_<hash>.bin`.

## Weather (read by Elements)

Rain and cloud cover come in two forms:
- **Tiles** for the map: XYZ tiles per data source.
- **Values** for the game: one byte per H3 cell, per shard.

Both use 15-minute frames over 72 hours, with times as the valid time of the measurement.

### Byte codes

| Product | `0` | `1..` | `255` |
| --- | --- | --- | --- |
| rain (`RAD2`) | dry | `1..253` rain, mm/h = 0.1 · 10^((v − 1) / 72) (0.1 to ~320 mm/h); `254` reserved | no data |
| clouds (`CLD1`) | clear | `1..100` cloud cover in % | no data |

"No data" is never "dry" or "clear". A frame without any measurement is missing from the manifest altogether.

### Tiles (map layer)

```
<product>/tiles/sources.json
<product>/tiles/<source>/manifest.json
<product>/tiles/<source>/<YYYYMMDDTHHMM>_<hash>/{z}/{x}/{y}.png
```

`<product>` is `radar/v2` or `clouds/v1`.

- Tiles are 256 px, EPSG:3857, as **8-bit greyscale PNGs holding the byte codes**. The app colours them itself, through a MapLibre protocol per product (`radar://`, `clouds://`).
- **A missing tile (404) means nothing to draw:** all dry or clear, or no data.
- Coarser zoom levels keep the highest rain value of each 2×2 block (so showers don't vanish when zooming out), and the mean for clouds.
- Rain goes up to the source's native resolution (z6 or z7); clouds to z5. MapLibre enlarges beyond that.

`sources.json`:

```json
{
  "format": "RAD2-SOURCES",
  "sources": [
    { "source": "opera", "kind": "radar", "priority": 0, "bounds": [-39.6, 31.7, 57.9, 67.7], "max_zoom": 6, "manifest": "opera/manifest.json" },
    { "source": "mtg", "kind": "satellite", "priority": -10, "masked_by": ["opera"], "bounds": [-25, 26, 45, 72], "max_zoom": 6, "manifest": "mtg/manifest.json" }
  ]
}
```

- `priority`: the higher one is the better source where sources overlap.
- `kind`: `radar` or `satellite`. The app labels satellite-only views "Satellit" instead of "Radar".
- `masked_by`: this source's tiles are `255` wherever one of the named sources has data in the same frame, so the two can be stacked without doubling the colour.

The app's rule:
1. Take the sources whose `bounds` touch the view.
2. Leave out a source that a better one covers entirely, unless the better one is in its `masked_by`.
3. Stack the rest, the better source on top.

It re-checks this every 5 minutes and after the map stops moving.

Tile manifest:

```json
{
  "format": "RAD2-TILES", "source": "opera", "generated": "2026-10-09T19:25:30Z",
  "step_minutes": 15, "min_zoom": 3, "max_zoom": 6, "bounds": [-39.6, 31.7, 57.9, 67.7],
  "license": "CC BY 4.0", "attribution": "EUMETNET OPERA, processed by <provider>",
  "frames": [ { "time": "2026-10-09T19:00:00Z", "kind": "obs", "dir": "20261009T1900_f8ebfd62" } ]
}
```

- Frames are in ascending time order. The app takes the newest with `kind: "obs"`; other kinds (a future nowcast) are ignored.
- `attribution` and `license` are shown in the map credits.
- For clouds the formats are `CLD1-SOURCES` and `CLD1-TILES`, with the same layout.

### Values (game rules)

```
<product>/values/<shard>/manifest.json
<product>/values/<shard>/blocks_<hash>.bin
<product>/values/<shard>/v_<YYYYMMDDTHHMM>_<hash>.bin
```

The values are not organised per region but in fixed **H3 blocks**: resolution-4 cells (`block_res`) with all their children at `cell_res` (8 for rain, 7 for clouds). The blocks are those that touch a region of the shard.

- `blocks_<hash>.bin`: a `u32` block count, then that many `u64` H3 indices in ascending order (little-endian).
- `v_….bin`: all blocks in that order; within each block, one byte per child cell, **with the children sorted by ascending H3 index**. A block has 7^(cell_res − 4) children, except pentagons.
- **Reading a point:**
  1. Point → H3 cell at `cell_res`.
  2. Cell → parent at resolution 4 → block number `b`.
  3. Position `k` of the cell among the block's sorted children.
  4. The byte at (children of all blocks before `b`) + `k`.

  `elements_shared::radar::Blocks` does this; the reader computes the children itself.
- One frame can combine several sources, taking each cell from the best source with data. `frames[].sources` names those that contributed. If a source arrives late, the frame is merged again and gets a new file name, and the server reloads when the newest frame's file name changes.
- A shard without any source still has a manifest, with an empty `frames`.

```json
{
  "format": "RAD2-VALUES", "shard": "eu", "generated": "2026-10-09T19:25:30Z",
  "step_minutes": 15, "block_res": 4, "cell_res": 8,
  "blocks": "blocks_9e9a84c3.bin", "block_count": 243,
  "frames": [ { "time": "2026-10-09T19:00:00Z", "kind": "obs", "sources": ["opera"], "values": "v_20261009T1900_af8825a6.bin" } ],
  "license": ["CC BY 4.0"], "attribution": ["EUMETNET OPERA, processed by <provider>"]
}
```

Clouds use `CLD1-VALUES` with the same layout.

### Use in Elements

- **Map:** two buttons, rain and clouds. Both are off by default and remembered in the browser. Clouds lie under the rain, and both lie under the game layers.
- **Switch:** `ELEMENTS_WEATHER_URL` is the data host (paths below it as above). Unset, everything weather-related is off.
- **Server:** `radar.rs` reads the newest rain and cloud frames of its shard every minute. The junction card shows "rain here now: x mm/h (radar 14:30)" or "dry".
- **Game rules** (`live_rain.rs`, on in `rules/default.toml`): where at least half of a region's junctions have values no older than an hour, rain per junction replaces the day's rain effect (`[weather] live_decay`, `live_natural`) and a clear sky by day replaces the day's sun (`sun_decay`, `sun_natural`; the sun's elevation over the region counts too).
- **CORS:** the weather host must allow the game's origin (only the origins the provider allows), and the page's CSP allows the host from `ELEMENTS_WEATHER_URL`.

### Data sources and credits

Credits come from each manifest's `attribution` and `license` (per source); the map shows them and the legal notice lists them. Elements adds nothing of its own. The software that prepares the files is not part of Elements.
