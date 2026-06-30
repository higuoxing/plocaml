CREATE EXTENSION IF NOT EXISTS plocaml;

CREATE TABLE test1 (a int);

CREATE PROCEDURE test_proc3(x int)
LANGUAGE plocaml
AS $$
  let x_val = PL.to_int ~default:0 args.(0) in
  let query = "INSERT INTO test1 VALUES (" ^ string_of_int x_val ^ ")" in
  let _ = PL.execute query in
  PL.Null
$$;
CALL test_proc3(55);

SELECT * FROM test1;

DROP PROCEDURE test_proc3;
DROP TABLE test1;
