-- Successors of junctions an OSM update switched off (update-region.sh): the
-- nearest active junction within 50 m. The server maps saved state on an old
-- id to the new one when it loads a region (permanent sources, their keys,
-- saved claims); written only by the import, chains already collapsed.
CREATE TABLE geo.node_moves (
    region text NOT NULL,
    old_id bigint NOT NULL,
    new_id bigint NOT NULL,
    moved_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (region, old_id)
);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT ON geo.node_moves TO elements_app;
  END IF;
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_read') THEN
    GRANT SELECT ON geo.node_moves TO elements_read;
  END IF;
END $$;
