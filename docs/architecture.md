# Architecture

One Rust process serves every active city. Browsers play through a web gateway; each loaded region is its own single-threaded Bevy app, run with many others on a few worker threads (`pool.rs`); PostGIS is the source of truth for the world and the only store for scores and aggregates.

```
 phone / browser (web/, TypeScript + Vite + MapLibre, PWA)
    │  https  /ws (JSON)  /api/*  /basemap/*  /media/*
    ▼
 reverse proxy (TLS, no access log)   ──►  127.0.0.1:5080
    ▼
 ┌─ container: elements-server ───────────────────────────────────────────┐
 │ gateway.rs  axum + tokio in its own thread                              │
 │   routes by ?region=, limits per client (limits.rs), CSP/headers,       │
 │   serves web/dist and /srv/static, ops/analytics/export handlers,       │
 │   one WebSocket task per browser  ──FromWeb (RegionTx wakes)──► worker  │
 │                                                                          │
 │ regions.rs  Host: loads a region on first use, unloads after idle,      │
 │   rereads geo.regions every 5 min, caps loaded regions                   │
 │ pool.rs  loader threads (prepare: DB, files, tiles) ──► worker threads  │
 │   each worker owns many region apps and updates one when a message      │
 │   arrives, ~16 Hz while someone is connected, 2 Hz when not             │
 │   ┌─ region app (one Bevy App, SingleThreaded executor) ──────────────┐ │
 │   │ db.rs load graph ──► sim.rs field (shared::field) ◄── rules.toml   │ │
 │   │ web.rs inbox: hello/pos/claim/… ──► actions, motion, plausible,    │ │
 │   │   keep, hinterland, challenge, campaign, finds, score, analytics   │ │
 │   │ web.rs broadcast: state/tiles/sources/totals back to the sockets   │ │
 │   └──────── lanes ──► writers.rs: shared writer threads ───────────────┘ │
 │     (ELEMENTS_DB_THREADS, default 4, one DB connection each; per region │
 │      a lane each for scores, analytics, keeps, campaign)                │
 │ shards.rs  outbox thread: signed events to the other shards             │
 └──────────────────────────────────────────────────────────────────────────┘
    ▼ postgres (TLS)                          ▲ HTTPS between shards
 PostGIS: geo.* (graph, regions, finds, zones), game.* (players, keeps,       
       chapters), analytics.* (k-anonymous aggregates), ops.* (tokens)      
```

## Pieces

- **Web app** (`web/`): MapLibre map with the street graph as tiles from the gateway (`tiles.rs`: nodes, edges and block outlines per z15 tile; a block's border junctions travel with its tile, so the app fills held blocks from the node states it already gets), the browser's GPS once a second over the socket, claims by tap. Region chosen on the device from `GET /api/regions?v=2` outlines (packed integer steps, ETag). Seven languages (de, en, fr, es, pl, it, pt). Rain and cloud map layers from the weather host (`radar.ts`), only when the server names one. No third-party requests in built apps (basemap, fonts, sprites from `/basemap`). Modules mirror a server file each (header comment). `docs/web-dev.md`.
- **Gateway** (`crates/server/src/gateway.rs`): HTTP + WebSocket/JSON. Protocol in `protocol.rs`; `web/src/protocol.ts` is generated from it by ts-rs (`UPDATE_PROTOCOL=1 cargo test -p elements-server protocol`; a test and CI fail on drift). Per-client token buckets (`limits.rs`, IPv4 or IPv6 /64 from Caddy's `X-Forwarded-For`; open sockets also per /48), `/ws` and `/api/session` only from this site's pages (`Origin`/`Sec-Fetch-Site`), proof of work for new identities (`pow.rs`), bearer ops tokens (`ELEMENTS_OPS_TOKEN` or `ops.tokens`). Analytics and export handlers read the DB through `AppState::with_db` on blocking tasks.
- **Region apps** (`regions.rs`, `main.rs::run_region`): the Bevy schedule is one explicit `.chain()` in `main.rs`: inbox → hotspots → hinterland → broadcast → overview → analytics flush → scores → siege → challenges → campaign → live rain/sun → weather → keeps → scores out. Each region has a `Clock` (`clock.rs`, `geo.regions.timezone`): points per day, "today", streaks, daily limits and analytics days change at local midnight. Day numbers still count from 1970-01-01. Everything a player does enters through `web::inbox` as a `FromWeb` message; replies go back through the socket's `Outbox`. A region stops when idle (`ELEMENTS_IDLE_MIN`); `ELEMENTS_REGION` and `ELEMENTS_PRELOAD` stay.
- **Rules** (`crates/shared`): the field spread (`field.rs`, CSR graph with origin tracing), claims and decay (`rules.rs`), a source's life from claim to fall (`source.rs`: strength over age, takeover, top-up, natural seeds, who holds what), hinterland sparks/supply/keeps (`hinterland.rs`), levels and abilities (`progression.rs`), weather (`weather.rs`: the day's weather from Open-Meteo, the sun's elevation), radar values (`radar.rs`: RAD2/CLD1 H3 blocks; server side `radar.rs` reads them every minute, `live_rain.rs` turns rain and clear sky per junction into ageing and natural strength), weekly streaks (`streak.rs`, `[streak]`; the city streak counts days with k players in `game.faction_day`), points (`scoring.rs`), encirclement of permanent sources (`[siege]`, server `siege.rs`), reinforcing at the front (`[reinforce]`, server `actions.rs`), new players to the weaker side with `join_weaker` (0.67), blocks (`rules.rs` `[area]`: who holds a block, area points; server side `area.rs`: once a minute in the scores tick, the ageing factor for border sources goes to `sim::expire_sources`). Values come from `rules/default.toml` (`ELEMENTS_RULES`, per region `geo.regions.rules`, overrides `ELEMENTS_RULES_OVERRIDE`).
- **Database**: Flyway migrations in `db/migrations` (`db/flyway.sh`, test database first). Import pipeline in `db/import` (imposm from a local OSM copy, terrain from Copernicus GLO-30 or local rasters, natural sources, blocks as faces of the street graph; `new-region.sh` does it in one go). Roles: `elements_app` for the containers, `elements_owner` for migrations and imports, `elements_read` for analysis.
- **Shards**: one database and one container per continent (`eu`, `sa`, `na`). A player belongs to the shard they started on; switching carries a signed passport; badges and deletions travel as events (`shards.rs`, `ELEMENTS_SHARD_KEY`; calls signed with time and nonce, replays refused).
- **Globe and weather files**: `marble.rs` publishes each shard's game state as H3 cells for a globe (`GFL1`); `radar.rs` reads rain (`RAD2`) and cloud cover (`CLD1`) from `ELEMENTS_WEATHER_URL`. Formats in `docs/data-files.md`.
- **Deploy**: one image (server + built web app + rules, `deploy/Dockerfile`); basemap and videos on the host, mounted as `/srv/static`. `docs/deploy.md` describes running an instance, `docs/configuration.md` lists the settings.

## Threads and database connections

| Thread | Owns | Connection |
| --- | --- | --- |
| gateway (tokio) | sockets, HTTP, limits, demo cache | `with_db`: a new connection per blocking call (lookups, passports, feedback, wishes, exports, analytics) |
| `pool.rs` loaders (`loader-N`, `ELEMENTS_LOADERS`, default 4) | `prepare`: rules, scores, keeps, graph, zones, finds, tiles of a region being loaded | a new one per load (`db::load_region_data`, scores, keeps) |
| `pool.rs` workers (`worker-N`, `ELEMENTS_WORKERS`, default cores − 1) | the region apps (world, players, sim, aggregator), each pinned to one worker; a panic unloads only its region | none directly, only channels |
| `writers.rs` (`db-writer-N`, `ELEMENTS_DB_THREADS`, default 4) | all regions' lanes: `score::Writer` (`game.players`, `game.player_region`, badges, finds, presence, daily sweep), `analytics::Writer` (`analytics.*`), `keep::Writer` (`game.keeps`, `game.keep_pot`, campaign boundary every 5 min), `campaign::Worker` (`game.chapters`, `game.chapter_progress`, weather day, reload every 5 min) | one per thread, reconnects on error |
| `shards::spawn_outbox` | `game.shard_outbox` → other shards over HTTPS | one |
| `regions::Host` list refresh | `geo.regions` every 5 min | one per refresh |
| `marble` | globe files every 5 min, S3 upload (`ELEMENTS_MARBLE_S3_*`) | none |
| `radar`, `clouds` | the shard's newest rain and cloud frames every minute (`ELEMENTS_WEATHER_URL`) | none |
| `<rain\|sun>-cells` | short-lived, per region: junction → H3 cell for live rain and sun | none |

Each region's writer of one kind is a lane: pinned to one writer thread by region and kind (so its writes keep their order), run when something is sent to it or its timer is due, dropped when its region unloads. All writers take batches over a channel and acknowledge a `Barrier` at shutdown (`--stop-timeout 30`, `docs/deploy.md`), so a redeploy loses nothing. Database connections: the writer threads plus short ones from the gateway, however many regions are loaded.

## Privacy by construction

Positions live only in memory on the player's entity and are gone with the socket. Analytics snap fixes to edges and H3 cells and keep counts per 15-minute window, written only when at least k persons are in a window, with fixed keyed noise and release the next day (`analytics.rs`, `db.rs`). Leftovers below k pool into the hour and the region's local day. Scores are keyed by the SHA-256 of a random 128-bit browser token; the ops dashboard sees aggregates only. The IP is used for rate limits in memory and never stored (Caddy has no access log). What the web app promises is in `web/datenschutz.html`; a code review on 2026-10-06 checked it line by line against the code.

## Clients

The web app is the client. A native app would speak the same WebSocket/JSON protocol (`protocol.rs`, `docs/api.md`); none is part of this repository.
