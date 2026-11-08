-- Link keys ([keep] link_keys): a key used for a link into another region
-- is bound to that link (hash of its near end) at its first claim.
ALTER TABLE game.keep_keys ADD COLUMN bound bytea;
