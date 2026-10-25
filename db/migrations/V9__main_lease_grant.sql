-- V8 forgot the app's rights on ops.main_lease (the cells' poll failed).
DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT SELECT, INSERT, UPDATE, DELETE ON ops.main_lease TO elements_app;
  END IF;
END $$;
