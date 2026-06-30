--
-- Tests for functions that return void
--

CREATE FUNCTION test_void_func1() RETURNS void AS $$
  PL.Null
$$ LANGUAGE plocaml;

-- illegal: can't return non-Null value in void-returning func
CREATE FUNCTION test_void_func2() RETURNS void AS $$
  PL.Int 10
$$ LANGUAGE plocaml;

CREATE FUNCTION test_return_none() RETURNS int AS $$
  PL.Null
$$ LANGUAGE plocaml;

-- Tests for functions returning void
SELECT test_void_func1(), test_void_func1() IS NULL AS "is null";

SELECT test_void_func2(); -- should fail

SELECT test_return_none(), test_return_none() IS NULL AS "is null";

DROP FUNCTION test_void_func1;
DROP FUNCTION test_void_func2;
DROP FUNCTION test_return_none;
