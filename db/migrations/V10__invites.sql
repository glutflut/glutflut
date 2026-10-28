-- Invites: a player's code (derived from the id, also in score.rs
-- `invite_code`), and who joined through whose link. Only days, no times.
ALTER TABLE game.players
  ADD COLUMN invite text GENERATED ALWAYS AS
    (substr(encode(sha256('invite:'::bytea || id), 'hex'), 1, 12)) STORED;
CREATE INDEX players_invite_idx ON game.players (invite);

CREATE TABLE game.invites (
  referee bytea PRIMARY KEY REFERENCES game.players(id) ON UPDATE CASCADE ON DELETE CASCADE,
  referrer bytea NOT NULL REFERENCES game.players(id) ON UPDATE CASCADE ON DELETE CASCADE,
  day date NOT NULL,
  qualified boolean NOT NULL DEFAULT false,
  rewarded boolean NOT NULL DEFAULT false
);
CREATE INDEX invites_referrer_idx ON game.invites (referrer);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, UPDATE, DELETE ON game.invites TO elements_app;
  END IF;
END $$;
