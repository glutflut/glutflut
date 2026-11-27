-- Instagram places of posts per region (like post_tags): picked in the post
-- form; preset: the one new posts start with (at most one per region). Places
-- used in posts join the list.
CREATE TABLE game.post_places (
    region text NOT NULL,
    -- The Facebook page id (digits).
    id text NOT NULL,
    name text NOT NULL DEFAULT '',
    preset boolean NOT NULL DEFAULT false,
    PRIMARY KEY (region, id)
);

CREATE UNIQUE INDEX post_places_one_preset ON game.post_places (region) WHERE preset;

INSERT INTO game.post_places (region, id, name)
SELECT DISTINCT ON (region, location ->> 'id') region, location ->> 'id', coalesce(location ->> 'name', '')
FROM game.posts
WHERE location ->> 'id' ~ '^[0-9]{1,30}$'
ORDER BY region, location ->> 'id', id DESC
ON CONFLICT DO NOTHING;

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, UPDATE, DELETE ON game.post_places TO elements_app;
  END IF;
END $$;
