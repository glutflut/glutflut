-- Feedback marked done on the admin page (admin.html): when and by whom
-- (token label or login). elements_app already may UPDATE game.feedback.
ALTER TABLE game.feedback ADD COLUMN done_at timestamptz, ADD COLUMN done_by text;
