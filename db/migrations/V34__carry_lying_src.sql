-- Where a put-down load was filled (carry.rs `Load::from`, a junction): a
-- site takes it only from `site_min_m` away. NULL for loads from before.
ALTER TABLE game.carry_lying ADD COLUMN src bigint;
