-- A post that sends itself (posts.rs): when due, the draft goes straight to
-- its channels; a failure stays on the draft for someone to send by hand.
ALTER TABLE game.posts ADD COLUMN auto_send boolean NOT NULL DEFAULT false;
