#!/usr/bin/env bash
# An indoor region (halls, `[indoor]`, server indoor.rs) from a hand-drawn
# graph (data job):
#
#   ELEMENTS_DB=… db/import/indoor-region.sh <slug> "<name>" <graph.geojson>
#
# graph.geojson: Points are junctions, with properties `key` (any unique
# text, used by the lines), `label` (printed on the sheet), optional
# `screen: true` (only the screen's changing code counts there) or
# `source: "fire" | "water"` (a natural source; not on screens). LineStrings
# are paths between two junctions: `from`/`to` keys, or their end points
# within 2 m of a junction. Draw it in QGIS or geojson.io over a hall plan.
# The region starts active and unlisted (by link only), rule set `indoor`
# (rules/indoor.toml, or your own `indoor.toml` next to ELEMENTS_RULES).
# Re-running replaces junctions and paths (ids change: print the sheets again).
# Env: LANGUAGE (de), TIMEZONE (Europe/Berlin), SHARD (eu), RULES (indoor).
set -euo pipefail
: "${ELEMENTS_DB:?set ELEMENTS_DB}"
. "$(dirname "$0")/../tunnel.sh"
db_tunnel
SLUG="${1:?slug, e.g. region-40c3}"
NAME="${2:?name}"
FILE="${3:?graph.geojson}"
[[ "$SLUG" =~ ^region-[a-z0-9-]+$ ]] || { echo "slug: region-<lowercase>"; exit 2; }
case "${SHARD:-eu}" in
  eu) LO=1000000 ;; na) LO=1000000000 ;; sa) LO=2000000000 ;;
  *) echo "SHARD: eu, na or sa"; exit 2 ;;
esac
python3 - "$FILE" "$SLUG" "$NAME" "${LANGUAGE:-de}" "${TIMEZONE:-Europe/Berlin}" "${SHARD:-eu}" "${RULES:-indoor}" "$LO" <<'PY' |
import json, math, sys
path, slug, name, lang, tz, shard, rules, lo = sys.argv[1:]
g = json.load(open(path))
pts, lines = {}, []
for f in g["features"]:
    t, p = f["geometry"]["type"], f.get("properties") or {}
    if t == "Point":
        k = str(p.get("key") or p.get("label") or len(pts))
        if k in pts: sys.exit(f"duplicate junction key {k}")
        src = p.get("source")
        if src not in (None, "fire", "water"): sys.exit(f"{k}: source fire or water")
        if src and p.get("screen"): sys.exit(f"{k}: a screen can't be a source")
        lon, lat = f["geometry"]["coordinates"][:2]
        typ = "screen" if p.get("screen") else {"fire": "plain_fire_source", "water": "plain_water_source"}.get(src, "crossing")
        pts[k] = (len(pts), lat, lon, str(p.get("label") or k), typ)
    elif t == "LineString":
        lines.append((p, f["geometry"]["coordinates"]))
def dist(a, b):
    la = math.radians((a[1] + b[1]) / 2)
    return math.hypot((a[0] - b[0]) * 111320 * math.cos(la), (a[1] - b[1]) * 110540)
def near(c):
    best = min(pts.items(), key=lambda kv: dist(c, (kv[1][2], kv[1][1])), default=None)
    if best and dist(c, (best[1][2], best[1][1])) <= 2.0: return best[0]
    sys.exit(f"line end {c} is not at a junction (2 m)")
edges = set()
for p, cs in lines:
    a = str(p["from"]) if "from" in p else near(cs[0])
    b = str(p["to"]) if "to" in p else near(cs[-1])
    if a not in pts or b not in pts: sys.exit(f"line {a}-{b}: unknown junction")
    if a != b: edges.add(tuple(sorted((a, b))))
if not pts or not edges: sys.exit("no junctions or no paths")
q = lambda s: "'" + s.replace("'", "''") + "'"
print(r"\set ON_ERROR_STOP on")
print("BEGIN;")
print(f"""INSERT INTO geo.regions (slug, name, active, listed, language, timezone, shard, rules)
  VALUES ({q(slug)}, {q(name)}, false, false, {q(lang)}, {q(tz)}, {q(shard)}, {q(rules)})
  ON CONFLICT (slug) DO UPDATE SET name = EXCLUDED.name, rules = EXCLUDED.rules;""")
print(f"CREATE TEMP TABLE r ON COMMIT DROP AS SELECT id FROM geo.regions WHERE slug = {q(slug)};")
# Its own block of node ids (as new-region.sh): kept on a re-run, else the next free million.
hi = 1_000_000_000 if int(lo) < 1_000_000_000 else int(lo) + 1_000_000_000
print(f"""CREATE TEMP TABLE o ON COMMIT DROP AS
  SELECT coalesce((SELECT (min(id) / 1000000) * 1000000 FROM geo.nodes WHERE region_id = (SELECT id FROM r)),
                  (SELECT (max(id) / 1000000 + 1) * 1000000 FROM geo.nodes WHERE id >= {lo} AND id < {hi}), {lo}) AS base;""")
print("DELETE FROM geo.edges WHERE region_id = (SELECT id FROM r);")
print("DELETE FROM geo.nodes WHERE region_id = (SELECT id FROM r);")
print("CREATE TEMP TABLE p (i int, lat float8, lon float8, label text, typ text, k text) ON COMMIT DROP;")
print("INSERT INTO p VALUES " + ",\n ".join(f"({i}, {lat}, {lon}, {q(lb)}, {q(ty)}, {q(k)})" for k, (i, lat, lon, lb, ty) in pts.items()) + ";")
print("""INSERT INTO geo.nodes (id, region_id, geom, lat, lon, elevation_m, node_type, story_tag)
  SELECT o.base + p.i, (SELECT id FROM r), ST_SetSRID(ST_MakePoint(p.lon, p.lat), 4326), p.lat, p.lon, 0, p.typ, p.label FROM p, o;""")
print("CREATE TEMP TABLE e (n int, a text, b text) ON COMMIT DROP;")
print("INSERT INTO e VALUES " + ",\n ".join(f"({n}, {q(a)}, {q(b)})" for n, (a, b) in enumerate(sorted(edges))) + ";")
print("""INSERT INTO geo.edges (id, region_id, source, target, geom, length_m, road_type,
                       fire_cost_forward, fire_cost_reverse, water_cost_forward, water_cost_reverse, base_travel_cost)
  SELECT o.base + e.n, (SELECT id FROM r), o.base + pa.i, o.base + pb.i, l.g, l.m, 'indoor', l.m, l.m, l.m, l.m, l.m
    FROM e JOIN p pa ON pa.k = e.a JOIN p pb ON pb.k = e.b, o,
         LATERAL (SELECT ST_MakeLine(ST_SetSRID(ST_MakePoint(pa.lon, pa.lat), 4326), ST_SetSRID(ST_MakePoint(pb.lon, pb.lat), 4326)) AS g) s,
         LATERAL (SELECT s.g AS g, greatest(1, ST_Length(s.g::geography)) AS m) l;""")
print(f"""UPDATE geo.regions g SET
    boundary_geom = s.b, centroid_geom = s.c,
    bounds_min_lat = s.y0 - 0.0004, bounds_max_lat = s.y1 + 0.0004,
    bounds_min_lon = s.x0 - 0.0006, bounds_max_lon = s.x1 + 0.0006,
    active = true, updated_at = now()
  FROM (SELECT ST_Multi(ST_Buffer(ST_ConvexHull(ST_Collect(geom))::geography, 40)::geometry) AS b,
               ST_Centroid(ST_Collect(geom)) AS c, min(lat) AS y0, max(lat) AS y1, min(lon) AS x0, max(lon) AS x1
          FROM geo.nodes WHERE region_id = (SELECT id FROM r)) s
  WHERE g.slug = {q(slug)};""")
print(f"SELECT {q(slug)} || ': ' || {len(pts)} || ' junctions, ' || {len(edges)} || ' paths, ids from ' || base FROM o;")
print("COMMIT;")
PY
psql "$ELEMENTS_DB" -XAtq -f -
