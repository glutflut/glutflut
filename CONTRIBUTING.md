# Contributing

Thanks for wanting to help with Glut & Flut. Issues and pull requests go to
<https://github.com/glutflut/glutflut>.

## Build and run

You need Rust stable (≥ 1.85, `rust-toolchain.toml`), Node 22, and a Postgres
with PostGIS. Flyway runs in a container (podman or docker).

```sh
# Database: an empty Postgres/PostGIS, then the schema
ELEMENTS_DB='postgresql://user:pass@localhost/elements' db/flyway.sh bootstrap

# A region: street graph from a local OSM copy (imposm) and Copernicus GLO-30 terrain
ELEMENTS_DB=… db/import/new-region.sh region-<name> "<Name>" '<osm admin area>'

# Server and web app
ELEMENTS_ALLOW_SIM=1 ELEMENTS_DB=… ELEMENTS_REGION=region-<name> cargo run -p elements-server
cd web && npm ci && npm run dev    # http://localhost:5173/?sim&at=<lat>,<lon>
```

`db/import/README.md` explains the import, `docs/web-dev.md` the local setup,
`docs/configuration.md` the settings and rule files. `just` lists the recipes.

## Tests

```sh
cargo fmt --all -- --check          # or: just lint, just test
cargo clippy -p elements-shared -p elements-server --all-targets -- -D warnings
cargo test -p elements-shared -p elements-server
cd web && npm run build            # tsc + vite build
node e2e/claim.mjs http://127.0.0.1:5173 claim.png   # browser checks in web/e2e/ (docs/web-dev.md)
```

The e2e scripts need a server started with `ELEMENTS_ALLOW_SIM=1`. If you
change a socket message in `crates/server/src/protocol.rs`, regenerate
`web/src/protocol.ts` with `UPDATE_PROTOCOL=1 cargo test -p elements-server protocol`.

## Pull requests

- One topic per pull request; say what it changes and how you tested it.
- Commit messages: [Conventional Commits](https://www.conventionalcommits.org/)
  with a scope, short (`fix(web): card follows live changes`).
- Schema changes are new Flyway migrations in `db/migrations/`; never edit an
  applied one.
- Keep code comments short. New UI texts need all seven locales in
  `web/src/locales/` (a rough translation is fine; say so in the PR).

## Licence and CLA

The code is under the GNU AGPL v3 or later (`LICENSE`). Before your first pull
request is merged, you sign the Contributor Licence Agreement in
[`CLA.md`](CLA.md); the CLA Assistant bot asks you to on the pull request.
It lets the project keep its options (for example a licence for a city that
cannot use AGPL code) while you keep the copyright of your work.

Security problems: please don't open an issue, see [`SECURITY.md`](SECURITY.md).
