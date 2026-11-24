-- Accounts the server sends posts with whose token it renews itself
-- (Instagram: 60 days). The token comes from the server's environment
-- (ELEMENTS_POST_INSTAGRAM_<REGION>); `seed` is the hash of that one, so a
-- new token there replaces the stored one. A server without the variable
-- ignores the row (dev after a reset from prod).
CREATE TABLE game.post_accounts (
    region text NOT NULL,
    channel text NOT NULL CHECK (channel IN ('instagram')),
    seed text NOT NULL,
    token text NOT NULL,
    user_id text NOT NULL,
    username text,
    expires_at timestamptz,
    refreshed_at timestamptz NOT NULL DEFAULT now(),
    -- The last renewal's error, if it failed.
    error text,
    PRIMARY KEY (region, channel)
);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, UPDATE, DELETE ON game.post_accounts TO elements_app;
  END IF;
END $$;
