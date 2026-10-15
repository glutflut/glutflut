# Web client: run and test

## Local (desktop browser)

```sh
set -a; . ~/.config/elements/env; set +a
ELEMENTS_ALLOW_SIM=1 ELEMENTS_DB=<test database> ELEMENTS_REGION=region-konstanz cargo run -p elements-server   # terminal 1
cd web && npm install && npm run dev                                                          # terminal 2
```

Open `http://localhost:5173/?sim&at=47.66038,9.17639` (sim mode: tap the map to move). Without `?sim` the app uses the device GPS. On a sim server, `&side=fire` or `&side=water` picks the side, and the settings panel has a switch button ("Zu Wasser wechseln (Test)"); both only work with `ELEMENTS_ALLOW_SIM=1`.

## Phone on the same Wi-Fi

GPS needs HTTPS, so run `npm run dev:https` instead of `npm run dev`. Vite prints a `Network:` URL like `https://192.168.1.x:5173/`. Open it on the phone, accept the certificate warning (self-signed), allow location. Outside Konstanz use `?sim`.

## Production-style (one port)

```sh
cd web && npm run build && cd ..
ELEMENTS_WEB_DIR=web/dist ELEMENTS_DB=… ELEMENTS_REGION=region-konstanz cargo run -p elements-server
```

The gateway serves the app, `/api/region` and `/ws` on `:5080`.

## Checks

```sh
cd web
node e2e/shot.mjs "http://127.0.0.1:5173/?sim&at=47.66038,9.17639" shot.png   # screenshot + console
node e2e/claim.mjs http://127.0.0.1:5173 claim.png                             # two players, claims
node e2e/pages.mjs http://127.0.0.1:5173 shots/                                 # help and legal pages over the game
```

`ELEMENTS_RULES_OVERRIDE='claim_ttl_s=10'` shortens claim lifetime for testing.

The scripts take `E2E_REGION` and `E2E_AT` ("lat,lon", `e2e/where.mjs`; default Konstanz) and need a server with `ELEMENTS_ALLOW_SIM=1`. They play as real players on that database: use a test database, and a region no other server of it is running (each writes back its own permanent sources per region).

Weather: start the server with `ELEMENTS_WEATHER_URL=<weather host>` to get the rain and cloud buttons, the junction card's rain and, with `rules/default.toml`, live rain and sun (formats in `docs/data-files.md`). If the weather host's CORS doesn't allow localhost, the map layers may stay empty while the server side works. Without it there is no weather at all.

`web/src/protocol.ts` (socket messages) is generated from `crates/server/src/protocol.rs`; after changing a message run `UPDATE_PROTOCOL=1 cargo test -p elements-server protocol` and commit the file. `cargo test` and CI fail while it is stale.

## Dashboard and hotspots

Start the server with `ELEMENTS_OPS_TOKEN=<token>`; the dashboard is `/dashboard.html?token=<token>` (production build) or `http://localhost:5173/dashboard.html` (dev). The page takes the token from the link once, removes it from the address bar and keeps it for that tab; a token typed into the field is remembered in the browser. The API accepts it as `Authorization: Bearer` only, never in the URL.

```sh
curl -X POST -H "Authorization: Bearer <token>" -H 'Content-Type: application/json' \
  -d '{"node": 100704, "minutes": 10}' http://127.0.0.1:5080/api/ops/hotspot     # manual hotspot
```

Scripts against a server behind a proxy hit the per-client limits; start that server with `ELEMENTS_LIMITS=0`. Local calls without a proxy are never limited.

New players solve a proof of work first (`ELEMENTS_POW_BITS`, 18; 0 = off); a script on a raw socket answers it with `answerPow` from `e2e/pow.mjs`. `bot: true` in `hello` (synthetic data) only counts on sim servers.

A first visit only watches (`web/src/watch.ts`): `hello` with `watch: true` gets a `welcome` with `watch: true` and no token; the server keeps no row, counts nothing and passes on only `view`, `overview` and `info`. "Mitspielen" (or "Los geht's" in the intro) stores `elements.joined` and reloads as a player. Browsers with a token, `?sim` and `?play` play at once (scripts that need a player in the browser add `?play`). `?kiosk` is a screen: always watching, no buttons, QR code to `/tv`, camera tour (`web/src/live.ts`).

Test knobs: `ELEMENTS_WINDOW_S` (aggregation window, 900), `ELEMENTS_K` (5), `ELEMENTS_ESTABLISH_S` (600; seconds a new real player must play before counting, 0 = at once), `ELEMENTS_HOTSPOT_SLOT` (600), `ELEMENTS_HOTSPOT_ACTIVE` (2), `ELEMENTS_HOTSPOT_COUNT` (12), `ELEMENTS_HOTSPOT_SITES` (node ids), `ELEMENTS_RULES_OVERRIDE` (single rule values, e.g. `claim_ttl_s=10`). All settings: `docs/configuration.md`.

Bot data is stored with `synthetic = true` and never mixes with real data in the dashboard unless "Alle" is selected. Run bots only against the dev database.

## Basemap (self-hosted)

`web/scripts/fetch-basemap.sh` downloads a Protomaps extract of the region (about 6 MB) plus fonts and sprites into `web/public/basemap/` (not in git). With it, no tile request leaves our server. Without it the dev server (`npm run dev`) falls back to OpenFreeMap and logs a warning. A built app never does: it logs an error and draws the game on a blank background, and the server's CSP does not allow the OpenFreeMap origin (the privacy page promises that nothing loads from third parties). On a host the basemap lives beside the container, see `docs/deploy.md`.

`?sim` (tap to move) only works when the server runs with `ELEMENTS_ALLOW_SIM=1`, which also switches off the position plausibility check (`server/src/motion.rs`). Never set it in prod. The e2e checks need it.
