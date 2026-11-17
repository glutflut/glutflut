-- The region at the moment a campaign chapter ended (campaign.rs): junction
-- ids per side, the totals and, for a hold chapter, who held each of its
-- junctions. Maps of the end (`/api/ops/nodes?chapter=`) draw from here, not
-- from the live state. One row per chapter, written once.
CREATE TABLE game.chapter_snapshots (
    region text NOT NULL,
    key text NOT NULL,
    taken_at timestamptz NOT NULL,
    winner text CHECK (winner IN ('fire', 'water')),
    fire_value bigint NOT NULL,
    water_value bigint NOT NULL,
    fire bigint[] NOT NULL,
    water bigint[] NOT NULL,
    front bigint[] NOT NULL,
    -- Hold: [{node, side}] in the order of chapters.nodes (side null: nobody).
    hold jsonb,
    PRIMARY KEY (region, key)
);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT ON game.chapter_snapshots TO elements_app;
  END IF;
END $$;
