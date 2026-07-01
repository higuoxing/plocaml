--
-- nested calls
--
CREATE FUNCTION nested_call_one(a text) RETURNS text
LANGUAGE plocamlu
AS $$
  let a_str = PL.to_string ~default:"" args.(0) in
  let q = Printf.sprintf "SELECT nested_call_two('%s')" a_str in
  let _ = PL.execute q in
  PL.String "nested_call_one"
$$;

CREATE FUNCTION nested_call_two(a text) RETURNS text
LANGUAGE plocamlu
AS $$
  let a_str = PL.to_string ~default:"" args.(0) in
  let q = Printf.sprintf "SELECT nested_call_three('%s')" a_str in
  let _ = PL.execute q in
  PL.String "nested_call_two"
$$;

CREATE FUNCTION nested_call_three(a text) RETURNS text
LANGUAGE plocamlu
AS $$
  let a_str = PL.to_string ~default:"" args.(0) in
  PL.String a_str
$$;

SELECT nested_call_one('pass this along');

-- some spi stuff
CREATE TABLE users (fname text, lname text);
INSERT INTO users VALUES ('willem', 'doe'), ('jane', 'doe'), ('john', 'doe'), ('rick', 'smith');

CREATE FUNCTION spi_prepared_plan_test_one(a text) RETURNS text
LANGUAGE plocamlu
AS $$
  let a_str = PL.to_string ~default:"" args.(0) in
  let q = "SELECT count(*) FROM users WHERE lname = $1" in
  let rv = PL.execute_with_args q [| PL.String a_str |] in
  let count_datum = List.assoc "count" rv.rows.(0) in
  let count = PL.to_int ~default:0 count_datum in
  PL.String ("there are " ^ string_of_int count ^ " " ^ a_str ^ "s")
$$;

SELECT spi_prepared_plan_test_one('doe');

CREATE FUNCTION spi_recursive_sum(a int) RETURNS int
LANGUAGE plocamlu
AS $$
  let a_val = PL.to_int ~default:0 args.(0) in
  if a_val > 1 then
    let q = Printf.sprintf "SELECT spi_recursive_sum(%d) as a" (a_val - 1) in
    let rv = PL.execute q in
    let r_datum = List.assoc "a" rv.rows.(0) in
    let r = PL.to_int ~default:0 r_datum in
    PL.Int (a_val + r)
  else
    PL.Int a_val
$$;

SELECT spi_recursive_sum(10);
