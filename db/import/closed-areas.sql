-- Streets nobody may simply walk on stay out of the graph (import-osm-imposm.sh,
-- step 2, on the edges before the junctions are counted).
-- Needs osm_import.edge2 (with access, foot), osm_closed and hand (polygons).
SET search_path = osm_import, public;

-- Private ways: foot=no/private, or access=private/no without a public foot.
DELETE FROM edge2
WHERE coalesce(nullif(foot, ''), '') IN ('no', 'private')
   OR (coalesce(nullif(access, ''), '') IN ('private', 'no')
       AND coalesce(nullif(foot, ''), '') NOT IN ('yes', 'designated', 'permissive'));

-- Closed areas: parks and gardens with a fee, zoos, theme parks, golf courses,
-- military land; private land, leisure, amenity and tourism areas from
-- 1000 m² (smaller ones are pools and yards).
CREATE TABLE closed_a AS
SELECT ST_MakeValid(geometry) AS g
FROM osm_closed
WHERE (leisure IN ('park', 'garden') AND fee = 'yes')
   OR tourism IN ('zoo', 'theme_park')
   OR leisure = 'golf_course'
   OR landuse = 'military'
   OR (access IN ('private', 'no') AND coalesce(nullif(highway, ''), '') = ''
       AND coalesce(nullif(landuse, ''), nullif(leisure, ''), nullif(amenity, ''), nullif(tourism, '')) IS NOT NULL
       AND ST_Area(geometry::geography) >= 1000);
CREATE INDEX ON closed_a USING gist (g);
-- Inside: off, unless the path itself is public (foot/access yes, designated, permissive).
DELETE FROM edge2 e USING closed_a a
WHERE e.geom && a.g AND ST_Intersects(a.g, ST_LineInterpolatePoint(e.geom, 0.5))
  AND coalesce(nullif(e.foot, ''), '') NOT IN ('yes', 'designated', 'permissive')
  AND coalesce(nullif(e.access, ''), '') NOT IN ('yes', 'designated', 'permissive');
-- Hand list (closed/<slug>.geojson): everything inside.
DELETE FROM edge2 e USING hand h
WHERE e.geom && h.g AND ST_Intersects(h.g, ST_LineInterpolatePoint(e.geom, 0.5));
