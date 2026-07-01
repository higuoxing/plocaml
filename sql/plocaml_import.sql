--
-- Loose analog of PL/Python's plpython_import test. OCaml has no runtime import
-- mechanism: stdlib modules (Array, List, String, Digest, ...) are always
-- linked and used directly, and referencing an unknown module is a COMPILE
-- error rather than a catchable exception (Python's ImportError). SHA-1 is not
-- in the OCaml stdlib, so we hash with Digest (MD5) instead. Composite-type
-- arguments (plpython's import_test_two) are not supported by PL/OCaml.
--

-- Referencing a module that does not exist is a compile error; unlike Python's
-- catchable ImportError, it cannot be recovered from with try/with.
CREATE FUNCTION import_fail() RETURNS text
LANGUAGE plocamlu
AS $$
  ignore (Foosocket.connect ());
  PL.String "succeeded, that wasn't supposed to happen"
$$;

SELECT import_fail();

-- Stdlib modules are always available; just use them.
CREATE FUNCTION import_succeed() RETURNS text
LANGUAGE plocamlu
AS $$
  let doubled = List.map (fun x -> x * 2) [ 1; 2; 3 ] in
  let arr = Array.of_list doubled in
  let joined = String.concat "," (List.map string_of_int (Array.to_list arr)) in
  ignore (Printf.sprintf "%s" joined);
  PL.String "succeeded, as expected"
$$;

SELECT import_succeed();

-- Hash a string with the stdlib Digest module (MD5; SHA-1 is not in stdlib).
CREATE FUNCTION md5_test(p text) RETURNS text
LANGUAGE plocamlu
AS $$
  let p = PL.to_string ~default:"" args.(0) in
  PL.String (Digest.to_hex (Digest.string p))
$$;

SELECT md5_test('md5 hash of this string');

DROP FUNCTION import_fail;
DROP FUNCTION import_succeed;
DROP FUNCTION md5_test;
