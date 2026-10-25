-- Points since the campaign start, per region (the "Kampagne" ranking).
ALTER TABLE game.player_region ADD COLUMN points_campaign bigint DEFAULT 0 NOT NULL;
