--
-- Test returning tuples
--

CREATE TABLE table_record (
	first text,
	second int4
	) ;

CREATE TYPE type_record AS (
	first text,
	second int4
	) ;


CREATE FUNCTION test_table_record_as(typ text, first text, second integer, retnull boolean) RETURNS table_record AS $$
  let retnull = PL.to_bool_exn retnull in
  if retnull then PL.Null
  else
    let typ = PL.to_string_exn typ in
    match typ with
    | "dict" ->
        PL.Record [ "first", first; "second", second; "additionalfield", PL.String "must not cause trouble" ]
    | "tuple" | "list" ->
        PL.Array [| first; second |]
    | _ -> failwith "unsupported typ"
$$ LANGUAGE plocamlu;

CREATE FUNCTION test_type_record_as(typ text, first text, second integer, retnull boolean) RETURNS type_record AS $$
  let retnull = PL.to_bool_exn retnull in
  if retnull then PL.Null
  else
    let typ = PL.to_string_exn typ in
    match typ with
    | "dict" ->
        PL.Record [ "first", first; "second", second; "additionalfield", PL.String "must not cause trouble" ]
    | "tuple" | "list" ->
        PL.Array [| first; second |]
    | "str" ->
        let f = PL.to_string ~default:"None" first in
        let s = match PL.to_int_opt second with Some x -> string_of_int x | None -> "None" in
        PL.String (Printf.sprintf "(%s,%s)" f s)
    | _ -> failwith "unsupported typ"
$$ LANGUAGE plocamlu;

CREATE FUNCTION test_in_out_params(first in text, second out text) AS $$
  let f = PL.to_string_exn first in
  PL.String (f ^ "_in_to_out")
$$ LANGUAGE plocamlu;

CREATE FUNCTION test_in_out_params_multi(first in text,
                                         second out text, third out text) AS $$
  let f = PL.to_string_exn first in
  PL.Array [| PL.String (f ^ "_record_in_to_out_1"); PL.String (f ^ "_record_in_to_out_2") |]
$$ LANGUAGE plocamlu;

CREATE FUNCTION test_inout_params(first inout text) AS $$
  let f = PL.to_string_exn first in
  PL.String (f ^ "_inout")
$$ LANGUAGE plocamlu;


-- Test tuple returning functions
SELECT * FROM test_table_record_as('dict', null, null, false);
SELECT * FROM test_table_record_as('dict', 'one', null, false);
SELECT * FROM test_table_record_as('dict', null, 2, false);
SELECT * FROM test_table_record_as('dict', 'three', 3, false);
SELECT * FROM test_table_record_as('dict', null, null, true);

SELECT * FROM test_table_record_as('tuple', null, null, false);
SELECT * FROM test_table_record_as('tuple', 'one', null, false);
SELECT * FROM test_table_record_as('tuple', null, 2, false);
SELECT * FROM test_table_record_as('tuple', 'three', 3, false);
SELECT * FROM test_table_record_as('tuple', null, null, true);

SELECT * FROM test_table_record_as('list', null, null, false);
SELECT * FROM test_table_record_as('list', 'one', null, false);
SELECT * FROM test_table_record_as('list', null, 2, false);
SELECT * FROM test_table_record_as('list', 'three', 3, false);
SELECT * FROM test_table_record_as('list', null, null, true);

SELECT * FROM test_type_record_as('dict', null, null, false);
SELECT * FROM test_type_record_as('dict', 'one', null, false);
SELECT * FROM test_type_record_as('dict', null, 2, false);
SELECT * FROM test_type_record_as('dict', 'three', 3, false);
SELECT * FROM test_type_record_as('dict', null, null, true);

SELECT * FROM test_type_record_as('tuple', null, null, false);
SELECT * FROM test_type_record_as('tuple', 'one', null, false);
SELECT * FROM test_type_record_as('tuple', null, 2, false);
SELECT * FROM test_type_record_as('tuple', 'three', 3, false);
SELECT * FROM test_type_record_as('tuple', null, null, true);

SELECT * FROM test_type_record_as('list', null, null, false);
SELECT * FROM test_type_record_as('list', 'one', null, false);
SELECT * FROM test_type_record_as('list', null, 2, false);
SELECT * FROM test_type_record_as('list', 'three', 3, false);
SELECT * FROM test_type_record_as('list', null, null, true);

SELECT * FROM test_type_record_as('str', 'one', 1, false);

SELECT * FROM test_in_out_params('test_in');
SELECT * FROM test_in_out_params_multi('test_in');
SELECT * FROM test_inout_params('test_in');

-- try changing the return types and call functions again

ALTER TABLE table_record DROP COLUMN first;
ALTER TABLE table_record DROP COLUMN second;
ALTER TABLE table_record ADD COLUMN first text;
ALTER TABLE table_record ADD COLUMN second int4;

SELECT * FROM test_table_record_as('dict', 'one', 1, false);

ALTER TYPE type_record DROP ATTRIBUTE first;
ALTER TYPE type_record DROP ATTRIBUTE second;
ALTER TYPE type_record ADD ATTRIBUTE first text;
ALTER TYPE type_record ADD ATTRIBUTE second int4;

SELECT * FROM test_type_record_as('dict', 'one', 1, false);

-- errors cases

CREATE FUNCTION test_type_record_error1() RETURNS type_record AS $$
    PL.Record [ "first", PL.String "first" ]
$$ LANGUAGE plocamlu;

SELECT * FROM test_type_record_error1();


CREATE FUNCTION test_type_record_error2() RETURNS type_record AS $$
    PL.Array [| PL.String "first" |]
$$ LANGUAGE plocamlu;

SELECT * FROM test_type_record_error2();


CREATE FUNCTION test_type_record_error3() RETURNS type_record AS $$
    PL.Record [ "first", PL.String "first" ]
$$ LANGUAGE plocamlu;

SELECT * FROM test_type_record_error3();

CREATE FUNCTION test_type_record_error4() RETURNS type_record AS $$
    PL.String "foo"
$$ LANGUAGE plocamlu;

SELECT * FROM test_type_record_error4();
