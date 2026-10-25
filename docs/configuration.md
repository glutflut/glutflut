# Configuration

Everything that configures Elements, in one place: environment variables, rule files (TOML) and the per-region settings in the database. Hosting (image, proxy, backups) is in `docs/deploy.md`.

Three layers decide how a region plays:

1. **Environment** of the server process (host `.env`, see `deploy/.env.example`): addresses, database, secrets, switches.
2. **Rules** (TOML, `rules/`): the game itself. The image default comes from `ELEMENTS_RULES`; a region can name its own set.
3. **Region rows** (`geo.regions`): city name, language, time zone, hotspot area, rule set, shard, listed or not.

## 1. Environment variables

Unset means the default. Flags (`=1`, `=0`) are compared exactly: `ELEMENTS_ALLOW_SIM=true` does nothing. Secrets live only in the containers' `.env` on the host and in a local file here (mode 600), never in a repo; rotation is in `docs/deploy.md` ("Secrets and rotation").

The server's main options are also command-line flags (`elements-server --help`); the variable is the same option.

### Server core

| Variable | Read by | Default | Meaning |
| --- | --- | --- | --- |
| `ELEMENTS_REGION` | server | none | Default region (slug): requests without `?region=`, `/api/health` (healthy if it is an active region of this server; not loaded for that). Loaded at start, then sleeps like any other region when unused (since 2026-10-12, so the database can scale to zero); `ELEMENTS_PRELOAD` keeps regions loaded. Example: `region-konstanz`. |
| `ELEMENTS_WEB_BIND` | server | `0.0.0.0:5080` | Web gateway (HTTP + WebSocket). The image sets the same. |
| `ELEMENTS_WEB_DIR` | server | none (no app served) | Built web app served at `/`. Image: `/srv/web`; locally `web/dist`. |
| `ELEMENTS_STATIC_DIR` | server | none | Files served before the web app (`basemap/`, `media/`). Image: `/srv/static`, mounted from the host (`docs/deploy.md`). |
| `ELEMENTS_PUBLIC_URL` | server | `https://glutflut.com` | Public base URL of this shard: links of presence codes, the globe's "play" link. Set per shard (`https://sa.glutflut.com`). |
| `ELEMENTS_SOURCE_URL` | server | none | Where this server's source code is published, `https://` only (AGPL: offer the source to all users). Sent as `source` in `/api/version`; the app's and info page's footers link it as "Quellcode". Unset: no link. |
| `ELEMENTS_VERSION` | server, web build | `dev` | Version string (`git describe` + date), a build argument of the image. Shown in the app and at `/api/version`. |
| `ELEMENTS_RULES` | server | none (built-in defaults, set `baseline`) | Path of the default rule file. Image: `/srv/rules/default.toml`. A typo stops the server at startup. See section 2. |
| `ELEMENTS_RULES_OVERRIDE` | server | none | Single rule values on top of every region's set: `claim_ttl_s=420; field.reach_scale=250`. An unknown key stops the server at startup. |
| `ELEMENTS_PRELOAD` | server | empty | Regions kept loaded from the start, comma-separated slugs. |
| `ELEMENTS_IDLE_MIN` | server | `180` | Unload a region after this many minutes without players. |
| `ELEMENTS_MAX_REGIONS` | server | `12` | Most regions loaded at once; the least recently used idle one goes. |
| `ELEMENTS_MAX_JUNCTIONS` | server | from the container's memory limit (1.5 KB per junction after 400 MB; 2g: about 1.1 million), none without a limit | Most junctions loaded at once (`0`: no limit). A region that doesn't fit waits (503, the app asks again) while the least recently used idle ones unload; a region alone always loads. |
| `ELEMENTS_WORKERS` | server | cores − 1 (min 1) | Worker threads that run the region apps. |
| `ELEMENTS_LOADERS` | server | `4` | Threads that load regions from the database. |
| `ELEMENTS_OPS_TOKEN` | server | none (ops API off) | **Secret, in `.env`.** Bearer token for `/api/ops/*`, `/api/analytics`, `/api/wishes`, `/api/feedback`; opens every region. City tokens: `elements-server token add`. |
| `ELEMENTS_AUTH_USER_HEADER` | server | none (logins off) | Logins through the proxy instead of tokens: the header with the user name, e.g. `Remote-User` (Authelia). Only behind a proxy that strips it from every other request (`docs/deploy.md`, Logins). |
| `ELEMENTS_AUTH_GROUPS_HEADER` | server | `Remote-Groups` | Header with the user's groups, comma-separated. |
| `ELEMENTS_AUTH_GROUP_PREFIX` | server | `elements-` | Only groups with this prefix count: `<prefix>read`, `<prefix>editor`, `<prefix>admin` for every region, `<prefix><role>-<slug>` for one (`elements-editor-region-konstanz`). |
| `ELEMENTS_AUTH_LOGOUT_URL` | server | none | `https://` address the dashboard's and editor's "Abmelden" goes to for a login. |
| `ELEMENTS_DEMO_DAY` | server | `2025-12-06` (the region's `demo_day` wins) | UTC day of the simulated demo dataset in the dashboard, for regions without their own. |
| `ELEMENTS_DEMO_LABEL` | server | `christmas-saturday` (the region's `demo_label` wins) | Name key of the demo day, for regions without their own. |
| `ELEMENTS_CITY` | server | none (region name up to " – " wins) | City name on the pages, only where the region has none. |
| `ELEMENTS_DUMP` | server | none | Dev: path of a CSV with each node's fire/water state, rewritten with every territory log line. |
| `UPDATE_PROTOCOL` | server tests | unset | Set: `cargo test -p elements-server protocol` rewrites `web/src/protocol.ts` instead of failing. |

### Hotspots

They date from one container per city. The region's own settings win; sites only count in the region that has those nodes. `COUNT`, `SLOT` and `ACTIVE` apply to every region of the process. In production use `geo.regions` and the editor.

| Variable | Read by | Default | Meaning |
| --- | --- | --- | --- |
| `ELEMENTS_HOTSPOT_SITES` | server | none | Fixed hotspot sites, comma-separated node ids. In the region that has these nodes they win over the city's chosen sites; other regions ignore them. |
| `ELEMENTS_HOTSPOT_CENTER` | server | centroid | Centre of the automatic site area, `lat,lon`, for regions without `hotspot_lat`/`lon`. |
| `ELEMENTS_HOTSPOT_AREA_M` | server | `700` | Radius of the automatic site area (m), for regions without `hotspot_area_m`. |
| `ELEMENTS_HOTSPOT_COUNT` | server | `12` | Automatic sites (farthest-point sampling). |
| `ELEMENTS_HOTSPOT_SLOT` | server | `600` | Slot length (s). |
| `ELEMENTS_HOTSPOT_ACTIVE` | server | `2` | Hotspots active per slot. |

### Database and shards

| Variable | Read by | Default | Meaning |
| --- | --- | --- | --- |
| `ELEMENTS_DB` | server, all `db/` and import scripts | none (server: no regions) | **Secret, in `.env`.** PostgreSQL URL. Containers connect as `elements_app`; Flyway and imports as `elements_owner`. Unquoted in Docker `.env` files. |
| `ELEMENTS_DB_READ` | server | `ELEMENTS_DB` | **Secret, in `.env`.** Read-only PostgreSQL URL as `elements_read` (geo and analytics, nothing about players) for the dashboard's reads: `/api/analytics`, `/api/export/…`, `/api/report`, `/api/areas`. Token checks and everything else stay on `ELEMENTS_DB`. |
| `ELEMENTS_DB_THREADS` | server | `4` | Writer threads for scores and analytics, one connection each. |
| `ELEMENTS_SHARD` | server | `eu` | This process runs the regions with this `geo.regions.shard`. |
| `ELEMENTS_CELL` | server | `main` | Cell within the shard: only regions with this `geo.regions.cell` load here; `main` also runs the shard-wide jobs (outbox, globe index). See `docs/deploy.md`, Cells. |
| `ELEMENTS_SHARDS` | server | empty | Other shards: `sa=https://sa.glutflut.com,na=https://na.glutflut.com`. Drives region links, CSP and the globe's shard index. |
| `ELEMENTS_SHARD_KEY` | server | none | **Secret, in `.env`.** Same value on all shards of one setup (dev has its own). Signs passports and calls between shards. |

### Analytics and privacy

| Variable | Read by | Default | Meaning |
| --- | --- | --- | --- |
| `ELEMENTS_NOISE_KEY` | server | empty (warns: noise predictable) | **Secret, in `.env`.** Fixes the noise on published counts. Never rotate (`docs/deploy.md`). |
| `ELEMENTS_K` | server | `5` | Minimum distinct persons per written aggregate. |
| `ELEMENTS_WINDOW_S` | server | `900` | Aggregation window (s). Lower only for tests. |
| `ELEMENTS_ESTABLISH_S` | server | `600` | Playing time (s) before a new real player counts in analytics; `0` = at once. |

### Anti-cheat and limits

The thresholds themselves (speed checks, shadow ban, tracks, rate buckets in `limits.rs`) are constants in the server, not configuration.

| Variable | Read by | Default | Meaning |
| --- | --- | --- | --- |
| `ELEMENTS_ALLOW_SIM` | server | off | `1`: test server. `?sim` (tap to move), `?side=`, side switch, bots count, the position check is off, TEST badge. Never with the prod database. |
| `ELEMENTS_LIMITS` | server | on | `0`: no per-client rate limits (only while running bots against a server behind Caddy). Local calls without a proxy are never limited. |
| `ELEMENTS_POW_BITS` | server | `18` (max 30) | Zero bits of the proof of work for new players; `0` = off. |
| `ELEMENTS_TRUST_AFTER_S` | server | `600` | New identities claim at half strength until they have played this long (s); `0` = full strength at once. |
| `ELEMENTS_CHEAT` | server | built-in (`cheat.rs`) | Thresholds of the position and track checks, `key=value; …` with the keys of `Cheat` in `crates/server/src/cheat.rs` (speeds in m/s, e.g. `max_speed_ms=7.5; lock_s=90`). A typo or an unknown key stops the server at startup. Production sets its own values here. |

### Weather and radar

| Variable | Read by | Default | Meaning |
| --- | --- | --- | --- |
| `ELEMENTS_WEATHER_URL` | server | none (no weather) | Weather data host, `http(s)://` only (formats in `docs/data-files.md`). Unset: no rain/cloud values, no buttons in the app, nothing in the CSP; live rain and sun rules then have no effect. |

The day's weather for `[weather]` (rain, heat, frost) comes from Open-Meteo per region and needs no variable.

### Globe export

Without the S3 values the globe files are only served at `/api/marble/…`.

| Variable | Read by | Default | Meaning |
| --- | --- | --- | --- |
| `ELEMENTS_MARBLE_S3_ENDPOINT` | server | none (no upload) | S3 endpoint URL of the bucket. |
| `ELEMENTS_MARBLE_S3_BUCKET` | server | none | Bucket name; files go under `glutflut/<shard>/`. |
| `ELEMENTS_MARBLE_S3_KEY_ID` | server | none | **Secret, in `.env`.** Access key id. |
| `ELEMENTS_MARBLE_S3_SECRET` | server | none | **Secret, in `.env`.** Secret key. |
| `ELEMENTS_MARBLE_S3_REGION` | server | from a Hetzner endpoint host (`nbg1.your-objectstorage.com` → `nbg1`), else `us-east-1` | S3 region for signing. |
| `ELEMENTS_MARBLE_ORIGINS` | server | none (only `localhost`/`127.0.0.1`) | Origins allowed to read `/api/marble/…` (CORS), comma separated, e.g. `https://globe.example.org`. |
| `ELEMENTS_MARBLE_UNLISTED` | server | off | `1`: unlisted (beta, demo) regions on the globe too. |

### Images (finds, zones, chapters)

City images from the editor (`media.rs`, `editor_media.rs`). Without the
four S3 values the editor shows the image section read-only (no upload,
no pick) and `GET /api/media` answers `[]`. The bucket must allow anonymous
GET on objects (listing may stay private: the database is the index).

| Variable | Read by | Default | Meaning |
| --- | --- | --- | --- |
| `ELEMENTS_MEDIA_S3_ENDPOINT` | server | none (no images) | S3 endpoint URL of the media bucket (Hetzner object storage). |
| `ELEMENTS_MEDIA_S3_BUCKET` | server | none | Bucket name; files go under `<region>/<kind>/<id>-<random>.jpg`, `Cache-Control: public, max-age=31536000, immutable`. |
| `ELEMENTS_MEDIA_S3_KEY_ID` | server | none | **Secret, in `.env`.** Access key id. |
| `ELEMENTS_MEDIA_S3_SECRET` | server | none | **Secret, in `.env`.** Secret key. |
| `ELEMENTS_MEDIA_S3_REGION` | server | from a Hetzner endpoint host, else `us-east-1` | S3 region for signing. |
| `ELEMENTS_MEDIA_PUBLIC_URL` | server | `<endpoint>/<bucket>` (path style) | Base URL the app loads images from. E.g. `https://example.org/img` on every container, with the proxy passing `/img/*` on to the bucket (`docs/deploy.md`, Reverse proxy), so players only talk to your domain. Its origin goes into the pages' CSP `img-src`. Changing it moves all images at once: only keys are stored. |
| `ELEMENTS_MEDIA_S3_PREFIX` | server | none | Prefix for new files, e.g. `dev/` on dev containers. A server deletes only files under its own prefix (none: the `region-…` keys at the root), so dev never deletes prod's files after a reset of dev to prod (dev still shows them). |
| `ELEMENTS_MEDIA_PRIVATE` | server | off | `1`: bucket not publicly readable; the app loads `/api/media/<id>`, a redirect to a presigned URL (1 h, cached 30 min). |

`img-src` also allows `upload.wikimedia.org` and `thumb.wikimedia.org` (Commons
thumbnails in the editor). Commons suggestions (`geo.commons_suggestions`) come from a data job of your own.

### Database scripts

Run on this machine. Database URLs come from `~/.config/elements/env` (`set -a; . ~/.config/elements/env; set +a` inside the command).

| Variable | Read by | Default | Meaning |
| --- | --- | --- | --- |
| `CONTAINER_ENGINE` | `db/flyway.sh` | first working of podman, docker | Container engine here. |
| `ELEMENTS_DB_VIA` | `db/tunnel.sh`, `via.sh`, `flyway.sh`, `dbvia.py` | none (direct) | `user@host` to tunnel the database through (a database that admits only that host). |
| `FLYWAY_IMAGE` | `flyway.sh` | `flyway/flyway:12.11.0-alpine` | Flyway container image. |
| `FROM_DB`, `TO_DB` | `via.sh` | none | Also tunnelled when set. |

Import scripts (`db/import/`) take many more (`REGION_SLUG`, `REGION_NAME`, `REGION_UUID`, `ID_OFFSET`, `SHARD`, `LANGUAGE`, `TIMEZONE`, `OSM_AREA`, `OSM_FILE`, `OSM_CLIP`, `OSM_PARENT`, `PLACES`, `COAST`, `WATER_R`, `WATER_SHARE`, `SOURCES`, `WATER_SOURCES`, `FOUNTAINS`, `AFFINITY`, `DEM_*`, `GRADE_K_*`, `COST_MULT_*`, `NEW_SINCE`, `OVERPASS`, `KEEP`, `DRY`, …). They are data jobs, documented in `db/import/README.md` and in each script's header.

### Web build, dev server and checks

| Variable | Read by | Default | Meaning |
| --- | --- | --- | --- |
| `ELEMENTS_GATEWAY` | `web/vite.config.ts` | `http://127.0.0.1:5080` | Server the Vite dev server proxies `/api` and `/ws` to. |
| `ELEMENTS_VERSION` | `web/vite.config.ts` | `git describe` + date + "(lokal)" | Version baked into the app. |
| `ELEMENTS_CREDITS_DIR` | `web/vite.config.ts` | `web/credits.local/` (copy yours there before an image build) | Instance-specific credits: `<name>.<lang>.html` replaces `<!-- credits:<name>:<lang> -->` in the pages (today: `terrain`, the elevation sources in the legal notice). Without them only the public credits (OSM, Copernicus GLO-30, …) show. |
| `HTTPS` | `web/vite.config.ts` | off | Set: self-signed HTTPS (`npm run dev:https`), so phones on the LAN get GPS. |
| `BBOX`, `BUILD`, `BASEMAP_SRC`, `BASEMAP_DIR`, `BASEMAP_THREADS`, `PMTILES_OUT`, `SKIP_ASSETS`, `SKIP_EXTRACT` | `web/scripts/fetch-basemap.sh` | see the script header | Local basemap extract (`docs/web-dev.md`). |
| `E2E_REGION`, `E2E_AT` | `web/e2e/where.mjs` (shared by the checks) | `region-konstanz`, a Konstanz point | Region and position of the e2e checks. |

## 2. Rules (TOML)

Rules are data (`crates/shared/src/rules.rs` and the modules it embeds). Every key is optional: a missing key keeps its default, a missing table keeps all of its defaults. An unknown key in a file is ignored by the TOML loader; in `ELEMENTS_RULES_OVERRIDE` it is an error.

### How a region gets its rules

1. `geo.regions.rules` names a set: the file `<name>.toml` in the directory of `ELEMENTS_RULES` (image: `/srv/rules`; without `ELEMENTS_RULES`: `rules/`). The column allows `[a-z0-9-]` only. A missing file is logged as an error when the region list loads (at start); if the named file fails to load, that region plays `ELEMENTS_RULES` (error in the log), the built-in defaults only without it.
2. Else the file in `ELEMENTS_RULES`. A broken file stops the server.
3. Else the built-in defaults (the "Default" column below, set name `baseline`).
4. Then `ELEMENTS_RULES_OVERRIDE` goes on top, for every region.

The server logs the effective main values per region (`game rules`). Rules are read when a region loads; `POST /api/ops/reload?region=<slug>` picks up a new set without a restart.

Image default: `deploy/Dockerfile` copies `rules/` (and your own sets from `rules.local/`) to `/srv/rules/` and sets `ELEMENTS_RULES=/srv/rules/default.toml`.

### `base` chains

`base = "other.toml"` (relative to the file) is loaded first; the file's values go on top, table by table and key by key. So a set lists only what it changes. Bases can have bases (up to 16 deep; a loop is an error). `name` is just a label; set it in every file, or the base's name shows in the logs.

The server logs the effective values of each region's set, base chain resolved, when the region loads (`game rules`).

### Field reference

"Default" is the built-in value (`Default` impls). "default.toml" is the value in `rules/default.toml` where it differs (– = default). "Where": server, or "ignored" for keys the server does not read (it warns).

#### Top level

| Key | Default | default.toml | Where | Meaning |
| --- | --- | --- | --- | --- |
| `base` | – | – | loader | File loaded first (see above). |
| `name` | `"baseline"` | `"default"` | server | Label in the logs. |
| `claim_radius_m` | 80 | – | server | A player can claim junctions within this distance (m). |
| `claim_cooldown_s` | 1 | 10 | server | Pause between claims (s). |
| `claim_ttl_s` | 180 | 540 | server | How long a claimed node stays a source (s). |
| `claim_strength` | 1.0 | 0.8 | server | Pinned intensity of a claimed source (1.0 = as strong as a natural one). |
| `claim_ramp_s` | 0 | 120 | server | Build-up: full strength after `claim_ramp_s` × the enemy's strength at the node (neutral or own ground: at once). 0 = off. |
| `hotspot_ttl_factor` | 3.0 | – | server | Lifetime factor for claims near an active hotspot. |
| `hotspot_radius_m` | 40 | – | server | "Near a hotspot" (m); also gives `[score] hotspot_factor`. |
| `frontier` | false | – | ignored | Claims only in or next to own territory. |
| `capture_s` | 0 | – | ignored | Stay this long within `capture_radius_m` before a claim takes (0 = instant). |
| `capture_radius_m` | 30 | – | ignored | See `capture_s`. |
| `energy_max` | 0 | – | ignored | Claim budget (0 = unlimited); a claim costs 1. |
| `energy_per_min` | 1.0 | – | ignored | Budget refill per minute. |
| `natural_fire_strength` | 1.0 | 0.6 (Bogotá 0.40) | server | Pinned intensity of natural fire sources. |
| `natural_water_strength` | 1.0 | – | server | Pinned intensity of natural water sources (shore). |
| `natural_spring_strength` | 0.8 | – | server | Small water sources (fountains, `spring_source`). |
| `decay` | `"none"` | `"linear"` | server | How a claimed source weakens: `none` (until it expires), `linear` (to 0 at the end), `halflife`. |
| `decay_halflife_s` | 180 | – | server | Half-life for `halflife` (s). |
| `fire_decay` | 1.0 | – | server | Ageing speed of fire sources (> 1 = faster). |
| `water_decay` | 1.0 | – | server | Ageing speed of water sources. |
| `decay_affinity` | 0.0 | – | server | Surroundings: at a node fully leaning to the source's element it ages this fraction slower, at an opposite one faster. |
| `takeover_stronger` | false | true | server | A claim on an enemy claimed source needs more strength than it has left. |
| `takeover_below` | 1.0 | 0.5 | server | With `takeover_stronger`: the enemy source must be below this fraction of a fresh claim. |
| `join_weaker` | 0.67 | – | server | Chance a new player joins the weaker side (fewer players today, else less land); 0.5 = coin, 1 = always. |

#### `[field]` (spread, `field.rs`)

| Key | Default | default.toml | Meaning |
| --- | --- | --- | --- |
| `reach_scale` | 200 | 300 | Reach per edge; higher = farther. |
| `min_cost` | 1.0 | – | Cost floor, so tiny edges don't transmit at ~1.0. |
| `affinity_weight` | 0.0 | 0.15 (Bogotá 0.075) | How strongly node affinity (−1 water … +1 fire, from the surroundings) tilts transmission. |
| `slope_k` | 0.0 | 8.0 | Slope from node heights instead of imported costs (0 = imported): cost = length × clamp(1 ∓ `slope_k` × grade, `slope_min`, `slope_max`); cheaper for fire uphill, water downhill. |
| `slope_min` | 0.4 | – | Lower clamp of the slope factor. |
| `slope_max` | 3.0 | – | Upper clamp. |
| `water_climb_m` | 0.0 | 5.0 | Water climbing `h` m keeps `exp(−h / water_climb_m)` of its transmission (0 = off). |
| `fire_drop_m` | 0.0 | 5.0 | Fire going down `h` m keeps `exp(−h / fire_drop_m)` (0 = off). |

#### `[score]` (points, `scoring.rs`)

| Key | Default | default.toml | Where | Meaning |
| --- | --- | --- | --- | --- |
| `claim` | 10 | – | server | A claim on neutral ground. Own ground never pays. |
| `front_bonus` | 10 | – | server | Extra for an enemy or contested node. |
| `hotspot_factor` | 2 | – | server | Factor near an active hotspot. |
| `rate` | 2 | – | server | Rating a place. |
| `rate_max_per_day` | 5 | – | server | Ratings per day that pay. |
| `hold_m_per_point` | 500 | – | server | Holding: one point per minute per this many metres of street your sources colour (~9 junctions). |
| `points_hour_max` | 1500 | – | server | Cap per player and hour. |
| `points_day_max` | 6000 | – | server | Cap per player and day at level 1 (or without levels). |
| `points_day_per_level` | 3000 | – | server | The day cap grows by this per level above 1 (level 5: 18000). |

#### `[progression]` (levels, XP, abilities, challenges; `progression.rs`)

Off unless `enabled`.

| Key | Default | default.toml | Meaning |
| --- | --- | --- | --- |
| `enabled` | false | true | Switch for the whole table. |
| `thresholds` | [100, 600, 1500, 3000] | – | XP for levels 2, 3, 4, 5. |
| `bonus_per_level` | 0.05 | – | Passive source strength bonus per level above 1. |
| `bonus_max` | 0.15 | – | Cap of that bonus. |
| `abilities` | true | – | Abilities (unlocked at levels 2–5: Weitblick, Nähren, Verstärken, Funkenflug/Welle). |
| `challenges` | true | – | Challenges on/off. |
| `newcomer_ttl_factor` | 1.5 | – | Rückenwind on a player's first day: longer lifetime. |
| `newcomer_takeover_below` | 0.333… (1/3) | 0.333 | Rückenwind: enemies take over only below this fraction (instead of `takeover_below`). |
| `reach_m` | 110 | 160 | Weitblick: one claim with this reach (m). |
| `reach_cooldown_s` | 600 | – | Weitblick cooldown. |
| `nourish_cooldown_s` | 600 | – | Nähren (own source back to full) cooldown. |
| `boost_cooldown_s` | 300 | – | Verstärken (a teammate's source back to full) cooldown. |
| `double_cooldown_s` | 600 | – | Funkenflug / Welle (a second, neighbouring junction) cooldown. |
| `challenge_s` | 300 | – | A challenge must be done within this round (s). |
| `challenge_long_s` | 480 | – | Round for challenges that send you somewhere (Entdecken, Ruhige Ecke). |
| `challenge_back_to_back` | false | – | Next challenge right after finishing one (else when the round ends). |
| `challenge_gap_s` | 0 | 1800 | Pause after a round before the next challenge (0 = when the round ends). |
| `challenges_per_day` | 0 | 4 | Challenges per player and day (0 = no limit). |
| `challenge_points` | 0 | – | Points for a finished challenge (0 = XP only). |
| `xp_challenge` | 30 | 90 | XP per challenge. |
| `xp_round` | 10 | – | XP per round. |
| `xp_zone` | 15 | – | XP per zone. |
| `xp_claim` | 2 | – | XP per plain claim. |
| `xp_claim_cap` | 20 | – | XP from plain claims per day at most. |
| `xp_rating` | 5 | – | XP per rating. |
| `xp_boost` | 5 | – | XP for both players of a Verstärken. |
| `variety_step` | 0.25 | – | Each further kind of goal met on a day adds this to the day's XP … |
| `variety_max` | 1.0 | – | … up to this. |
| `zone_r_m` | 400 | – | A claim belongs to the nearest zone centre within this (zone XP, Entdecken). |
| `hold_junctions` | 20 | – | "Halten": junctions held from own sources that count. |

#### `[tunnel]` (server only)

A successful claim rarely opens a tunnel: for `window_s` the player may claim one remote junction where the enemy's element gathers (water: a sink, fire: a hilltop) and the enemy holds it.

| Key | Default | Meaning |
| --- | --- | --- |
| `chance` | 0.03 | Chance per successful claim (0 = off). |
| `window_s` | 60 | Time to use it (s). |
| `cooldown_s` | 900 | At most one per player this often. |
| `min_m` | 300 | Least distance of the far end. |
| `max_m` | 2500 | Most distance. |
| `hops` | 3 | A hilltop (sink) is higher (lower) than every junction within `hops` … |
| `margin_m` | 1.0 | … by at least this. |
| `enemy_below` | 0.9 | Only where the enemy holds less than this share of the claim's peak strength. |

No set in `rules/` changes `[tunnel]`, so the defaults are live.

#### `[sparks]` (Glutnest / Wasserader, `hinterland.rs`)

When a player has nothing to claim within `idle_m`, an ownerless source of the other element sometimes appears in the player's territory and grows until put out with a claim.

| Key | Default | default.toml | Meaning |
| --- | --- | --- | --- |
| `enabled` | false | true | Switch. |
| `idle_m` | 120 | – | "Nothing to do": no node that isn't the player's side within this (m). |
| `per_min` | 1.0 | – | Chance per idle minute of one player. |
| `min_m` | 120 | – | Least distance from the player. |
| `max_m` | 300 | – | Most distance. |
| `share` | 0.0 | – | Only while the player's side owns at least this share of owned nodes (0 = always). |
| `max` | 3 | – | At most this many at once against one side. |
| `start` | 0.3 | – | Strength when it appears … |
| `full` | 0.9 | – | … and after `grow_s`. |
| `grow_s` | 300 | – | Growth time (s). |
| `life_s` | 900 | – | Gone by itself after this (s). |
| `points` | 20 | – | Extra points for putting one out (integer). |

#### `[supply]` (anchors, Nachschub)

A claim on own ground places an anchor (Brunnen / Herd). Claimed sources within `range_m` of street through own territory from an anchor age at `supplied` speed, the others at `cut`.

| Key | Default | default.toml | Meaning |
| --- | --- | --- | --- |
| `enabled` | false | true | Switch. |
| `range_m` | 500 | – | Supply range along own streets (m). |
| `anchor_ttl_s` | 900 | – | Anchor lifetime (s). |
| `natural` | false | – | Natural sources supply too. |
| `supplied` | 0.75 | – | Ageing speed of supplied sources (1 = normal). |
| `cut` | 1.5 | – | Ageing speed of cut-off sources. |
| `anchor_points` | 10 | – | Points for an anchor (integer). |
| `anchor_gap_m` | 200 | – | An anchor only where the nearest one is farther (0 = anywhere). |

#### `[keep]` (Dauerquelle: Glut / Eis)

Permanent player sources. They fade from `strength` to `floor` over `fade_s`; each enemy claim that can't take one over wears it by `hit`; the owner tops it up with a claim on it. Takeovers follow `takeover_below`.

| Key | Default | default.toml | Where | Meaning |
| --- | --- | --- | --- | --- |
| `enabled` | false | true | server | Switch. |
| `per_player` | 1 | 2 | server | Permanent sources per player at level 1. |
| `per_level` | 0 | 1 | server | One more per level above 1 (with `[progression]`). |
| `gap_m` | 60 | – | server | At least this far from natural and other permanent sources (m). |
| `strength` | 0.8 | – | server | Strength when placed or topped up. |
| `floor` | 0.2 | 0.3 | server | Strength after `fade_s`. |
| `fade_s` | 3600 | 86400 | server | Fade time (s). |
| `hit` | 0.1 | 0.0 | server | Wear per enemy claim that can't take it. |
| `points_per_h` | 10 | – | server | Persistence points per hour standing (paid via the bearer ticket). |
| `expire_s` | 259200 (3 days) | – | server | Gone after this long without a top-up. |

#### `[weather]` (`weather.rs`)

Effect sizes at full intensity (0 = that effect off). The day's part (rain, heat, frost) needs `enabled` and the region's stored weather day. The live parts need `ELEMENTS_WEATHER_URL`; where live data covers a junction it replaces the day's rain (or sun) part.

| Key | Default | default.toml | Meaning |
| --- | --- | --- | --- |
| `enabled` | false | true | Switch for the day's weather. |
| `rain_decay` | 0.0 | 0.3 | Rain: water sources age this much slower, fire faster. |
| `rain_natural` | 0.0 | 0.5 | Rain: natural fire this much weaker. |
| `heat_decay` | 0.0 | 0.3 | Warm or sunny: water ages faster, fire slower. |
| `heat_natural` | 0.0 | 0.15 | Warm or sunny: natural water weaker. |
| `frost_climb` | 0.0 | 0.5 | Frost: water climbs better (`field.water_climb_m` × (1 + x)). |
| `rain_full_mm` | 10 | – | Full rain intensity from this much rain (mm/day). |
| `heat_full_k` | 6 | – | Full heat from this many K above the usual maximum (+2 K is still normal). |
| `frost_full_c` | 5 | – | Full frost from this many degrees below zero (minimum). |
| `live_decay` | 0.0 | 0.3 | Live rain (radar per junction and quarter hour): water ages slower, fire faster. |
| `live_natural` | 0.0 | 0.5 | Live rain: natural fire weaker. |
| `live_min_mm_h` | 0.2 | – | Live rain intensity 0 at this rate … |
| `live_full_mm_h` | 4.0 | – | … 1 from this rate (log scale). |
| `sun_decay` | 0.0 | 0.3 | Live sun (clear sky by day): fire ages slower, water faster. |
| `sun_natural` | 0.0 | 0.5 | Live sun: natural water weaker. |
| `sun_clear_pct` | 20 | – | Sun intensity 1 at or below this cloud cover (%) … |
| `sun_cloudy_pct` | 80 | – | … 0 from this. |
| `sun_elev_full` | 10 | – | Times daylight, full from this sun elevation (°). |

#### `[tide]` (`shared/tide.rs`, server `tide.rs`; on in `default.toml`)

Gauge level → phase −1 (mean low water) … +1 (mean high water); `k` = phase × weight per junction. At high tide fire's natural sources near the water get weaker and water ages slower, at low tide the other way round. Without a fresh level or above mean high water + `surge_m`: no effect. On the server only in regions with a gauge (`geo.region_tide`) and junction distances (`geo.node_tide`), both filled by a data job of your own; a host thread reads each gauge every 5 minutes (`pegelonline`, `noaa`, `ea`, `imi`; the level in the terrain's datum), a level older than 30 minutes (EA: 60) or a jump over 1 m switches it off. So far: Hamburg (St. Pauli), Bremen (Große Weserbrücke), New York (The Battery), San Francisco (Fort Point); London (EA, Tower Pier `0007`) and Dublin (`imi`, `Dublin_Port`) are ready for when the regions exist. The app shows it as 🌊 next to the weather; the tap also names the next high or low water in device time: from the forecast where the source has one (NOAA, fetched every 6 hours), else estimated from the last high and low in the gauge's last 26 hours plus the mean tidal period (12 h 25 min; PEGELONLINE, shown as "ca."). Junctions within 30 m of tidal water (`tide::SHORE_M`) get no sparks and no automatic hotspots.

| Key | Default | Meaning |
| --- | --- | --- |
| `strength` | 0.0 | The losing side's natural sources this much weaker at `k` = ±1 (0 = off). |
| `decay` | 0.0 | The favoured side ages this much slower, the other faster. |
| `fade_m` | 400 | Weight 1 at the water, 0 from this distance (cosine). |
| `elev_full_m`, `elev_half_m` | 5, 15 | Full weight up to this height above mean high water, half from the second. |
| `surge_m` | 1.5 | Storm surge: off above mean high water + this. |

#### `[finds]` (Fundstücke, server only)

| Key | Default | Meaning |
| --- | --- | --- |
| `uncover_m` | 40 | A claim within this uncovers a find (m) … |
| `dwell_s` | 10 | … or staying this long within it (s). |
| `hint_m` | 150 | A hint from this distance (m). |
| `first_hint_m` | 400 | … from this distance while the player has found nothing yet (m). |
| `hint_every_s` | 300 | Again for the same find after this (s). |

#### `[area]` (blocks, `rules.rs`; server `area.rs`)

A side holds a block (`geo.blocks`) when it owns at least `share` of its border junctions and the other side none. Needs blocks from `db/import/import-blocks.sh`.

| Key | Default | default.toml | Meaning |
| --- | --- | --- | --- |
| `enabled` | false | true | Switch. |
| `share` | 0.9 | – | Share of border junctions needed. |
| `points_per_ha_min` | 1.0 | 10.0 | Points per minute: `points_per_ha_min × ha^exponent`, to the players whose sources hold the border. |
| `exponent` | 1.0 | – | See above. |
| `age_factor` | 1.0 | 0.5 | Border sources of a held block age at this speed (0 = not at all). |
| `break_minutes` | 0 | 5 | Breaking a ring pays this many minutes of the block's area points (0 = off) … |
| `break_min` | 10 | – | … at least … |
| `break_max` | 50 | – | … at most. |
| `break_held_s` | 300 | – | Only if the block was held this long with a player's source on its border. |
| `break_every_s` | 3600 | – | At most once per block this often. |

#### `[siege]` (encirclement, `rules.rs`; server `siege.rs`)

A claimed or permanent source whose junction has at least `min_neighbours` neighbours, all held by the other side, for `delay_s`, ages `factor` times as fast (permanent: `keep_factor`) until the ring breaks. Natural sources and sparks never.

| Key | Default | default.toml | Meaning |
| --- | --- | --- | --- |
| `enabled` | false | true | Switch. |
| `min_neighbours` | 2 | 3 | Least neighbours (a dead end is never surrounded). |
| `delay_s` | 120 | – | Ring must stand this long (s). |
| `factor` | 4.0 | 1.0 | Ageing factor of surrounded claimed sources. |
| `keep_factor` | 4.0 | – | Ageing factor of surrounded permanent sources. |
| `by_players` | true | – | A neighbour counts only if a player's source of the other side holds it. |
| `points` | 0 | – | Points for closing a ring around a claimed source, split among its holders. |
| `keep_points` | 0 | 50 | Same around a permanent source. |

#### `[reinforce]` (claims at the front)

| Key | Default | default.toml | Meaning |
| --- | --- | --- | --- |
| `enabled` | false | true | Where nothing is left to take and no anchor fits, a claim may put an ordinary source on an own junction without one. No points. |
| `front_only` | false | true | Only where a neighbour isn't ours. |

#### `[streak]` (weekly streaks, `streak.rs`; server only)

| Key | Default | default.toml | Meaning |
| --- | --- | --- | --- |
| `enabled` | false | true | Switch. |
| `days_per_week` | 3 | – | Play days that make a week. |
| `acts_per_day` | 3 | – | Small actions that make a play day (a claim is one; a challenge, find or rating a whole day). |
| `spare_weeks` | 1 | – | Missed weeks spared per campaign. |
| `badges` | [2, 4, 8] | – | Streak lengths (weeks) that earn a badge `streak-N`. |

## 3. Other configuration

### Per region (`geo.regions`)

Set by the import scripts (`new-region.sh` takes `SHARD`, `LANGUAGE`, `TIMEZONE`) and by hand (SQL or the editor). The server rereads the region list every 5 minutes; a loaded region picks up changes with `POST /api/ops/reload?region=<slug>`.

| Column | Default | Meaning |
| --- | --- | --- |
| `slug`, `name` | – | Id in URLs (`region-konstanz`); name, the city is the part before " – ". |
| `active` | false | Playable and listed in `/api/regions`. |
| `listed` | true | false: demo or beta city, only by link (`?region=`), not matched by location, not on the globe (unless `ELEMENTS_MARBLE_UNLISTED=1`). |
| `shard` | `eu` | Server group that runs the region (`ELEMENTS_SHARD`). |
| `cell` | `main` | Server within the shard (`ELEMENTS_CELL`); the proxy routes by it (`docs/deploy.md`, Cells). |
| `rules` | NULL (= `ELEMENTS_RULES`) | Rule set by file name without `.toml`, `[a-z0-9-]` only. |
| `language` | `de` | Fallback language of the app (two letters). |
| `timezone` | `Europe/Berlin` | IANA zone: local days and hours (dashboard, "released next day", demo day). |
| `city_in` | NULL | How the app says "in <city>" per language, e.g. `{"de": "im Furttal", "en": "in the Furttal"}`. |
| `hotspot_lat`, `hotspot_lon`, `hotspot_area_m` | NULL (centroid, 700 m) | Area for automatic hotspot sites. |
| `demo_label` | NULL (`christmas-saturday`) | Name key of the demo day. |
| `demo_day` | NULL (`2025-12-06`) | Day of the simulated demo dataset. |
| `boundary_geom`, `bounds_*`, `centroid_geom` | – | Outline (location match, basemap bounds), centre. |

Other per-city data that changes how the game plays lives in its own tables and is edited in `/editor.html`, not in rules: chapters (`game.chapters`), hotspot sites and plans, campaign phrases, finds (`geo.finds`). Dashboard tokens: `ops.tokens` (`elements-server token …`). Presence codes: `game.presence_codes` (`elements-server code …`).

### Web app

The app has no configuration file. It learns everything from the server: `/api/regions` (cities, outlines), `/api/version` (version, shard, weather host if set), `/api/region`. Build-time values: `ELEMENTS_VERSION`, `ELEMENTS_GATEWAY`, `HTTPS` (above). Rule changes visible to players need an entry in `NEWS` in `web/src/news.ts` (`docs/deploy.md`, "Game rules").

URL parameters: `?region=<slug>` (pick a city), `?lang=<xx>`, `?p=<code>` (presence code), `?token=` (dashboard, taken once and removed). Test servers only (`ELEMENTS_ALLOW_SIM=1`): `?sim`, `?at=lat,lon`, `?side=fire|water`. Local state is in `localStorage` under `elements.*` (`web/src/store.ts`).

### Host, image, Caddy

See `docs/deploy.md`: image and `ENV` defaults (`deploy/Dockerfile`), container flags, the reverse proxy, backups and monitoring.
