# Dev tasks for the Elements workspace. Install `just`: https://github.com/casey/just
#
# `just` with no args lists recipes. Database scripts read ELEMENTS_DB from the
# environment: `set -a; . ~/.config/elements/env; set +a` first.

default:
    @just --list

# --- workspace (shared, server) ---

# Type-check the default members.
check:
    cargo check --all-targets

# Format + clippy gate (matches CI).
lint:
    cargo fmt --all -- --check
    cargo clippy -p elements-shared -p elements-server --all-targets -- -D warnings

fmt:
    cargo fmt --all

test:
    cargo test -p elements-shared -p elements-server

# --- run locally (docs/web-dev.md) ---

# Run the headless server (needs ELEMENTS_DB and ELEMENTS_REGION; add
# ELEMENTS_ALLOW_SIM=1 for ?sim tap-to-move on dev).
server *ARGS:
    cargo run -p elements-server --bin elements-server -- {{ARGS}}

# Vite dev server for the web app on :5173 (proxies /api and /ws to :5080).
web-dev:
    cd web && npm run dev

# Type-check and build the web app into web/dist (what the image ships).
web-build:
    cd web && npm run build


# --- data (db/import/README.md) ---

# Migrations: `just flyway migrate` (test database first; ELEMENTS_DB_VIA=user@host
# tunnels through a host the database admits).
flyway *ARGS:
    db/flyway.sh {{ARGS}}

# A new region in one command from the local OSM copy: slug, name, Overpass
# area filter. See db/import/README.md "A new region in one command".
new-region slug name area:
    db/import/new-region.sh {{slug}} "{{name}}" '{{area}}'

# Sample terrain elevation onto an imported region's graph (edge grades and
# directional costs). new-region.sh runs this already.
import-terrain:
    db/import/import-terrain-region.sh
