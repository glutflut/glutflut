# Closed areas by hand

Areas closed to the public (entrance fee, opening hours, private) that OSM
doesn't tag as such: one GeoJSON file per region, `<region slug>.geojson`, in
this folder or the one `ELEMENTS_CLOSED_DIR` names; a
FeatureCollection of polygons with `name`, `osm` and `why`. Every street inside
stays out of the graph (`import-osm-imposm.sh`). Geometry from OpenStreetMap
(© OpenStreetMap contributors, ODbL).

A new one from an OSM way or relation:

    osmium getid -r <extract>.osm.pbf w123 -o /tmp/a.osm.pbf
    osmium export /tmp/a.osm.pbf --geometry-types=polygon -f geojson
