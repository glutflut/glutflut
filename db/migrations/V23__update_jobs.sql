-- OSM updates of running regions (update-region.sh) through a queue: the
-- admin page or a nightly updater adds jobs, the updater runs them one at a
-- time and stores the report. Stage 1: dry runs only.
CREATE TABLE ops.region_updates (
    region text PRIMARY KEY,
    -- How often the updater checks the region at night.
    rhythm text NOT NULL DEFAULT 'quarterly' CHECK (rhythm IN ('off', 'monthly', 'quarterly')),
    -- Write without approval when within the limits (stage 3).
    auto boolean NOT NULL DEFAULT false,
    last_check timestamptz,
    last_write timestamptz,
    updated_by text,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE ops.update_jobs (
    id bigserial PRIMARY KEY,
    region text NOT NULL,
    kind text NOT NULL DEFAULT 'dry' CHECK (kind IN ('dry', 'write')),
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'done', 'waiting', 'approved', 'written', 'failed', 'skipped')),
    created_by text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz,
    finished_at timestamptz,
    -- Counts and verdict (update-region.sh's summary), OSM extract dates.
    summary jsonb,
    -- The Markdown report.
    report text,
    error text
);

-- At most one open job per region.
CREATE UNIQUE INDEX update_jobs_open ON ops.update_jobs (region)
    WHERE status IN ('pending', 'running', 'waiting', 'approved');
CREATE INDEX update_jobs_region ON ops.update_jobs (region, created_at DESC);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    -- The server (admin page) and the updater (stage 1 runs as the app role).
    GRANT SELECT, INSERT, UPDATE ON ops.region_updates, ops.update_jobs TO elements_app;
    GRANT USAGE ON SEQUENCE ops.update_jobs_id_seq TO elements_app;
    -- update-region.sh compares the schema versions of source and clone.
    GRANT SELECT ON public.flyway_schema_history TO elements_app;
  END IF;
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_read') THEN
    GRANT SELECT ON ops.region_updates, ops.update_jobs TO elements_read;
  END IF;
END $$;
