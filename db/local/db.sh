#!/usr/bin/env bash
# A local PostGIS for data jobs, in a container (podman or docker):
#   db/local/db.sh start    # build the image once, start, print the URL
#   db/local/db.sh stop
#   db/local/db.sh url      # postgresql://… for ELEMENTS_LOCAL_DB
# Data in ${ELEMENTS_LOCAL_DATA:-~/.cache/elements/local-pg}; port ${ELEMENTS_LOCAL_PORT:-5440}.
# Empty database: ELEMENTS_DB="$(db/local/db.sh url)" db/flyway.sh bootstrap
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
NAME=elements-local-pg IMAGE=localhost/elements-local-pg:17
PORT="${ELEMENTS_LOCAL_PORT:-5440}"
DATA="${ELEMENTS_LOCAL_DATA:-${XDG_CACHE_HOME:-$HOME/.cache}/elements/local-pg}"
if [[ -z "${CONTAINER_ENGINE:-}" ]]; then
  for c in podman docker; do
    if command -v "$c" >/dev/null 2>&1 && "$c" info >/dev/null 2>&1; then CONTAINER_ENGINE="$c"; break; fi
  done
fi
E="${CONTAINER_ENGINE:?no usable podman/docker}"
URL="postgresql://elements:elements@127.0.0.1:$PORT/elements?sslmode=disable"
case "${1:-url}" in
  start)
    "$E" image exists "$IMAGE" 2>/dev/null || "$E" image inspect "$IMAGE" >/dev/null 2>&1 \
      || "$E" build -q -t "$IMAGE" -f "$DIR/Containerfile" "$DIR" >/dev/null
    mkdir -p "$DATA"
    if ! "$E" container inspect "$NAME" >/dev/null 2>&1; then
      "$E" run -d --name "$NAME" -p "127.0.0.1:$PORT:5432" -v "$DATA:/var/lib/postgresql/data:Z" \
        -e POSTGRES_USER=elements -e POSTGRES_PASSWORD=elements -e POSTGRES_DB=elements \
        --shm-size=1g "$IMAGE" -c fsync=off -c synchronous_commit=off -c full_page_writes=off \
        -c shared_buffers=2GB -c work_mem=256MB -c maintenance_work_mem=1GB -c max_wal_size=8GB >/dev/null
    else
      "$E" start "$NAME" >/dev/null
    fi
    for _ in $(seq 60); do psql "$URL" -XAtqc "SELECT 1" >/dev/null 2>&1 && break; sleep 1; done
    psql "$URL" -XAtqc "SELECT 1" >/dev/null
    echo "$URL" ;;
  stop) "$E" stop "$NAME" >/dev/null ;;
  url) echo "$URL" ;;
  *) echo "start|stop|url"; exit 2 ;;
esac
