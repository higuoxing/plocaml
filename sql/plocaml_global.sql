CREATE EXTENSION IF NOT EXISTS plocamlu;

--
-- check static and global data (SD and GD), mirroring PL/Python's
-- plpython_global test. SD is private to each function; GD is shared across
-- all functions in the session.
--

CREATE FUNCTION global_test_one() RETURNS text
LANGUAGE plocamlu
AS $$
  if not (Hashtbl.mem sd "global_test") then
    PL.set sd "global_test" "set by global_test_one";
  if not (Hashtbl.mem gd "global_test") then
    PL.set gd "global_test" "set by global_test_one";
  PL.String ("SD: " ^ PL.get sd "global_test" ^ ", GD: " ^ PL.get gd "global_test")
$$;

CREATE FUNCTION global_test_two() RETURNS text
LANGUAGE plocamlu
AS $$
  if not (Hashtbl.mem sd "global_test") then
    PL.set sd "global_test" "set by global_test_two";
  if not (Hashtbl.mem gd "global_test") then
    PL.set gd "global_test" "set by global_test_two";
  PL.String ("SD: " ^ PL.get sd "global_test" ^ ", GD: " ^ PL.get gd "global_test")
$$;

CREATE FUNCTION static_test() RETURNS int4
LANGUAGE plocamlu
AS $$
  let n = match PL.get_opt sd "call" with Some c -> c + 1 | None -> 1 in
  PL.set sd "call" n;
  PL.Int n
$$;

SELECT static_test();
SELECT static_test();
SELECT global_test_one();
SELECT global_test_two();

DROP FUNCTION global_test_one;
DROP FUNCTION global_test_two;
DROP FUNCTION static_test;
