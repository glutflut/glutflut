# Source me. With ELEMENTS_DB_VIA=user@host, `db_tunnel [VAR]` reaches the
# database in $VAR (default ELEMENTS_DB) through an SSH tunnel via that host,
# for a database that admits only its addresses. TLS still ends at the
# database; for Neon the endpoint goes in `options` since there is no SNI for
# 127.0.0.1. Tunnels close when the script exits. Without
# ELEMENTS_DB_VIA, or for a local database (also one already tunnelled, so
# child scripts reuse the parent's tunnel), nothing changes.
#
# After the call: $VAR points at the local end (exported), DB_TUNNEL_OPTS
# holds the (URL-encoded) options for clients that build their own URL (JDBC).

DB_TUNNEL_CTLS=()

db_tunnel_close() {
  local c
  for c in "${DB_TUNNEL_CTLS[@]}"; do
    ssh -S "$c" -O exit "$ELEMENTS_DB_VIA" 2>/dev/null || true
  done
}

db_tunnel() {
  local var="${1:-ELEMENTS_DB}"
  local url="${!var}"
  DB_TUNNEL_OPTS=""
  [[ -n "${ELEMENTS_DB_VIA:-}" && -n "$url" ]] || return 0
  local scheme="${url%%://*}" rest="${url#*://}"
  local creds="${rest%%@*}" hostpart="${rest#*@}"
  local hostport="${hostpart%%/*}" tail="${hostpart#*/}"
  local host="${hostport%%:*}" port=5432
  [[ "$hostport" == *:* ]] && port="${hostport##*:}"
  case "$host" in 127.0.0.1 | localhost | "[::1]" | "") return 0 ;; esac
  local db="${tail%%\?*}" query=""
  [[ "$tail" == *\?* ]] && query="${tail#*\?}"
  local endpoint="${host%%.*}"
  endpoint="${endpoint%-pooler}"

  local lport ctl
  lport="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
  ctl="$(mktemp -u "${TMPDIR:-/tmp}/elements-tunnel.XXXXXX")"
  ssh -f -N -M -S "$ctl" -o ExitOnForwardFailure=yes \
    -L "127.0.0.1:$lport:$host:$port" "$ELEMENTS_DB_VIA"
  DB_TUNNEL_CTLS+=("$ctl")
  trap db_tunnel_close EXIT
  echo "db: $var via $ELEMENTS_DB_VIA (127.0.0.1:$lport)" >&2

  DB_TUNNEL_OPTS="endpoint%3D$endpoint"
  printf -v "$var" '%s' "$scheme://$creds@127.0.0.1:$lport/$db?${query:+$query&}options=$DB_TUNNEL_OPTS"
  export "${var?}"
}
