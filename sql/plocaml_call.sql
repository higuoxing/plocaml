CREATE EXTENSION IF NOT EXISTS plocamlu;

--
-- Tests for procedures / CALL syntax
--
CREATE PROCEDURE test_proc1()
LANGUAGE plocamlu
AS $$
  PL.Null
$$;
CALL test_proc1();

-- error: can't return non-Null
CREATE PROCEDURE test_proc2()
LANGUAGE plocamlu
AS $$
  PL.Int 5
$$;
CALL test_proc2();

CREATE TABLE test1 (a int);
CREATE PROCEDURE test_proc3(x int)
LANGUAGE plocamlu
AS $$
  let x_val = PL.to_int ~default:0 args.(0) in
  let query = "INSERT INTO test1 VALUES (" ^ string_of_int x_val ^ ")" in
  let _ = PL.execute query in
  PL.Null
$$;
CALL test_proc3(55);
SELECT * FROM test1;

-- output arguments
CREATE PROCEDURE test_proc5(INOUT a text)
LANGUAGE plocamlu
AS $$
  let a_str = PL.to_string ~default:"" args.(0) in
  PL.Array [| PL.String (a_str ^ "+" ^ a_str) |]
$$;
CALL test_proc5('abc');

CREATE PROCEDURE test_proc6(a int, INOUT b int, INOUT c int)
LANGUAGE plocamlu
AS $$
  let a_val = PL.to_int ~default:0 args.(0) in
  let b_val = PL.to_int ~default:0 args.(1) in
  let c_val = PL.to_int ~default:0 args.(2) in
  PL.Array [| PL.Int (b_val * a_val); PL.Int (c_val * a_val) |]
$$;
CALL test_proc6(2, 3, 4);

-- OUT parameters
CREATE PROCEDURE test_proc9(IN a int, OUT b int)
LANGUAGE plocamlu
AS $$
  let a_val = PL.to_int ~default:0 args.(0) in
  PL.notice ("a: " ^ string_of_int a_val);
  PL.Array [| PL.Int (a_val * 2) |]
$$;

DO $$
DECLARE _a int; _b int;
BEGIN
  _a := 10; _b := 30;
  CALL test_proc9(_a, _b);
  RAISE NOTICE '_a: %, _b: %', _a, _b;
END
$$;

DROP PROCEDURE test_proc1;
DROP PROCEDURE test_proc2;
DROP PROCEDURE test_proc3;
DROP TABLE test1;

-- elog tests
CREATE PROCEDURE test_elog()
LANGUAGE plocamlu
AS $$
  PL.elog PL.Warning "This is a warning";
  PL.elog PL.Info "This is an info";
  PL.Null
$$;
CALL test_elog();
DROP PROCEDURE test_elog;
