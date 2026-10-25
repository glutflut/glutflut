-- Linked permanent sources ([keep] link): the partner's ticket hash, if any.
ALTER TABLE game.keeps ADD COLUMN link bytea;
