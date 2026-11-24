-- A post's place for Instagram: {id, name}, id of a Facebook page with a
-- location (Instagram's location_id). Mastodon has no such thing.
ALTER TABLE game.posts ADD COLUMN location jsonb;
