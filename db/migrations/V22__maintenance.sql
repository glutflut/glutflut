-- Planned maintenance windows of this shard (all its cells), set on the admin
-- page. The app announces the next one 48 h ahead (`GET /api/maintenance`,
-- maintenance.rs); the maintenance page itself comes from the proxy. `note`:
-- key of a fixed extra sentence (no free text towards players).
CREATE TABLE ops.maintenance (
    id bigserial PRIMARY KEY,
    starts timestamptz NOT NULL,
    ends timestamptz NOT NULL,
    note text CHECK (note IN ('move', 'update', 'database')),
    created_by text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    cancelled_at timestamptz,
    cancelled_by text,
    CHECK (ends > starts)
);

CREATE INDEX maintenance_open ON ops.maintenance (ends) WHERE cancelled_at IS NULL;

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, UPDATE ON ops.maintenance TO elements_app;
    GRANT USAGE ON SEQUENCE ops.maintenance_id_seq TO elements_app;
  END IF;
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_read') THEN
    GRANT SELECT ON ops.maintenance TO elements_read;
  END IF;
END $$;
