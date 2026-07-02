--
-- Test named and nameless parameters
--
-- PL/OCaml exposes named SQL parameters as OCaml locals bound to args.(i),
-- alongside the positional args array. A composite argument arrives as a
-- PL.Record: a (column-name, value) association list, accessed by name like
-- PL/Python's dict. (PL/Python's NameError test has no PL/OCaml equivalent and
-- is omitted.)
--

-- Nameless parameters: access positionally via args.(i).
CREATE FUNCTION test_param_names0(integer, integer) RETURNS int AS $$
  PL.Int (PL.to_int ~default:0 args.(0) + PL.to_int ~default:0 args.(1))
$$ LANGUAGE plocamlu;

-- Named parameters are available as locals, equal to the positional args.
CREATE FUNCTION test_param_names1(a0 integer, a1 text) RETURNS boolean AS $$
  PL.Bool (a0 = args.(0) && a1 = args.(1))
$$ LANGUAGE plocamlu;

-- Named parameters can be used directly to compute the result.
CREATE FUNCTION test_param_names_add(a integer, b integer) RETURNS int AS $$
  PL.Int (PL.to_int ~default:0 a + PL.to_int ~default:0 b)
$$ LANGUAGE plocamlu;

-- Composite-type parameter: the row arrives as a PL.Record, accessed by
-- column name.

CREATE FUNCTION test_param_names2(u users) RETURNS text AS $$
  match args.(0) with
  | PL.Record _ as u ->
    let s k = PL.to_string ~default:"" (PL.field k u) in
    PL.String
      (Printf.sprintf "userid=%d username=%s fname=%s lname=%s"
         (PL.to_int ~default:0 (PL.field "userid" u))
         (s "username") (s "fname") (s "lname"))
  | PL.Null -> PL.Null
  | _ -> PL.error "expected a composite argument"; PL.Null
$$ LANGUAGE plocamlu;

-- A composite result can be built by name with PL.Record (the return-side
-- counterpart of a Record argument); field order in the list is irrelevant.
CREATE FUNCTION make_user(id int, uname text) RETURNS users AS $$
  PL.Record [ "lname", PL.String "doe";
              "userid", args.(0);
              "username", args.(1);
              "fname", PL.String "jane" ]
$$ LANGUAGE plocamlu;

-- Round-trip: composite in (Record), composite out (Record).
CREATE FUNCTION uppercase_names(u users) RETURNS users AS $$
  let up k = PL.String (String.uppercase_ascii (PL.to_string ~default:"" (PL.field k args.(0)))) in
  PL.Record [ "userid", PL.field "userid" args.(0);
              "username", PL.field "username" args.(0);
              "fname", up "fname";
              "lname", up "lname" ]
$$ LANGUAGE plocamlu;

SELECT test_param_names0(2, 7);
SELECT test_param_names1(1, 'text');
SELECT test_param_names_add(40, 2);
SELECT test_param_names2(users) FROM users ORDER BY userid;
SELECT test_param_names2(NULL);
SELECT * FROM make_user(7, 'g7');
SELECT * FROM uppercase_names(ROW('jane', 'doe', 'j_doe', 1)::users);

DROP FUNCTION test_param_names0;
DROP FUNCTION test_param_names1;
DROP FUNCTION test_param_names_add;
DROP FUNCTION test_param_names2;
DROP FUNCTION make_user;
DROP FUNCTION uppercase_names;
