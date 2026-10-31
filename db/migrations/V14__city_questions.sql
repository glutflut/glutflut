-- The city's own questions (editor_questions.rs): asked in the app's
-- "Wie ist es hier?" card while they run, near a hotspot site (within
-- radius_m) or anywhere in the region. Answers are counted like ratings in
-- analytics.perception (category 'q<id>.<n>', persons in '#q<id>').
CREATE TABLE game.questions (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    region text NOT NULL,
    -- {lang: question}
    names jsonb NOT NULL,
    -- [{lang: answer}, ...], 2 to 4
    answers jsonb NOT NULL,
    site bigint REFERENCES geo.hotspot_sites (id) ON DELETE SET NULL,
    radius_m integer NOT NULL DEFAULT 250,
    starts timestamp with time zone NOT NULL,
    ends timestamp with time zone NOT NULL,
    created_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT questions_check CHECK (ends > starts),
    CONSTRAINT questions_radius_check CHECK (radius_m BETWEEN 50 AND 2000)
);
CREATE INDEX questions_region_ends_idx ON game.questions (region, ends);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, UPDATE, DELETE ON game.questions TO elements_app;
  END IF;
END $$;
