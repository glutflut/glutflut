# HTTP API

All routes live in `crates/server/src/gateway.rs` (the route table in `serve`); there is no OpenAPI spec. Responses are JSON unless noted. The game itself talks over `/ws` (messages in `protocol.rs`, mirrored in `web/src/protocol.ts`).

Common conventions:

- `?region=<slug>` picks the region; without it the server's default (`ELEMENTS_REGION`). Unknown slug: 404. A region on another shard: 421 `{url}` with that shard's address (`AppState::region`).
- `503` with `Retry-After: 2` and body `region loading`: the region is starting, ask again.
- **Token**: `Authorization: Bearer <token>`, never in the URL. `ELEMENTS_OPS_TOKEN` opens every region; city tokens (`elements-server token add`, `ops.tokens`) open their regions with a role (`read`, `editor`, `admin`). Without a token, a login through the proxy counts the same way (`ELEMENTS_AUTH_*`, `docs/deploy.md`, Logins; only from the same site). Without either: 401.
- Rate limits per client (`limits.rs`): 429 with `Retry-After`.

## Public

| Route | What |
| --- | --- |
| `GET /api/health` | `{ok, db, region, version}`; 200 or 503. For monitoring; never wakes a region or the database. |
| `GET /api/version` | `{version, shard, cell, weather?}`, plus `region, city, city_in, demo_label, demo_day, language` for the region. |
| `GET /api/regions[?v=2]` | All active regions of all shards; `v=2` packs the outlines. ETag, `max-age=300`. |
| `GET /api/region/meta` | Region settings and the tile index. |
| `GET /api/region` | The whole street graph as GeoJSON (large). |
| `GET /{s,k,m,t,ig,h,p,plan,tv}` | Short links (sticker, card, mystery card, play-day flyer, Instagram, lecture hall, press, ARG, kiosk screen): 302 to `/`, counts one hit per day and code in `analytics.link_day` (no cookie; bots and repeats from one client within 10 minutes left out). |
| `GET /api/tile/{x}/{y}` | One z15 graph tile `{n, e}` (gzip). |
| `GET /api/overview[&peek]` | Territory per tile `{z, t}` for the zoomed-out map; `peek`: 204 unless the region is loaded already. |
| `GET /api/leaderboard` | See below. `max-age=10`; 503 `no scores yet`. |
| `GET /api/media`, `/api/media/{id}` | Approved images (finds, zones, chapters). |
| `GET /api/chronicle` | The campaigns' chronicle: published chapters grouped by campaign (newest first) with titles, story, dates, goal, progress, `state` (`over`, `live`, `soon`) and `winner`. `max-age=120`. Page: `/chronik.html`. |
| `GET /api/mood` | Mood map: shares of the "Wie ist es hier?" answers per H3 cell (res 9) for one past day (yesterday, else the latest of the week before), only cells where at least k different persons rated that day; shares in steps of 10 %. Real players; `&demo=1` shows the synthetic ones of the region's demo day (`elements-server demo`; app: `?mooddemo`). `{day, k, cells: [[h3, {category: percent}]]}`. `max-age=600`. |
| `GET /api/marble/manifest.json`, `shards.json`, `{slug}/{file}` | Globe files (`docs/data-files.md`). |
| `GET /api/world[?scope=shard]`, `GET /api/marble/world.json` | World ranking: season points of the best players over all regions and shards (`world.rs`; `scope=shard` is one shard's part, what the shards exchange). `max-age=300`, CORS as for the globe files. |
| `GET /api/maintenance?region=` | `{maintenance: false, window, tz}`: the next planned maintenance window of this shard within 48 h (`{id, starts, ends, note}`, Unix seconds; `note`: `move`, `update`, `database` or null) or null, and the region's time zone. `max-age=60`. While the maintenance page is on, the proxy answers instead: 503 `{maintenance: true, until}`. |
| `POST /api/wish` `{lat, lon}` | "I want to play here"; counts the H3 res-6 cell. 204. |
| `POST /api/feedback` | Feedback from the app. 204. |
| `POST /api/session` | Body: the 32-hex token; sets the token cookie (empty body deletes it). Same origin only. |
| `GET /ws` | The game socket. Same origin only. |
| `POST /api/passport`, `/api/shard/event`, `/api/shard/recover` | Between shards (signed); not for tools. |
| `POST /api/cell/far?region=` | Between cells of a shard (signed): link messages for a region of that cell. |
| `POST <relay>/far?shard=&region=` | Sent, not served: link messages for a region of another shard to the relay (`ELEMENTS_FAR_RELAY`), same body and signature as `/api/cell/far`. |

`/api/leaderboard`:

```json
{
  "today":  [{"name": "…", "faction": "fire", "points": 120, "title": true, "streak": 3}],
  "season": [ … ],
  "fire":   {"node_minutes": 0, "points": 0, "wins": 0},
  "water":  { … },
  "titles": ["name or null", "name or null"],
  "city":   {"days": 4, "today": true, "missing": 0, "need": 5}
}
```

`today` and `season` are the top 10 only (hidden and shadowed players left out). `city` is the city streak: `today` is true once at least `need` (k) players scored today; below that, `need - missing` is today's count.

## With a token

| Route | Role | What |
| --- | --- | --- |
| `GET /api/access` | any | `{all, regions, role, roles}`: what the token or login opens (`roles`: `[{region, role}]`, region `null` = all). A login adds `user` and, if set, `logout`; a login without any of the groups gets 200 with no regions. |
| `GET /api/analytics` | read | k-anonymous summary (dwell per edge, persons per hour, …). Query: `days` (1–365, default 7) or `day=YYYY-MM-DD`; `data=synthetic\|all`. Real data appears the day after. `day=demo&data=synthetic` is public. Also `growth` (per day: seen, new, on 2+ days, last 30 days; today included), `peak` (most persons in a quarter hour) and `links` (short-link hits, all regions). |
| `GET /api/ops/live` | read | Live view: last closed 15-minute window, `{k, window_start, span_s, age_s, zones: {id: persons}}`, zones at or above `k` only, rounded to fives. `k` is `null` when off (`ELEMENTS_LIVE_K`). Query: `data=synthetic`. One row per region (`analytics.live_zones`), replaced by the next window; survives a restart. |
| `GET /api/export/{name}` | read | `dwell.csv`, `flows.csv`, `perception.geojson`, `crowdflow.json`; same query as analytics. |
| `GET /api/report` | read | Before/after report per focus place (`report.rs`). |
| `GET /api/areas` | read | Held areas (`areas.rs`). |
| `GET /api/feedback`, `GET /api/wishes` | ops token | Latest 500 feedback entries (with `id`, `done_at`, `done_by`); wish cells. |
| `GET/POST/DELETE /api/ops/chapters`, `GET /api/ops/chapters/suggest` | editor | Chapter editor (`editor.rs`). |
| `GET/POST/DELETE /api/ops/mottos`, `GET/POST /api/ops/places` | editor | Campaign phrases, places (finds may carry a `text` per language and a `campaign`). |
| `GET/POST/DELETE /api/ops/notices` | editor | News flashes (`editor_notices.rs`): `{id?, names: {lang: {title, text}}, url, starts, ends}`; the app shows running ones. |
| `GET/POST/DELETE /api/ops/media`, `POST /api/ops/media/commons`, `/decide` | editor | Images. |
| `POST /api/ops/hotspot` `{node, minutes}` | admin | Start a hotspot. 202. |
| `GET /api/ops/nodes` | editor | `{t, fire, water, front}`: junction ids per side now. 503 + `Retry-After` until the region has built it (only on request). For pictures of the game state (posts, posters). |
| `GET /api/ops/indoor` | admin | Indoor regions (`[indoor]`) only, else 409: `{region, base, window_s, hold_s, nodes: [{id, lat, lon, label, screen, code?, secret?}]}`; sheets get their fixed `code`, screens their `secret` (hex) for `/knoten.html`. Contains secrets: admins only. Page: `/indoor.html`. |
| `GET /api/ops/regions` | admin, all regions | Every region of this shard, also inactive and unlisted: `{shard, regions: [{slug, name, active, listed, rules, cell, language, indoor, loaded, players_today, players_7d, feedback_30d}], peers: [{shard, base}]}` (`ELEMENTS_ADMIN_PEERS`). Page: `/admin.html` (with feedback, approvals, wishes, indoor sheets). |
| `GET /api/ops/pending` | admin, all regions | What waits for approval in this shard's regions: `{media: [{id, region, region_name, kind, target, target_name, url, width, height, author, licence_label, licence_url, source_url, alt, by, created, bytes, replaces}], mottos: [{region, region_name, language, num, campaign, side, names, by}]}`. The admin page approves with `POST /api/ops/media/decide` and `POST /api/ops/mottos` (both read a text/plain body as JSON, for other shards' admin page). |
| `POST /api/ops/feedback/{id}` | admin, all regions | Body `{done: bool}` (read as JSON whatever its content type, so another shard's admin page can send a simple request): mark a feedback entry done (time and who: login or token label) or open again. 204, 404 unknown id. |
| `GET/POST /api/ops/maintenance` | admin, all regions | Maintenance windows of this shard (`ops.maintenance`): GET `{shard, items: [{id, starts, ends, note, by, cancelled, cancelled_by}]}` (open ones, then the last 20 past or cancelled); POST (text body read as JSON) `{starts, minutes, note?}` adds one (5–720 min, up to 60 days ahead), `{cancel: id}` cancels one; `{id}`, 404 unknown or over. Audit log. |
| `GET/POST /api/ops/updates`, `GET /api/ops/updates/{id}` | admin, all regions | OSM update queue (`ops.update_jobs`, `ops.region_updates`; run by an updater with `db/import/update-region.sh`): GET `{shard, regions: [{region, name, active, rhythm, auto, last_check, last_write}], jobs: [{id, region, kind, status, by, created, started, finished, summary, error}]}` (last 50 jobs; `summary` is the dry run's `<report>.json`); POST (text body read as JSON) `{region, rhythm}` (`off`, `monthly`, `quarterly`) or `{region, run: true}` (a dry run at the next tick; 409 while one is open), `{approve: id, now?}` (a `waiting` job writes: the next night 02–05 local, or at the next tick with `now`) or `{discard: id}` (drops an open job; 409 if not open); `/{id}`: `{id, region, report}` (Markdown). Audit log. |
| `POST /api/ops/reload` | admin | Unload and reload the region from the database. 202 `{region, was_loaded}`. |

Example:

```sh
curl -H "Authorization: Bearer $ELEMENTS_OPS_TOKEN" "https://<host>/api/analytics?region=<slug>&days=7"
```

## Player counts

No route reports players connected right now: positions and sockets exist only in memory (privacy by construction, `docs/architecture.md`). What there is:

- **Scored today**: `/api/leaderboard` → `city`; exact only below k (`need - missing`), above that only "at least k". The `today` list shows at most 10.
- **Seen today per region**: the database, role `elements_read`. Rows are written in batches by the scores writer, so a few minutes behind; `day`/`last_seen` are the region's local day.

  ```sql
  SELECT region, count(*) AS seen_today,
         count(*) FILTER (WHERE day = last_seen AND points_day > 0) AS scored_today
  FROM game.player_region
  WHERE last_seen = current_date
  GROUP BY region ORDER BY 2 DESC;
  ```

- **Persons per hour** (past days, k-anonymous, noised): `/api/analytics` → hourly.

A live count of open sockets per region would be a small addition (the region app knows its clients); expose it as a number only, behind a token.
