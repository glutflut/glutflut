#!/usr/bin/env bash
# Run a command with ELEMENTS_DB (and FROM_DB/TO_DB, if set) tunnelled via
# ELEMENTS_DB_VIA (see tunnel.sh); without it, just run the command.
#   ELEMENTS_DB_VIA=user@host db/via.sh bash -c 'psql "$ELEMENTS_DB" -c ...'
set -euo pipefail
. "$(dirname "$0")/tunnel.sh"
db_tunnel ELEMENTS_DB
[[ -n "${FROM_DB:-}" ]] && db_tunnel FROM_DB
[[ -n "${TO_DB:-}" ]] && db_tunnel TO_DB
"$@"
