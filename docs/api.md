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
| `GET /api/marble/manifest.json`, `shards.json`, `{slug}/{file}` | Globe files (`docs/data-files.md`). |
| `POST /api/wish` `{lat, lon}` | "I want to play here"; counts the H3 res-6 cell. 204. |
| `POST /api/feedback` | Feedback from the app. 204. |
| `POST /api/session` | Body: the 32-hex token; sets the token cookie (empty body deletes it). Same origin only. |
| `GET /ws` | The game socket. Same origin only. |
| `POST /api/passport`, `/api/shard/event`, `/api/shard/recover` | Between shards (signed); not for tools. |

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
| `GET /api/export/{name}` | read | `dwell.csv`, `flows.csv`, `perception.geojson`, `crowdflow.json`; same query as analytics. |
| `GET /api/report` | read | Before/after report per focus place (`report.rs`). |
| `GET /api/areas` | read | Held areas (`areas.rs`). |
| `GET /api/feedback`, `GET /api/wishes` | ops token | Latest 500 feedback entries; wish cells. |
| `GET/POST/DELETE /api/ops/chapters`, `GET /api/ops/chapters/suggest` | editor | Chapter editor (`editor.rs`). |
| `GET/POST/DELETE /api/ops/mottos`, `GET/POST /api/ops/places` | editor | Campaign phrases, places. |
| `GET/POST/DELETE /api/ops/media`, `POST /api/ops/media/commons`, `/decide` | editor | Images. |
| `POST /api/ops/hotspot` `{node, minutes}` | admin | Start a hotspot. 202. |
| `GET /api/ops/nodes` | editor | `{t, fire, water, front}`: junction ids per side now. 503 + `Retry-After` until the region has built it (only on request). For pictures of the game state (posts, posters). |
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
