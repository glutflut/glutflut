-- Story-driven campaigns and news flashes (campaign.rs, finds.rs, editor_notices.rs).

-- A chapter's narrated layer: {lang: {intro, fire, water, tie}}. The intro
-- shows once when the chapter starts, the ending that matches the outcome
-- when it ends and in the chronicle (/chronik.html, /api/chronicle).
ALTER TABLE game.chapters ADD COLUMN story jsonb;

-- Finds with a text ({lang: text}: a letter, a diary page) show it when
-- found and keep it in the album; finds of a campaign only play while that
-- campaign runs (first chapter start to last chapter end).
ALTER TABLE geo.finds ADD COLUMN text jsonb, ADD COLUMN campaign text;

-- News flashes of a region (a live event, a market opening): shown in the
-- app while they run, written in the editor.
CREATE TABLE game.notices (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    region text NOT NULL,
    names jsonb NOT NULL,
    url text,
    starts timestamp with time zone NOT NULL,
    ends timestamp with time zone NOT NULL,
    created_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT notices_check CHECK (ends > starts),
    CONSTRAINT notices_url_check CHECK (url IS NULL OR url ~ '^https?://')
);
CREATE INDEX notices_region_ends_idx ON game.notices (region, ends);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, UPDATE, DELETE ON game.notices TO elements_app;
  END IF;
END $$;
