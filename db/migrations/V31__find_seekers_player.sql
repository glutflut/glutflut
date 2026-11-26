-- Seekers of shared finds go with the player: deleting the game state (or
-- its expiry) removes them, a recovery code's new id carries them.
ALTER TABLE game.find_seekers
    ADD CONSTRAINT find_seekers_id_fkey FOREIGN KEY (id) REFERENCES game.players (id)
    ON UPDATE CASCADE ON DELETE CASCADE;
