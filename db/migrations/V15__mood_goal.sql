-- Chapters can ask for "Wie ist es hier?" answers (goal 'moods', campaign.rs).
ALTER TABLE game.chapters DROP CONSTRAINT chapters_goal_check;
ALTER TABLE game.chapters ADD CONSTRAINT chapters_goal_check
    CHECK (goal = ANY (ARRAY['claims', 'fronts', 'finds', 'points', 'area', 'hold', 'moods']));
