-- After the nodes got their heights (import-terrain-region.sh): bridge heights, then the edges'
-- grades and element costs. Variables: rid, kfire, kwater, cmin, cmax,
-- demres, maxgrade, scope (all: every edge; bridges: only the edges at
-- bridge junctions).

-- Bridges: the terrain models drop them, so a junction on a bridge got the
-- water or valley below. Junctions whose active edges are all bridge edges
-- take the length-weighted mean of their neighbours along the bridge, again
-- and again, so heights run straight between the abutments (which keep
-- theirs). 1000 rounds settle chains of about 30 junctions between abutments.
CREATE TEMP TABLE bridge_e ON COMMIT DROP AS
SELECT e.source AS a, e.target AS b, 1.0 / GREATEST(e.length_m, 1.0) AS w
FROM geo.edges e WHERE e.region_id = :'rid' AND e.is_active AND e.bridge;
CREATE TEMP TABLE ground_n ON COMMIT DROP AS
SELECT source AS id FROM geo.edges WHERE region_id = :'rid' AND is_active AND NOT bridge
UNION SELECT target FROM geo.edges WHERE region_id = :'rid' AND is_active AND NOT bridge;
CREATE TEMP TABLE bridge_n ON COMMIT DROP AS
SELECT id FROM (SELECT a AS id FROM bridge_e UNION SELECT b FROM bridge_e) q
EXCEPT SELECT id FROM ground_n;
CREATE TEMP TABLE bridge_nb ON COMMIT DROP AS
SELECT a AS id, b AS nb, w FROM bridge_e UNION ALL SELECT b, a, w FROM bridge_e;
CREATE TEMP TABLE bridge_z ON COMMIT DROP AS
SELECT n.id, n.elevation_m::double precision AS z FROM geo.nodes n
WHERE n.id IN (SELECT nb FROM bridge_nb);
CREATE INDEX ON bridge_z (id);
CREATE INDEX ON bridge_nb (id);
DO $$
BEGIN
  FOR i IN 1..1000 LOOP
    UPDATE bridge_z z SET z = m.z
    FROM (SELECT b.id, sum(nz.z * b.w) / sum(b.w) AS z
            FROM bridge_n n JOIN bridge_nb b ON b.id = n.id JOIN bridge_z nz ON nz.id = b.nb
           GROUP BY b.id) m
    WHERE z.id = m.id;
  END LOOP;
END $$;
UPDATE geo.nodes n SET elevation_m = round(z.z::numeric, 2)
FROM bridge_z z JOIN bridge_n b ON b.id = z.id
WHERE n.id = z.id;
SELECT count(*) AS bridge_junctions FROM bridge_n;

-- Edges: endpoint elevations from the now-sampled nodes; grade = rise/run.
-- Directional element costs are "preferred, not strict": a multiplicative tilt
-- by grade, clamped to [cmin,cmax] so neither element is ever blocked.
--   fire rises  -> cheaper uphill   (cost ~ 1 - k*grade in the travel direction)
--   water flows -> cheaper downhill (cost ~ 1 + k*grade in the travel direction)
WITH g AS (
  SELECT e.id,
         sn.elevation_m AS ef,
         tn.elevation_m AS et,
         GREATEST(e.length_m, 1.0) AS len,
         -- grade resolved at no finer than the DEM footprint, then clamped.
         LEAST(:maxgrade, GREATEST(-:maxgrade,
           (tn.elevation_m - sn.elevation_m) / GREATEST(e.length_m, :demres)
         )) AS gf
  FROM geo.edges e
  JOIN geo.nodes sn ON sn.id = e.source
  JOIN geo.nodes tn ON tn.id = e.target
  WHERE e.region_id = :'rid'
    AND (:'scope' = 'all' OR e.source IN (SELECT id FROM bridge_n) OR e.target IN (SELECT id FROM bridge_n))
)
UPDATE geo.edges e SET
  elev_from_m      = round(g.ef::numeric, 2),
  elev_to_m        = round(g.et::numeric, 2),
  grade_forward    = round(g.gf::numeric, 5),
  grade_reverse    = round((-g.gf)::numeric, 5),
  base_travel_cost = round(g.len::numeric, 3),
  fire_cost_forward  = round((g.len * LEAST(:cmax, GREATEST(:cmin, 1 - :kfire  * g.gf)))::numeric, 3),
  fire_cost_reverse  = round((g.len * LEAST(:cmax, GREATEST(:cmin, 1 + :kfire  * g.gf)))::numeric, 3),
  water_cost_forward = round((g.len * LEAST(:cmax, GREATEST(:cmin, 1 + :kwater * g.gf)))::numeric, 3),
  water_cost_reverse = round((g.len * LEAST(:cmax, GREATEST(:cmin, 1 - :kwater * g.gf)))::numeric, 3)
FROM g WHERE e.id = g.id;
