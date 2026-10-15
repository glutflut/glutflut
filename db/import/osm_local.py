"""The local OSM copy (schema `osm`, import-osm-extract.sh) for the region
imports: instead of Overpass, which is slow and fails now and then. Each
import falls back to Overpass when the copy has nothing there (an area not
imported); OVERPASS=1 forces Overpass."""
import json
import os
import re

import dbvia


def use(db):
    """True if the local copy exists and isn't switched off."""
    if os.environ.get("OVERPASS") == "1":
        return False
    out = dbvia.psql(db, "-XAtc", "SELECT to_regclass('osm.pois') IS NOT NULL").strip()
    return out == "t"


def rows(db, sql):
    """The rows of `sql` as dicts."""
    out = dbvia.psql(db, "-XAt", "-f", "-", input=f"SELECT coalesce(json_agg(t), '[]') FROM ({sql}) t")
    return json.loads(out)


def lit(v):
    return "'" + str(v).replace("'", "''") + "'"


def osm_key(kind, osm_id):
    """imposm ids as Overpass names them: points are nodes, areas ways, or
    relations (negative ids)."""
    if kind == "point":
        return f"node/{osm_id}"
    return f"relation/{-osm_id}" if osm_id < 0 else f"way/{osm_id}"


def areas(osm_area):
    """(name, admin_level) pairs from an Overpass area expression
    ('["name"="X"]["admin_level"="8"]…;area["name"="Y"]…')."""
    out = []
    for part in osm_area.split(";"):
        name = re.search(r'\["name"="([^"]+)"\]', part)
        level = re.search(r'\["admin_level"="(\d+)"\]', part)
        if name:
            out.append((name.group(1), int(level.group(1)) if level else None))
    return out


def conds(osm_area):
    """WHERE clause matching those admin areas by name and level."""
    return " OR ".join(
        f"(name = {lit(n)}" + (f" AND admin_level = {lv})" if lv else ")") for n, lv in areas(osm_area)
    )


def parent_sql(geom="geometry"):
    """Names repeat (Moos, Aach, Reichenau): OSM_PARENT, admin areas in the same
    syntax (e.g. a district and the canton across the border), keeps only the
    areas inside them. Local copy only; Overpass ignores it."""
    parent = os.environ.get("OSM_PARENT")
    if not parent:
        return ""
    return (f" AND ST_Intersects(ST_PointOnSurface({geom}), "
            f"(SELECT ST_Union(geometry) FROM osm.boundaries WHERE {conds(parent)}))")


def area_sql(osm_area):
    """SQL for the union of those administrative areas (one geometry)."""
    return f"(SELECT ST_Union(geometry) FROM osm.boundaries WHERE ({conds(osm_area)}){parent_sql()})"


def check_sql(osm_area):
    """SQL listing each area that matches no boundary or more than one."""
    vals = ", ".join(f"({lit(n)}, {lv if lv else 'NULL'}::int)" for n, lv in areas(osm_area))
    return (
        f"SELECT a.name || ': ' || count(b.name) FROM (VALUES {vals}) a(name, lv) "
        f"LEFT JOIN osm.boundaries b ON b.name = a.name AND (a.lv IS NULL OR b.admin_level = a.lv)"
        f"{parent_sql('b.geometry')} "
        f"GROUP BY a.name HAVING count(b.name) <> 1"
    )


def reach_sql(osm_area, m=300):
    """The areas plus what lies within `m` of them in no other area of those
    levels (lake, sea, river): bridges and piers at the shore stay whole."""
    levels = ",".join(sorted({str(lv) for _, lv in areas(osm_area) if lv})) or "8"
    return (
        f"(WITH a AS MATERIALIZED (SELECT {area_sql(osm_area)} AS g), "
        f"b AS MATERIALIZED (SELECT ST_Buffer(g::geography, {m})::geometry AS g FROM a), "
        f"o AS MATERIALIZED (SELECT ST_Union(o.geometry) AS g FROM osm.boundaries o, b "
        f"WHERE o.admin_level IN ({levels}) AND o.geometry && b.g) "
        f"SELECT ST_Union(a.g, coalesce(ST_Difference(b.g, o.g), a.g)) FROM a, b, o)"
    )


if __name__ == "__main__":
    # python3 osm_local.py check '<osm-area>': the SQL of check_sql.
    import sys
    print(check_sql(sys.argv[2]))
