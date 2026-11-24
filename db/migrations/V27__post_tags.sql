-- Hashtags of posts: per region a list (preset: ticked for new posts), per
-- post the ones it uses; added at the end of each channel's text when the
-- draft is made.
ALTER TABLE game.posts ADD COLUMN tags text[] NOT NULL DEFAULT '{}';

CREATE TABLE game.post_tags (
    region text NOT NULL,
    -- Without "#".
    tag text NOT NULL,
    preset boolean NOT NULL DEFAULT false,
    PRIMARY KEY (region, tag)
);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, UPDATE, DELETE ON game.post_tags TO elements_app;
  END IF;
END $$;
