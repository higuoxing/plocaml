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

DROP PROCEDURE test_proc3;
DROP TABLE test1;

--
-- plan and result objects
--
CREATE FUNCTION result_nrows_test(cmd text) RETURNS int
LANGUAGE plocamlu
AS $$
  let cmd_str = PL.to_string ~default:"" args.(0) in
  let rv = PL.execute cmd_str in
  PL.Int rv.nrows
$$;

SELECT result_nrows_test($$SELECT 1$$);
SELECT result_nrows_test($$CREATE TEMPORARY TABLE foo2 (a int, b text)$$);
SELECT result_nrows_test($$INSERT INTO foo2 VALUES (1, 'one'), (2, 'two')$$);
SELECT result_nrows_test($$UPDATE foo2 SET b = '' WHERE a = 2$$);

CREATE FUNCTION result_status_test(cmd text) RETURNS int
LANGUAGE plocamlu
AS $$
  let cmd_str = PL.to_string ~default:"" args.(0) in
  let rv = PL.execute cmd_str in
  PL.Int rv.status
$$;

SELECT result_status_test($$SELECT 1$$);
SELECT result_status_test($$CREATE TEMPORARY TABLE foo3 (a int, b text)$$);
SELECT result_status_test($$INSERT INTO foo3 VALUES (1, 'one'), (2, 'two')$$);
SELECT result_status_test($$UPDATE foo3 SET b = '' WHERE a = 2$$);

CREATE FUNCTION result_subscript_test() RETURNS void
LANGUAGE plocamlu
AS $$
  let rv = PL.execute "SELECT 1 AS c UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4" in
  let get_c row = PL.to_int ~default:0 (List.assoc "c" row) in
  PL.notice (string_of_int (get_c rv.rows.(1)));
  PL.notice (string_of_int (get_c rv.rows.(3)));
  PL.Null
$$;

SELECT result_subscript_test();

CREATE FUNCTION result_empty_test() RETURNS void
LANGUAGE plocamlu
AS $$
  let rv = PL.execute "select 1 where false" in
  PL.notice (Printf.sprintf "nrows: %d, array length: %d" rv.nrows (Array.length rv.rows));
  PL.Null
$$;

SELECT result_empty_test();

CREATE FUNCTION result_str_test(cmd text) RETURNS text
LANGUAGE plocamlu
AS $$
  let cmd_str = PL.to_string ~default:"" args.(0) in
  let plan = PL.prepare cmd_str [| |] in
  let rv = PL.execute_plan plan [| |] in
  let row_to_str row =
    let cols = List.map (fun (k, v) ->
      let v_str = match v with
        | PL.Int i -> string_of_int i
        | PL.String s -> "'" ^ s ^ "'"
        | _ -> "..."
      in
      Printf.sprintf "'%s': %s" k v_str
    ) row in
    "{" ^ String.concat ", " cols ^ "}"
  in
  let rows_str = String.concat ", " (Array.to_list (Array.map row_to_str rv.rows)) in
  PL.String (Printf.sprintf "<PLocamlResult status=%d nrows=%d rows=[%s]>" rv.status rv.nrows rows_str)
$$;

SELECT result_str_test($$SELECT 1 AS foo UNION SELECT 2$$);
SELECT result_str_test($$CREATE TEMPORARY TABLE foo1 (a int, b text)$$);

-- cursor objects
CREATE FUNCTION simple_cursor_test() RETURNS int
LANGUAGE plocamlu
AS $$
  let res = PL.cursor "select fname, lname from users" in
  let rv = PL.fetch res 100 in
  let does = ref 0 in
  Array.iter (fun row ->
    let lname = PL.to_string ~default:"" (List.assoc "lname" row) in
    if lname = "doe" then does := !does + 1
  ) rv.rows;
  PL.close res;
  PL.Int !does
$$;

SELECT simple_cursor_test();

CREATE FUNCTION double_cursor_close() RETURNS int
LANGUAGE plocamlu
AS $$
  let res = PL.cursor "select fname, lname from users" in
  PL.close res;
  PL.close res;
  PL.Null
$$;

SELECT double_cursor_close();

CREATE FUNCTION cursor_fetch() RETURNS int
LANGUAGE plocamlu
AS $$
  let res = PL.cursor "select fname, lname from users" in
  let rv1 = PL.fetch res 3 in
  if rv1.nrows <> 3 then failwith "fetch 3 failed";
  let rv2 = PL.fetch res 3 in
  if rv2.nrows <> 1 then failwith "fetch 1 failed";
  let rv3 = PL.fetch res 3 in
  if rv3.nrows <> 0 then failwith "fetch 0 failed";
  PL.close res;
  PL.Null
$$;

SELECT cursor_fetch();

CREATE FUNCTION fetch_after_close() RETURNS int
LANGUAGE plocamlu
AS $$
  let res = PL.cursor "select fname, lname from users" in
  PL.close res;
  let _ = PL.fetch res 1 in
  PL.Null
$$;

SELECT fetch_after_close();

CREATE FUNCTION cursor_plan() RETURNS text
LANGUAGE plocamlu
AS $$
  let plan = PL.prepare "select fname, lname from users where fname like $1 || '%' order by fname" [| "text" |] in
  let res1 = PL.cursor_plan plan [| PL.String "w" |] in
  let rv1 = PL.fetch res1 10 in
  let fname1 = PL.to_string ~default:"" (List.assoc "fname" rv1.rows.(0)) in
  PL.close res1;

  let res2 = PL.cursor_plan plan [| PL.String "j" |] in
  let rv2 = PL.fetch res2 10 in
  let fname2 = PL.to_string ~default:"" (List.assoc "fname" rv2.rows.(0)) in
  PL.close res2;

  PL.String (fname1 ^ ", " ^ fname2)
$$;

SELECT cursor_plan();

CREATE FUNCTION cursor_plan_wrong_args() RETURNS text
LANGUAGE plocamlu
AS $$
  let plan = PL.prepare "select fname, lname from users where fname like $1 || '%'" [| "text" |] in
  let _ = PL.cursor_plan plan [| PL.String "a"; PL.String "b" |] in
  PL.String "should not reach here"
$$;
SELECT cursor_plan_wrong_args();
CREATE FUNCTION execute_plan_wrong_args() RETURNS text
LANGUAGE plocamlu
AS $$
  let plan = PL.prepare "select fname, lname from users where fname like $1 || '%'" [| "text" |] in
  let _ = PL.execute_plan plan [| PL.String "a"; PL.String "b" |] in
  PL.String "should not reach here"
$$;
SELECT execute_plan_wrong_args();
