-- Own pictures of a post (posts.rs): uploaded in the editor, mixed with the
-- game card in the post's order. JPEG as uploaded (upright, at most 2048 px,
-- no EXIF); made 4:5 when a draft is made. post_id is NULL until the post is
-- saved (unsaved ones go after a day).
CREATE TABLE game.post_images (
    id bigserial PRIMARY KEY,
    region text NOT NULL,
    post_id bigint REFERENCES game.posts (id) ON DELETE CASCADE,
    jpeg bytea NOT NULL,
    width integer NOT NULL,
    height integer NOT NULL,
    created_by text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX post_images_post_idx ON game.post_images (post_id);
CREATE INDEX post_images_region_idx ON game.post_images (region, created_at);

-- The pictures in order: "card" or {id, alt, fit}; NULL: the card alone (if any).
ALTER TABLE game.posts ADD COLUMN pictures jsonb;

-- A draft's own pictures in order with their alt texts; card_at: where the
-- card (image) goes among them.
ALTER TABLE game.post_runs
    ADD COLUMN extra bytea[],
    ADD COLUMN extra_alt text[],
    ADD COLUMN card_at integer;

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, UPDATE, DELETE ON game.post_images TO elements_app;
    GRANT USAGE ON SEQUENCE game.post_images_id_seq TO elements_app;
  END IF;
END $$;
