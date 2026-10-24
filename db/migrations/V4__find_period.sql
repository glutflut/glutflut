-- Finds play only within a period (finds.rs): NULL on either end means no
-- bound. A find of a campaign needs both: the campaign running and now within
-- the period (a letter revealed later in the story, a seasonal find).
ALTER TABLE geo.finds
    ADD COLUMN starts timestamp with time zone,
    ADD COLUMN ends timestamp with time zone,
    ADD CONSTRAINT finds_period_check CHECK (starts IS NULL OR ends IS NULL OR ends > starts);
