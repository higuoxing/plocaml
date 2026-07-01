--
-- Mirror of PL/Python's plpython_error test, covering the error-handling
-- behavior that maps onto PL/OCaml. Errors raised through PL.execute now
-- preserve their SQLSTATE, so plpgsql can catch them by condition.
--
-- Omitted (no PL/OCaml equivalent): typed spiexceptions caught by class,
-- raising spiexceptions with a custom sqlstate, and the Python traceback /
-- SHOW_CONTEXT stack-trace tests.
--

-- Flat-out OCaml syntax error. PL/OCaml compiles a function lazily on first
-- call, so the error shows up when called. Run twice: a failed compile is not
-- cached, so both calls report the error.
CREATE FUNCTION ocaml_syntax_error() RETURNS text
LANGUAGE plocamlu
AS $$
  let x =
$$;

SELECT ocaml_syntax_error();
SELECT ocaml_syntax_error();

-- Flat-out SQL syntax error.
CREATE FUNCTION sql_syntax_error() RETURNS text
LANGUAGE plocamlu
AS $$
  ignore (PL.execute "syntax error");
  PL.Null
$$;

SELECT sql_syntax_error();

-- Uncaught OCaml exception: array index out of bounds.
CREATE FUNCTION index_invalid(a text) RETURNS text
LANGUAGE plocamlu
AS $$
  args.(1)
$$;

SELECT index_invalid('test');

-- SPI error surfacing a plain PostgreSQL error (missing relation).
CREATE FUNCTION test_pg_error() RETURNS void
LANGUAGE plocamlu
AS $$
  let _ = PL.execute "SELECT * FROM table_that_does_not_exist" in
  PL.Null
$$;

SELECT test_pg_error();

-- Nested SPI error: calling a function that does not exist.
CREATE FUNCTION nested_undefined() RETURNS text
LANGUAGE plocamlu
AS $$
  ignore (PL.execute "SELECT no_such_func('foo')");
  PL.Null
$$;

SELECT nested_undefined();

-- A bad type name in PL.prepare, left uncaught.
CREATE FUNCTION invalid_type_uncaught() RETURNS text
LANGUAGE plocamlu
AS $$
  ignore (PL.prepare "SELECT $1::text" [| "nonexistent_type" |]);
  PL.Null
$$;

SELECT invalid_type_uncaught();

-- Catch the error and return NULL.
CREATE FUNCTION invalid_type_caught() RETURNS text
LANGUAGE plocamlu
AS $$
  (try
     ignore (PL.prepare "SELECT $1::text" [| "nonexistent_type" |]);
     PL.Null
   with Failure msg -> PL.notice msg; PL.Null)
$$;

SELECT invalid_type_caught();

-- Catch the error and re-raise it as a plain error.
CREATE FUNCTION invalid_type_reraised() RETURNS text
LANGUAGE plocamlu
AS $$
  (try
     ignore (PL.prepare "SELECT $1::text" [| "nonexistent_type" |]);
     PL.Null
   with Failure msg -> PL.error msg; PL.Null)
$$;

SELECT invalid_type_reraised();

-- Error raised from nested (local) functions.
CREATE FUNCTION nested_error() RETURNS text
LANGUAGE plocamlu
AS $$
  let fun1 () = PL.error "boom" in
  let fun2 () = fun1 () in
  let fun3 () = fun2 () in
  fun3 ();
  PL.String "not reached"
$$;

SELECT nested_error();

-- PL.warning from a nested function should not abort execution.
CREATE FUNCTION nested_warning() RETURNS text
LANGUAGE plocamlu
AS $$
  let fun1 () = PL.warning "boom" in
  fun1 ();
  PL.String "you've been warned"
$$;

SELECT nested_warning();

-- Referencing a non-existent value is a compile error (the analog of Python's
-- toplevel AttributeError).
CREATE FUNCTION unbound_value_error() RETURNS void
LANGUAGE plocamlu
AS $$
  PL.nonexistent
$$;

SELECT unbound_value_error();

-- Calling PL/OCaml from SQL and vice versa should not lose the error.
CREATE OR REPLACE FUNCTION sql_error() RETURNS void AS $$
begin
  perform 1/0;
end
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION ocaml_traceback() RETURNS void
LANGUAGE plocamlu
AS $$
  ignore (PL.execute "select sql_error()");
  PL.Null
$$;

SELECT sql_error();
SELECT ocaml_traceback();

-- SPI errors in PL/OCaml functions preserve the SQLSTATE value, so plpgsql can
-- catch them by condition.
CREATE TABLE specific (i integer PRIMARY KEY);

CREATE FUNCTION ocaml_unique_violation() RETURNS void
LANGUAGE plocamlu
AS $$
  ignore (PL.execute "insert into specific values (1)");
  ignore (PL.execute "insert into specific values (1)");
  PL.Null
$$;

CREATE FUNCTION catch_ocaml_unique_violation() RETURNS text AS $$
begin
    begin
        perform ocaml_unique_violation();
    exception when unique_violation then
        return 'ok';
    end;
    return 'not reached';
end;
$$ LANGUAGE plpgsql;

SELECT catch_ocaml_unique_violation();

-- Manually starting subtransactions - a bad idea.
CREATE FUNCTION manual_subxact() RETURNS void
LANGUAGE plocamlu
AS $$
  ignore (PL.execute "savepoint save");
  PL.Null
$$;

SELECT manual_subxact();

-- Error carrying a DETAIL string, surfaced through PL.execute (bug #18070).
CREATE FUNCTION ocaml_error_detail() RETURNS text
LANGUAGE plocamlu
AS $$
  ignore (PL.execute "SELECT to_date('xy', 'DD')");
  PL.Null
$$;

SELECT ocaml_error_detail();

DROP FUNCTION ocaml_syntax_error;
DROP FUNCTION sql_syntax_error;
DROP FUNCTION index_invalid;
DROP FUNCTION test_pg_error;
DROP FUNCTION nested_undefined;
DROP FUNCTION invalid_type_uncaught;
DROP FUNCTION invalid_type_caught;
DROP FUNCTION invalid_type_reraised;
DROP FUNCTION nested_error;
DROP FUNCTION nested_warning;
DROP FUNCTION unbound_value_error;
DROP FUNCTION ocaml_traceback;
DROP FUNCTION sql_error;
DROP FUNCTION ocaml_unique_violation;
DROP FUNCTION catch_ocaml_unique_violation;
DROP FUNCTION manual_subxact;
DROP FUNCTION ocaml_error_detail;
DROP TABLE specific;
