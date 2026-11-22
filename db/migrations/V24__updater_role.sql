-- Writing OSM updates after approval (update-region.sh --write from the
-- updater): its own role with exactly the tables a write reads and changes,
-- so the machine running it needs neither the owner nor more rights for the
-- game role. Created without login; an operator gives it a password
-- (ALTER ROLE elements_updater LOGIN PASSWORD '…') where it is used.
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_updater') THEN
    BEGIN
      CREATE ROLE elements_updater NOLOGIN;
    EXCEPTION WHEN insufficient_privilege THEN
      RAISE NOTICE 'no CREATEROLE: create elements_updater by hand, then re-run the grants of V24';
    END;
  END IF;
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_updater') THEN
    GRANT USAGE ON SCHEMA geo, game, ops TO elements_updater;
    GRANT SELECT ON ALL TABLES IN SCHEMA geo TO elements_updater;
    GRANT SELECT ON game.keeps, game.keep_keys, game.hotspot_plans, public.flyway_schema_history TO elements_updater;
    GRANT INSERT, UPDATE ON geo.nodes, geo.edges, geo.finds TO elements_updater;
    GRANT INSERT, UPDATE, DELETE ON geo.blocks, geo.node_moves TO elements_updater;
    GRANT UPDATE ON geo.regions, geo.hotspot_sites, game.hotspot_plans TO elements_updater;
    GRANT USAGE ON ALL SEQUENCES IN SCHEMA geo TO elements_updater;
    -- Row security (V1) has only read policies: writes for this role.
    CREATE POLICY nodes_updater ON geo.nodes FOR ALL TO elements_updater USING (true) WITH CHECK (true);
    CREATE POLICY edges_updater ON geo.edges FOR ALL TO elements_updater USING (true) WITH CHECK (true);
    CREATE POLICY regions_updater ON geo.regions FOR ALL TO elements_updater USING (true) WITH CHECK (true);
  END IF;
END $$;

-- Approval of a waiting job (admin page): by whom, and whether it may run at
-- once instead of the next night.
ALTER TABLE ops.update_jobs
    ADD COLUMN approved_by text,
    ADD COLUMN approved_at timestamptz,
    ADD COLUMN run_now boolean NOT NULL DEFAULT false;
-- The places counted at the last write (the base of the next update's
-- sources, formerly REASONS_BASE in the recipe).
ALTER TABLE ops.region_updates ADD COLUMN places text;
