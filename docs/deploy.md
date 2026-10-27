# Deploy

Running your own instance. One image (`deploy/Dockerfile`) holds the server and the built web app, no basemap. One container serves every active region: the app picks the city by the player's location. A reverse proxy (we use Caddy) terminates TLS and proxies to the container on localhost. Basemap and videos live on the host in a directory mounted into the container (see below), so a new city needs no new image. Every environment variable and rule switch: `docs/configuration.md`.


## What you need

- A Linux host with Docker (no compose), outbound access to the database.
- PostgreSQL with PostGIS, pgRouting and H3 (any provider or your own). Create the schema with `db/flyway.sh bootstrap` (Flyway in a container; `migrate` for later migrations) and import a region (`db/import/README.md`).
- Database roles: the containers connect as `elements_app`; `elements_owner` is for Flyway and imports; `elements_read` reads `geo` and `analytics` for analysis. Create them by SQL.
- A domain and a reverse proxy with TLS.

## Image

Build from the repository root, with podman or docker:

```sh
docker build -f deploy/Dockerfile -t elements \
  --build-arg ELEMENTS_VERSION="$(git describe --always --dirty=+) $(date -u +%F)" .
```

`ELEMENTS_VERSION` shows in the app and at `/api/version` (default `dev`). Credits of your own data (e.g. the terrain sources of your regions in the legal notice): put `<name>.<lang>.html` files in `web/credits.local/` before the build (`docs/configuration.md`, `ELEMENTS_CREDITS_DIR`). For an ARM (aarch64) host the same file cross-compiles on an x86 machine without emulation: `--platform linux/arm64` (not tried yet). The runtime stage runs no commands; CA certificates and the `elements` user come from the build stage.

Build where you like and copy the image over (`docker save elements | gzip | ssh user@host 'gunzip | docker load'`), so the host never compiles Rust.

## Host

Create `~/elements/.env` on the host from `deploy/.env.example` (`ELEMENTS_DB`, default `ELEMENTS_REGION`, `ELEMENTS_OPS_TOKEN`, `ELEMENTS_NOISE_KEY`, `ELEMENTS_PUBLIC_URL`, `ELEMENTS_SOURCE_URL`), then:

```sh
docker run -d --name elements --restart unless-stopped --stop-timeout 30 \
  -v ~/elements-static:/srv/static:ro \
  --log-opt max-size=10m --log-opt max-file=3 \
  --memory 2g --memory-swap 2g \
  --read-only --tmpfs /tmp --cap-drop ALL --security-opt no-new-privileges --pids-limit 512 \
  --env-file ~/elements/.env -p 127.0.0.1:5080:5080 elements
```

- `--stop-timeout 30`: time to write open analytics windows on a redeploy.
- `--memory`: regions load on request; over the cap the container restarts, not the host. A town takes tens of MB, a city of 700k junctions ~630 MB.
- The rest hardens it: no writes outside `/tmp`, no capabilities, bounded processes, capped logs (the server logs no IP addresses).
- To update: build, `docker rm -f elements`, run again.

AGPL: if you run a modified version for others, offer them its source. Set `ELEMENTS_SOURCE_URL` to your repository; the app links to it next to the version.

## Test instance

A second container from the same image for tests: a test database, `?sim` (tap to move) allowed, a yellow TEST badge in the game. It switches off the position check, so never point it at production.

- Its own `.env` with the test `ELEMENTS_DB`, `ELEMENTS_REGION`, its own `ELEMENTS_OPS_TOKEN`, `ELEMENTS_ALLOW_SIM=1`; run it under another name and port (`-p 127.0.0.1:5081:5080`).
- Testers without GPS: `https://dev.example.org/?sim&at=47.66038,9.17639`, then tap the map (`at` also picks the city; `?sim&region=<slug>`). `&side=fire|water` picks the side.

## Regions

Every region with `active = true` in `geo.regions` is playable. The app gets `GET /api/regions` (outlines), takes one location fix and tests it in the browser: inside a region it plays there, outside all it shows the "not here yet" page (explainer video, cities, "I want to play here"), without a position the city list. `?region=<slug>` picks a city directly (links, QR codes, dashboard).

- Database writes for all regions go through `ELEMENTS_DB_THREADS` writer threads (default 4, one connection each).
- Region apps run on `ELEMENTS_WORKERS` worker threads (default: cores − 1) and are loaded by `ELEMENTS_LOADERS` loader threads (default 4); a region without players costs almost nothing.
- A region loads when the first player opens it (seconds; about a minute for a city of 700k junctions) and unloads after `ELEMENTS_IDLE_MIN` (180) minutes without anyone. The default region and `ELEMENTS_PRELOAD` stay loaded. RSS after load: a town of 40k junctions ~70 MB, a city of 700k ~630 MB.
- New city: import it (`db/import/new-region.sh`), set `active = true`, then fetch its basemap (below). The server rereads the region list every 5 minutes; no redeploy.
- Unlisted regions: `geo.regions.listed = false` keeps a region off the public list and out of the location match; it plays only by link (`?region=<slug>`).
- Rules per region: `geo.regions.rules` = a file in `/srv/rules` without `.toml` (NULL = `ELEMENTS_RULES`). Set the column only once the file is in the deployed image: a missing file is logged at start, and the region then plays `ELEMENTS_RULES`.
- Days: everything day-related (points per day, "today", streaks, daily limits, analytics days) changes at local midnight of `geo.regions.timezone` (`clock.rs`; `TIMEZONE` in `new-region.sh`).
- Players are the same everywhere (side, name, XP, recovery code); points and rankings are per city (`game.player_region`).
- Wishes ("I want to play here"): `GET /api/wishes` with the ops token lists H3 cells (res 6) with counts.
- Feedback (settings → feedback): `curl -H "Authorization: Bearer $TOKEN" https://<host>/api/feedback[?region=<slug>]` lists the latest 500. The dashboard shows the same list for a token of all cities.
- More shards (e.g. one per continent, each with its own database): `geo.regions.shard` + `ELEMENTS_SHARD`/`ELEMENTS_SHARDS` and the same `ELEMENTS_SHARD_KEY` on all of them (see `.env.example`). Players keep side, name and badges across shards (passport, `server/src/shards.rs`).

### A new shard

1. **Database:** a new one for the shard, roles as above.
2. **Schema:** `db/flyway.sh bootstrap`. From then on every migration runs on every shard.
3. **Region rows:** every shard's database needs the `geo.regions` rows of all regions (copy the missing rows over); the graph only where the region runs. Each shard has its own node id band, picked by `SHARD` in `new-region.sh` (eu below 1e9, na 1e9–2e9, sa 2e9–3e9; a new shard needs its band there).
4. **DNS and proxy:** a name for the shard, a site block.
5. **Host:** a `.env` for the shard's container: `ELEMENTS_DB`, `ELEMENTS_SHARD=<shard>`, `ELEMENTS_REGION`, `ELEMENTS_SHARDS=<other>=https://…,…`, the shared `ELEMENTS_SHARD_KEY`, the same `ELEMENTS_NOISE_KEY`, its own `ELEMENTS_OPS_TOKEN`, `ELEMENTS_PUBLIC_URL`.
6. **Other shards:** add the new one to their `ELEMENTS_SHARDS`, redeploy them. CSP and region links follow from that; the web app has no shard list.
7. **Deploy:** the container as above, with the shard's `.env`; basemaps for its regions.
8. **Privacy:** name the new data location on your privacy page.

### Cells: several servers in one shard

When one container per shard isn't enough (RAM of the awake regions, CPU), a shard gets more servers ("cells"). Players still see one address per shard; the proxy routes by `?region=`.

- **Data:** `geo.regions.cell` (default `main`). A server runs with `ELEMENTS_CELL` (default `main`) and loads only the regions of its shard and cell. A request for another cell's region (proxy not updated yet) gets 503 "loading", the app asks again. Group neighbours in one cell: they show each other on the map and players cross over.
- **Cells and upstreams** in `ops.cells`: one row per cell, `INSERT INTO ops.cells (shard, cell, upstream) VALUES ('eu', 'main', '127.0.0.1:5080'), ('eu', 'b', '127.0.0.1:5090')`.
- **Main cell:** requests without a region (region list, session, passport, pages) go to the cell named `main` (proxy). The shard-wide jobs (globe manifest, shard index, world ranking, cleanup) run in the cell holding the **main lease** (`ops.main_lease`, 30 min, renewed with the region leases; any cell takes it once it is free). A single cell needs no lease: it is main by name. The outbox to other shards runs in every cell (rows locked while sent). `ELEMENTS_REGION` of each cell must be one of its own regions (health check).
- **Bus** (`ops.cell_events`): deleting a player, a new player id and "write the players now" reach the other cells too; polled only while the shard has more than one cell. One-time checks in `ops.used_once`.
- **Leases** (`ops.region_lease`): a cell loads a region only while no other cell holds it. A lease lasts 30 minutes, renewed every 15 s while players are connected and at each idle poll (10 min). A cell that dies keeps its regions until its leases run out; to take over sooner, delete them (`DELETE FROM ops.region_lease WHERE cell = 'x'`, likewise `ops.main_lease`) once that cell is surely down.
- **Setting one up:** a container with its own name, port and `.env` (same database, same keys, `ELEMENTS_CELL=b`, `ELEMENTS_REGION` of the cell); register it and main in `ops.cells`; route in the proxy by `?region=`: every region whose cell isn't `main` to its cell, everything else (also requests without a region) to main. In Caddy a `map {query.region} {cell_eu} { region-x 127.0.0.1:5090 … default 127.0.0.1:5080 }` and `reverse_proxy {cell_eu}`.
- **Moving:** in one transaction `UPDATE geo.regions SET cell = 'b' WHERE slug IN (…)` and `INSERT INTO ops.cell_events (shard, from_cell, kind) VALUES ('eu', 'ops', 'refresh')` (the old cell writes the players and lets go within seconds, the new one loads the region on the next request), then update the proxy's routing.
- **Limits:** rate limits count per cell. Backups: one container per shard is enough (the database is the same).

## Basemap and videos

The container serves what it finds in `/srv/static` (mount a host directory, e.g. `~/elements-static`). Without a basemap the app shows the game on a blank background (a built app never loads tiles from a third party), so fetch one before the first start. With `web/scripts/fetch-basemap.sh` (Protomaps extracts, header of the script):

```sh
S=~/elements-static/basemap
PMTILES_OUT=$S/region-x.pmtiles BBOX=9.10,47.63,9.22,47.70 SKIP_ASSETS=1 web/scripts/fetch-basemap.sh   # per region: its bounds + ~2 km
PMTILES_OUT=$S/world.pmtiles BBOX=world MAXZOOM=6 SKIP_ASSETS=1 web/scripts/fetch-basemap.sh          # the globe, once
SKIP_EXTRACT=1 BASEMAP_DIR=$S web/scripts/fetch-basemap.sh                                           # fonts and sprites, once
```

- `basemap/<slug>.pmtiles` per active region (bounds in `geo.regions` or `GET /api/regions`), `world.pmtiles`, fonts and sprites.
- `media/elements-<lang>.mp4`: explainer videos, if you have any (about 540 px wide, H.264).

## Reverse proxy

Any proxy with TLS in front of `127.0.0.1:5080`, WebSockets passed through. For Caddy:

```
example.org {
	encode gzip
	reverse_proxy 127.0.0.1:5080
}
```

Leave out an access log, so client IPs are not stored. Images from an S3 bucket under your own domain (`ELEMENTS_MEDIA_PUBLIC_URL=https://example.org/img`): pass `/img/*` on to the bucket, GET and HEAD only, without cookies and auth headers.

## Logins

Instead of (or besides) tokens, dashboard and editor can use the logins of a forward-auth server such as Authelia, authentik or oauth2-proxy. The proxy asks it about every request; for a logged-in person it passes their name and groups as headers, and the server maps the groups to roles:

- `ELEMENTS_AUTH_USER_HEADER=Remote-User` switches it on, `ELEMENTS_AUTH_GROUPS_HEADER` (default `Remote-Groups`) names the groups header (`docs/configuration.md`).
- Groups (prefix `ELEMENTS_AUTH_GROUP_PREFIX`, default `elements-`): `elements-read`, `elements-editor`, `elements-admin` for every region, `elements-editor-region-konstanz` for one. The rights are those of the token roles; a person may have several groups. Other groups are ignored.
- The audit log names the person as `@<name>`. Approving an image still needs a second person.
- A token typed in wins over the login (scripts, cities without logins keep working).

**The server trusts these headers as they come.** So the container must be reachable only through the proxy (`-p 127.0.0.1:…`, as above), and the proxy must remove them from every request that didn't pass the login. Easiest with two names for the same container: the public one strips them, a second one (for dashboard and editor) puts every request through the login. For Caddy and Authelia:

```
# Public: the game; login headers from outside never reach the server.
example.org {
	request_header -Remote-User
	request_header -Remote-Groups
	reverse_proxy 127.0.0.1:5080
}

# Dashboard and editor: everything through the login (`route` keeps this order).
ops.example.org {
	route {
		request_header -Remote-User
		request_header -Remote-Groups
		forward_auth 127.0.0.1:9091 {
			uri /api/authz/forward-auth
			copy_headers Remote-User Remote-Groups
		}
		reverse_proxy 127.0.0.1:5080
	}
}
```

Then `https://ops.example.org/dashboard.html` and `/editor.html` show "logged in as …" instead of the token field. The server also refuses a login on requests another site started (`Sec-Fetch-Site`, `Origin`), since the login rides on a cookie the browser would send along.

## Database

New migrations: test database first, then production, on every shard. Add columns with defaults or new tables, so the running code keeps working while it migrates. A database that only admits the host: `ELEMENTS_DB_VIA=user@host` tunnels every script through it (`db/tunnel.sh`).

## Game rules

The image ships `rules/default.toml` (plus `rules.local/*.toml` if you add your own) in `/srv/rules/` and uses `ELEMENTS_RULES=/srv/rules/default.toml` (live rain and sun need `ELEMENTS_WEATHER_URL`). Every key: `docs/configuration.md`, section 2. Per region: `geo.regions.rules`. Single values without a new image: `ELEMENTS_RULES_OVERRIDE=claim_ttl_s=420; claim_strength=0.8` in `.env` (no quotes there; quotes around the whole value are dropped anyway). A mistake in it is logged as `ELEMENTS_RULES_OVERRIDE ignored` and the region plays without the overrides. The server logs the effective rules at startup. Before a deploy that changes rules or what players see, add an entry on top of `NEWS` in `web/src/news.ts`.

## Operations

- Dashboard: `/dashboard.html?region=<slug>` (token).
- Manual hotspot: `POST /api/ops/hotspot?region=<slug>` with `Authorization: Bearer $ELEMENTS_OPS_TOKEN`.
- Reload a region from the database (new graph, outline, settings, rules) without a restart: `POST /api/ops/reload?region=<slug>`, same token.
- Tokens per city (only the hash is stored, `ops.tokens`):

  ```sh
  ELEMENTS_DB=… elements-server token add --region region-x --label "City X"                 # prints the token once
  ELEMENTS_DB=… elements-server token add --region region-x --label "X editors" --role editor
  ELEMENTS_DB=… elements-server token list
  ELEMENTS_DB=… elements-server token revoke "City X"
  ```

  Without `--region` a token opens all regions. Roles: `read` (default) the dashboard; `editor` also writes chapters, places and images in `/editor.html` (logged in `ops.audit_log`); `admin` also hotspots, region reloads and approving phrases. Images need a second token with another label to approve them.
- Presence codes for posters and stands: `elements-server code add --region <slug> --lat … --lon … --label "…"` prints the link for the QR code; `code list`, `code revoke <code>`.
- Redeploys are safe: on SIGTERM the server writes open aggregation windows, scores and permanent sources (grace period 30 s), and the live claims, anchors and sparks (`game.live_sources`, anonymous: no player behind them). The next start puts them back, aged by the time in between, so a deploy costs players only the reconnect. The same happens when an idle region is unloaded.

## Backups (aggregates)

Player tables are kept only as long as your database's point-in-time restore window (say so in your privacy policy). Back up only what holds no personal data, e.g. nightly with `pg_dump -Fc`: `analytics.*`, the campaign state, `game.wishes`, `geo.regions`; `geo` weekly. Never `game.players` and its tables, never `ops.tokens`.

Restore: create the schemas `analytics`, `game`, `geo` (and the extensions `postgis`, for geo also `h3`, `pgrouting`), then `pg_restore --no-owner --no-privileges -d <url> <file>`; foreign keys to `ops` fail and can be ignored.

## Monitoring

- `GET /api/health`: 200 `{"ok":true,…}` if the database answers and the default region is loaded, else 503.

## Globe and weather

Each shard can publish its regions as H3 cells with 72 h of history (`marble.rs`, format in `docs/data-files.md`), to an S3 bucket (`ELEMENTS_MARBLE_S3_*`) or only at `/api/marble/manifest.json`. Weather (rain and clouds) is off unless `ELEMENTS_WEATHER_URL` is set; the files come from a separate pipeline under its own terms, so point an instance only at files it may use. Without it there are no weather buttons and no requests.

## Secrets and rotation

All secrets live in the containers' `.env` on the host (`deploy/.env.example`), never in the repo. A change needs a new container (`docker rm -f`, `docker run`).

| Secret | Rotate how | Effect |
| --- | --- | --- |
| `ELEMENTS_OPS_TOKEN` | new value in `.env`, redeploy | old token stops at once; city tokens in `ops.tokens` are untouched |
| `ELEMENTS_SHARD_KEY` | new value in **every** shard's `.env`, redeploy all | passports and shard events signed with the old key fail for a few minutes |
| Database password | new URL in `.env`, redeploy | writers reconnect on the next write |
| `ELEMENTS_NOISE_KEY` | **do not rotate** | the key fixes the noise per published row; a new key shifts every count already published |
| `ELEMENTS_CHEAT` | production thresholds, not the public defaults | see `crates/server/src/cheat.rs` |

## When things go wrong

- **`/api/health` 503**: the database does not answer or the default region is not loaded. `docker logs --tail 200 elements`: `connect failed` means the database; `region load failed` means the graph or rules.
- **Proxy 502**: the container is down. `docker ps -a`.
- **Container restarts, `OOMKilled`**: the memory cap (`--memory`) was hit, usually too many large regions loaded at once. Raise it or lower `ELEMENTS_MAX_REGIONS`.
- **`scores: write failed` repeatedly**: usually a missing migration (`db/flyway.sh info`) or the database's connection limit.
- **Database down**: the game keeps running for connected players (world and limits in memory); new players get `busy`, scores queue in memory. Keep the container up.
- **Region stuck or stale graph**: `POST /api/ops/reload?region=<slug>`.

## Glossary (app term → rule key → module)

| In the app (de) | In the code | Rules | Module |
| --- | --- | --- | --- |
| Kreuzung | node of the street graph (`GeoNode`) | – | `shared/geo.rs`, `server/sim.rs` |
| Erobern, Quelle | claim → `ElementSource` | `claim_radius_m`, `claim_cooldown_s`, `claim_ttl_s`, `claim_strength`, `claim_ramp_s`, `takeover_below`, `decay` | `server/actions.rs`, `server/web.rs` (Claim) |
| Rückenwind | newcomer bonus on the first campaign day | `[progression] newcomer_ttl_factor`, `newcomer_takeover_below` | `shared/progression.rs`, `server/score.rs`, `web.rs` |
| Weitblick | ability `reach` | `[progression] reach_m`, `reach_cooldown_s` | `shared/progression.rs`, `web.rs` (`ABILITIES`) |
| Nähren | ability `nourish` | `nourish_cooldown_s` | same |
| Verstärken | ability `boost` | `boost_cooldown_s` | same |
| Funkenflug / Welle | ability `double` (fire / water name) | `double_cooldown_s` | same |
| Glutnest / Wasserader (Hinterland) | sparks | `[sparks]` | `shared/hinterland.rs`, `server/hinterland.rs` |
| Anker, Versorgung | anchors and supply | `[supply]` | same |
| Dauerquelle: Glut / Eis | keep (permanent source, bearer ticket) | `[keep]` | `shared/hinterland.rs` (`Keep`), `server/keep.rs` |
| Tunnel | tunnel to a far enemy hilltop/sink | `[tunnel]` | `shared/rules.rs`, `web.rs` |
| Fundstücke | finds | `[finds]` | `server/finds.rs`, `geo.finds` |
| Aufgabe | challenge (round) | `[progression] challenge_*`, `xp_*` | `shared/progression.rs`, `server/challenge.rs` |
| Kapitel | campaign chapter | `game.chapters` (data, not rules) | `server/campaign.rs`, editor |
| Hotspot | hotspot slot | `ELEMENTS_HOTSPOT_*` (env) | `server/hotspot.rs` |
| Wetter | the day's weather as a rule | `[weather]` | `shared/weather.rs`, `server/weather.rs` |
| Regen hier jetzt, Regenradar | live rain and sun per junction | `[weather] live_*`, `sun_*` | `shared/radar.rs`, `server/radar.rs`, `server/live_rain.rs` |
| Front stärken | reinforce at the front | `[reinforce]` | `server/actions.rs` |
| eingekesselt | encirclement of a permanent source | `[siege]` | `server/siege.rs` |
| Serie, Wochen in Folge | weekly streak | `[streak]` | `shared/streak.rs`, `server/score.rs` |
| Stadt-Serie | city streak (days with k players) | – | `server/score.rs`, `game.faction_day` |
| Vor Ort bestätigt | presence code | – | `server/gateway.rs`, `score.rs` |
