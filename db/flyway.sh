#!/usr/bin/env bash
# Run Flyway (in a container) against the database in ELEMENTS_DB.
#
#   db/flyway.sh bootstrap   # empty database: baseline at v0, then run V1..Vn
#   db/flyway.sh adopt       # database that already has the V1 schema: stamp V1, run V2..Vn
#   db/flyway.sh [info]      # applied / pending
#   db/flyway.sh migrate     # apply pending
#   db/flyway.sh validate    # checksum check, no writes
#
# Test database first, then production. A database that admits only one host:
# ELEMENTS_DB_VIA=user@host (see tunnel.sh).
set -euo pipefail

FLYWAY_IMAGE="${FLYWAY_IMAGE:-flyway/flyway:12.11.0-alpine}"
DIR="$(cd "$(dirname "$0")" && pwd)"

# podman or docker, whichever works (CONTAINER_ENGINE to choose).
if [[ -z "${CONTAINER_ENGINE:-}" ]]; then
  for c in podman docker; do
    if command -v "$c" >/dev/null 2>&1 && "$c" info >/dev/null 2>&1; then CONTAINER_ENGINE="$c"; break; fi
  done
fi
[[ -n "${CONTAINER_ENGINE:-}" ]] || { echo "error: no usable podman/docker (or set CONTAINER_ENGINE)" >&2; exit 1; }
: "${ELEMENTS_DB:?set ELEMENTS_DB (see ~/.config/elements/env)}"

. "$DIR/tunnel.sh"
db_tunnel
# libpq URL -> JDBC URL + separate credentials (kept out of the URL and logs).
rest="${ELEMENTS_DB#*://}"
creds="${rest%%@*}"
hostpart="${rest#*@}"
# sslmode from the URL, else require (a local test database: ?sslmode=disable).
ssl=require
[[ "$hostpart" =~ [?\&]sslmode=([a-z-]+) ]] && ssl="${BASH_REMATCH[1]}"
JDBC_URL="jdbc:postgresql://${hostpart%%\?*}?sslmode=$ssl${DB_TUNNEL_OPTS:+&options=$DB_TUNNEL_OPTS}"

# The container must see a tunnel or a local database on the host's 127.0.0.1.
NET=()
[[ -n "$DB_TUNNEL_OPTS" || "$hostpart" =~ ^(localhost|127\.0\.0\.1)[:/] ]] && NET=(--network host)

flyway() {
  "$CONTAINER_ENGINE" run --rm "${NET[@]}" \
    -v "$DIR/migrations:/flyway/sql/migrations:ro" \
    -v "$DIR/flyway.conf:/flyway/conf/flyway.conf:ro" \
    -e FLYWAY_URL="$JDBC_URL" \
    -e FLYWAY_USER="${creds%%:*}" \
    -e FLYWAY_PASSWORD="${creds#*:}" \
    "$FLYWAY_IMAGE" "$@"
}

case "${1:-}" in
  bootstrap)
    flyway baseline -baselineVersion=0 -baselineDescription="bootstrap (nothing applied)"
    flyway migrate ;;
  adopt)
    flyway baseline -baselineVersion=1 -baselineDescription="baseline (goose schema)"
    flyway migrate ;;
  "") flyway info ;;
  *) flyway "$@" ;;
esac
