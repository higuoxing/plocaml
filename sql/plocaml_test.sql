-- first some tests of basic functionality

-- really stupid function just to get the module loaded
CREATE FUNCTION stupid() RETURNS text AS $$
  PL.String "zarkon"
$$ LANGUAGE plocamlu;

select stupid();

-- check versioning / another simple function
CREATE FUNCTION stupidn() RETURNS text AS $$
  PL.String "zarkon"
$$ LANGUAGE plocamlu;

select stupidn();

-- test multiple arguments and odd characters in function name
CREATE FUNCTION "Argument test #1"(u users, a1 text, a2 text) RETURNS text
	AS $$
  let u = PL.to_record_exn args.(0) in
  let a1 = PL.to_string_exn args.(1) in
  let a2 = PL.to_string_exn args.(2) in
  let sorted_keys = List.sort (fun (k1, _) (k2, _) -> String.compare k1 k2) u in
  let format_val v = match v with
    | PL.String s -> s
    | PL.Int i -> string_of_int i
    | PL.Null -> "None"
    | _ -> "unknown"
  in
  let formatted = List.map (fun (k, v) -> Printf.sprintf "%s: %s" k (format_val v)) sorted_keys in
  let out = String.concat ", " formatted in
  PL.String (Printf.sprintf "%s %s => {%s}" a1 a2 out)
$$ LANGUAGE plocamlu;

select "Argument test #1"(users, fname, lname) from users where lname = 'doe' order by 1;

-- check module contents
CREATE FUNCTION module_contents() RETURNS SETOF text AS
$$
  let contents = [
    "Array"; "Bool"; "Debug1"; "Debug2"; "Debug3"; "Debug4"; "Debug5";
    "Error"; "Float"; "Info"; "Int"; "Log"; "Notice"; "Null"; "Record";
    "String"; "Warning"; "close"; "cursor"; "cursor_plan"; "debug"; "elog";
    "error"; "execute"; "execute_plan"; "execute_with_args"; "fetch"; "field";
    "gd"; "get"; "get_opt"; "get_sd"; "info"; "log"; "log_level_to_int";
    "notice"; "prepare"; "quote_ident"; "quote_literal"; "quote_nullable";
    "report"; "set"; "to_array"; "to_array_exn"; "to_array_opt"; "to_bool";
    "to_bool_exn"; "to_bool_opt"; "to_float"; "to_float_exn"; "to_float_opt";
    "to_int"; "to_int_exn"; "to_int_opt"; "to_record_exn"; "to_record_opt";
    "to_string"; "to_string_exn"; "to_string_opt"; "warning"
  ] in
  let arr = Array.of_list (List.map (fun s -> PL.Array [| PL.String s |]) contents) in
  PL.Array arr
$$ LANGUAGE plocamlu;

select * from module_contents();

CREATE FUNCTION elog_test_basic() RETURNS void
AS $$
  PL.debug "debug";
  PL.log "log";
  PL.info "info";
  PL.info "37";
  PL.info "";
  PL.info "info 37 [1, 2, 3]";
  PL.notice "notice";
  PL.warning "warning";
  PL.error "error";
  PL.Null
$$ LANGUAGE plocamlu;

SELECT elog_test_basic();
