-- Time to the first claim of new players, as two counts per day: how many
-- new identities joined, and how many of them claimed within five minutes.
ALTER TABLE analytics.daily
    ADD COLUMN joined integer DEFAULT 0 NOT NULL,
    ADD COLUMN quick integer DEFAULT 0 NOT NULL;
