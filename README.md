# Glut & Flut

**Glut & Flut** is the game's name in the app; **Elements** is the series and
code name (crates, image, env variables). A location-based territory game on the real street graph of a city: fire
against water, claimed junction by junction with the phone's GPS, spread by a
field simulation that follows the terrain. Played in the browser, no accounts,
no stored positions. Play it at <https://glutflut.com>.

Rust server (Bevy ECS, axum, PostGIS at Neon), TypeScript web app (Vite,
MapLibre GL) and a PostGIS import pipeline. How it fits together:
[`docs/architecture.md`](docs/architecture.md). Changes:
[`CHANGELOG.md`](CHANGELOG.md). Contributing: [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Layout

| Path | What |
| --- | --- |
| `crates/shared` | rules as data, the field spread, hinterland, progression, weather and radar values, streaks, points, graph types |
| `crates/server` | the headless server: web gateway (`gateway.rs`), per-region Bevy apps (`regions.rs`, `web.rs`, `sim.rs`), local days (`clock.rs`), scores, analytics, keeps, campaign, live weather (`radar.rs`, `live_rain.rs`), shards, limits |
| `rules/` | the rule sets the server runs (TOML, `docs/configuration.md`) |
| `web/` | the PWA: `src/` (one module per server concern, `main.ts` wires them), locales, static pages (info, privacy, imprint), `e2e/` a few Playwright checks |
| `db/` | Flyway migrations (`migrations/`, `flyway.sh`), the import pipeline (`import/`, start with `new-region.sh`) and tunnel helpers |
| `deploy/` | the image (`Dockerfile`) and the container's settings (`.env.example`) |
| `docs/` | `architecture.md`, `deploy.md` (running your own instance), `configuration.md` (env and rules), `api.md`, `data-files.md` (globe and weather files), `web-dev.md` |
| `tools/` | `notice.sh` (regenerates `NOTICE`) |

## Run it

```sh
export ELEMENTS_DB=postgresql://…                        # PostGIS, migrated (db/flyway.sh), a region imported
ELEMENTS_ALLOW_SIM=1 ELEMENTS_REGION=region-konstanz cargo run -p elements-server
cd web && npm install && npm run dev                     # http://localhost:5173/?sim&at=47.66038,9.17639
```

Details, phone over Wi-Fi, dashboard, e2e checks: [`docs/web-dev.md`](docs/web-dev.md).
`just` lists the recipes (`just lint`, `just test`, `just web-build`, `just flyway`, `just new-region`).

- **Database**: PostgreSQL with PostGIS (we run it at Neon). Schema changes
  are Flyway migrations (`db/flyway.sh`), data jobs live in `db/import`
  (a region from OpenStreetMap and Copernicus GLO-30: `db/import/README.md`).
- **Rules**: `rules/*.toml`, chosen with `ELEMENTS_RULES`, per region in
  `geo.regions.rules`, single values with `ELEMENTS_RULES_OVERRIDE` (image
  default `rules/default.toml`).
- **Deploy**: one container image (`deploy/Dockerfile`), the basemap beside
  it on the host. See [`docs/deploy.md`](docs/deploy.md).
- **CI**: fmt, clippy and tests for shared and server; `tsc` and
  `vite build` for the web app (`.github/workflows/ci.yml`).

## Versions

Bevy 0.18 (headless ECS, pinned in `Cargo.toml`),
Rust stable ≥ 1.85 (`rust-toolchain.toml`), Node 22 for the web app.

## Licence

The code (server, web app, import scripts, migrations) is licensed under
the GNU Affero General Public License, version 3 or later: [`LICENSE`](LICENSE),
SPDX `AGPL-3.0-or-later` in `Cargo.toml` and `web/package.json`. Anyone who runs
a modified version as a service must offer its source to the users (AGPL §13);
set `ELEMENTS_SOURCE_URL` and the app links to it next to the version.

UI texts and their translations (`web/src/locales/`, the HTML pages) are part of
the code and under the AGPL, except the legal pages (all rights reserved).
Not covered by the AGPL: the name, logo and icons (trademark, all rights
reserved; a fork may run the code but not under this name), the game content
(campaign texts, chapters, finds, inscriptions, demo scenarios: CC BY-NC 4.0)
and the operating configuration of our instances. Map data © OpenStreetMap
contributors (ODbL); terrain from Copernicus GLO-30 and, per city, the
sources named on the app's legal page; the day's weather from Open-Meteo (CC BY 4.0); rain and
cloud files from Weathermarble (its terms, `docs/data-files.md`); attributions in the
app's legal page. Dependencies are MIT, Apache-2.0, BSD, Zlib, ISC, Unicode-3.0
or MPL-2.0 (Rust) and MIT or BSD-3-Clause (npm), all compatible with the AGPL;
the list is in [`NOTICE`](NOTICE). Contributions are made under the
[`CLA`](CLA.md); security reports: [`SECURITY.md`](SECURITY.md).
