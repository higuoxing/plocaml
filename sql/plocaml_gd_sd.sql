CREATE EXTENSION IF NOT EXISTS plocamlu;

--
-- Tests for the GD (global, session-wide) and SD (per-function) stores,
-- mirroring PL/Python. A store holds values of ANY type; use PL.set / PL.get /
-- PL.get_opt (the type read back must match the type written). Both are
-- exposed to function bodies as the bare names `gd` and `sd`.
--

-- SD persists across calls to the same function within a session.
CREATE FUNCTION sd_counter() RETURNS int
LANGUAGE plocamlu
AS $$
  let n = match PL.get_opt sd "n" with Some x -> x | None -> 0 in
  PL.set sd "n" (n + 1);
  PL.Int (n + 1)
$$;

SELECT sd_counter();
SELECT sd_counter();
SELECT sd_counter();

-- SD is private to each function: two functions using the same key don't
-- interfere with each other.
CREATE FUNCTION sd_a() RETURNS int
LANGUAGE plocamlu
AS $$
  let n = match PL.get_opt sd "k" with Some x -> x | None -> 0 in
  PL.set sd "k" (n + 10);
  PL.Int (n + 10)
$$;

CREATE FUNCTION sd_b() RETURNS int
LANGUAGE plocamlu
AS $$
  let n = match PL.get_opt sd "k" with Some x -> x | None -> 0 in
  PL.set sd "k" (n + 100);
  PL.Int (n + 100)
$$;

SELECT sd_a();
SELECT sd_b();
SELECT sd_a();
SELECT sd_b();

-- GD is shared across all functions in the session.
CREATE FUNCTION gd_set(v int) RETURNS void
LANGUAGE plocamlu
AS $$
  PL.set gd "shared" (PL.to_int ~default:0 args.(0));
  PL.Null
$$;

CREATE FUNCTION gd_get() RETURNS int
LANGUAGE plocamlu
AS $$
  match PL.get_opt gd "shared" with Some x -> PL.Int x | None -> PL.Null
$$;

SELECT gd_set(42);
SELECT gd_get();

-- The store holds arbitrary types, not just SQL scalars: here a string.
CREATE FUNCTION gd_string() RETURNS text
LANGUAGE plocamlu
AS $$
  (match PL.get_opt gd "greeting" with
   | Some _ -> ()
   | None -> PL.set gd "greeting" "hello");
  PL.String (PL.get gd "greeting")
$$;

SELECT gd_string();

-- Memoization: SD caches an expensive result; a GD counter records how many
-- times the expensive branch actually ran (should be exactly once).
CREATE FUNCTION sd_memo() RETURNS int
LANGUAGE plocamlu
AS $$
  match PL.get_opt sd "cached" with
  | Some x -> PL.Int x
  | None ->
    let calls = match PL.get_opt gd "memo_computes" with Some c -> c | None -> 0 in
    PL.set gd "memo_computes" (calls + 1);
    PL.set sd "cached" 99;
    PL.Int 99
$$;

CREATE FUNCTION gd_computes() RETURNS int
LANGUAGE plocamlu
AS $$
  match PL.get_opt gd "memo_computes" with Some n -> PL.Int n | None -> PL.Int 0
$$;

SELECT sd_memo();
SELECT sd_memo();
SELECT sd_memo();
SELECT gd_computes();

-- A prepared plan is stored directly (no datum wrapper) and reused across
-- calls. SPI_keepplan keeps it valid beyond the call that created it, and
-- holding the value in the store keeps it from being finalized. A GD counter
-- proves PL.prepare only ran once.
CREATE FUNCTION plan_cached(n int) RETURNS int
LANGUAGE plocamlu
AS $$
  let plan =
    match PL.get_opt gd "sq_plan" with
    | Some p -> p
    | None ->
      let p = PL.prepare "SELECT $1 * $1 AS r" [| "int4" |] in
      PL.set gd "sq_plan" p;
      let c = match PL.get_opt gd "sq_prepares" with Some c -> c | None -> 0 in
      PL.set gd "sq_prepares" (c + 1);
      p
  in
  let rv = PL.execute_plan plan [| args.(0) |] in
  PL.Int (PL.to_int ~default:0 (List.assoc "r" rv.rows.(0)))
$$;

CREATE FUNCTION plan_prepares() RETURNS int
LANGUAGE plocamlu
AS $$
  match PL.get_opt gd "sq_prepares" with Some n -> PL.Int n | None -> PL.Int 0
$$;

SELECT plan_cached(3);
SELECT plan_cached(4);
SELECT plan_cached(5);
SELECT plan_prepares();

DROP FUNCTION sd_counter;
DROP FUNCTION sd_a;
DROP FUNCTION sd_b;
DROP FUNCTION gd_set;
DROP FUNCTION gd_get;
DROP FUNCTION gd_string;
DROP FUNCTION sd_memo;
DROP FUNCTION gd_computes;
DROP FUNCTION plan_cached;
DROP FUNCTION plan_prepares;
