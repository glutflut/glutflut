-- Posts from the game (posts.rs): planned per region in the editor; a
-- trigger (a time, a chapter's start or end) makes a draft with the texts
-- for the outcome and the picture of the game state (card.rs). Someone looks
-- at it and sends it, or discards it. Account keys never live here.
CREATE TABLE game.posts (
    id bigserial PRIMARY KEY,
    region text NOT NULL,
    -- Only in the editor.
    title text NOT NULL,
    trigger text NOT NULL CHECK (trigger IN ('at', 'chapter_start', 'chapter_end', 'manual')),
    at timestamptz,
    chapter text,
    -- Minutes after the chapter's start or end (may be negative).
    offset_min integer NOT NULL DEFAULT 0,
    -- {any|fire|water|tie: {instagram, mastodon, alt}}; {fire} {water} {chapter} filled in.
    texts jsonb NOT NULL,
    -- {lat, lon, width_m, title, note}; null: no picture.
    card jsonb,
    channels text[] NOT NULL DEFAULT '{instagram,mastodon}',
    created_by text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (trigger <> 'at' OR at IS NOT NULL),
    CHECK (trigger NOT IN ('chapter_start', 'chapter_end') OR chapter IS NOT NULL)
);

CREATE INDEX posts_region_idx ON game.posts (region);

-- A draft of a post and what became of it. One automatic draft per post;
-- "draft now" in the editor makes more.
CREATE TABLE game.post_runs (
    id bigserial PRIMARY KEY,
    post_id bigint NOT NULL REFERENCES game.posts (id) ON DELETE CASCADE,
    region text NOT NULL,
    auto boolean NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    state text NOT NULL DEFAULT 'draft' CHECK (state IN ('draft', 'sent', 'discarded')),
    outcome text CHECK (outcome IN ('fire', 'water', 'tie')),
    -- {fire, water}
    numbers jsonb,
    -- {instagram, mastodon, alt}, filled in.
    texts jsonb NOT NULL,
    image bytea,
    -- Why there is no picture, and the like.
    note text,
    decided_by text,
    decided_at timestamptz,
    -- Per channel {url} or {error} (sending, later).
    results jsonb
);

CREATE UNIQUE INDEX post_runs_auto_once ON game.post_runs (post_id) WHERE auto;
CREATE INDEX post_runs_region_idx ON game.post_runs (region, created_at);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, UPDATE, DELETE ON game.posts, game.post_runs TO elements_app;
    GRANT USAGE ON SEQUENCE game.posts_id_seq, game.post_runs_id_seq TO elements_app;
  END IF;
END $$;
