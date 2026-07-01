CREATE TYPE type_record AS (
	first text,
	second integer
);

CREATE TYPE table_record AS (
	first text,
	second integer,
	retnull boolean
);

CREATE FUNCTION multiout_simple(OUT i integer, OUT j integer) AS $$
  PL.Array [| PL.Int 1; PL.Int 2 |]
$$ LANGUAGE plocaml;

SELECT multiout_simple();
SELECT * FROM multiout_simple();
SELECT i, j + 2 FROM multiout_simple();
SELECT (multiout_simple()).j + 3;

CREATE FUNCTION multiout_simple_setof(n integer DEFAULT 1, OUT integer, OUT integer) RETURNS SETOF record AS $$
  let n_val = PL.to_int ~default:1 args.(0) in
  let arr = Array.make n_val (PL.Array [| PL.Int 1; PL.Int 2 |]) in
  PL.Array arr
$$ LANGUAGE plocaml;

SELECT multiout_simple_setof();
SELECT * FROM multiout_simple_setof();
SELECT * FROM multiout_simple_setof(3);

CREATE FUNCTION multiout_return_table() RETURNS TABLE (x integer, y text) AS $$
  PL.Array [|
    PL.Array [| PL.Int 4; PL.String "four" |];
    PL.Array [| PL.Int 7; PL.String "seven" |];
    PL.Array [| PL.Int 0; PL.String "zero" |]
  |]
$$ LANGUAGE plocaml;

SELECT * FROM multiout_return_table();

CREATE FUNCTION return_record(t text) RETURNS record AS $$
  let t = args.(0) in
  PL.Array [| t; PL.Int 10 |]
$$ LANGUAGE plocaml;

SELECT * FROM return_record('abc') AS r(t text, val integer);
SELECT * FROM return_record('abc') AS r(t text, val bigint);
SELECT * FROM return_record('abc') AS r(t text, val integer);
SELECT * FROM return_record('abc') AS r(t varchar(30), val integer);
SELECT * FROM return_record('abc') AS r(t varchar(100), val integer);
SELECT * FROM return_record('999') AS r(val text, t integer);

CREATE FUNCTION return_record_2(t text) RETURNS record AS $$
  let t = args.(0) in
  PL.Array [| PL.Int 1; PL.Int 2; t |]
$$ LANGUAGE plocaml;

SELECT * FROM return_record_2('v3') AS (v1 int, v2 int, v3 text);

DROP FUNCTION multiout_simple();
DROP FUNCTION multiout_simple_setof(integer);
DROP FUNCTION multiout_return_table();
DROP FUNCTION return_record(text);
DROP FUNCTION return_record_2(text);
DROP TYPE type_record CASCADE;
DROP TYPE table_record CASCADE;
