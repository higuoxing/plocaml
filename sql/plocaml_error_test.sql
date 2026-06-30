CREATE FUNCTION test_pg_error() RETURNS void
LANGUAGE plocaml
AS $$
  let _ = PL.execute "SELECT * FROM table_that_does_not_exist" in
  PL.Null
$$;
SELECT test_pg_error();
