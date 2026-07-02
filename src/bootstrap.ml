module PLOCaml = struct
  (* Abstract handles backed by C custom blocks (see spi.c). *)
  type plan
  type cursor

  type datum =
    | Null
    | Int of int
    | Float of float
    | String of string
    | Bool of bool
    | Array of datum array
    | Record of (string * datum) list

  let to_int_exn = function
    | Int x -> x
    | _ -> failwith "PL/OCaml: Expected Int"

  let to_float_exn = function
    | Float x -> x
    | _ -> failwith "PL/OCaml: Expected Float"

  let to_string_exn = function
    | String x -> x
    | _ -> failwith "PL/OCaml: Expected String"

  let to_bool_exn = function
    | Bool x -> x
    | _ -> failwith "PL/OCaml: Expected Bool"

  let to_array_exn = function
    | Array x -> x
    | _ -> failwith "PL/OCaml: Expected Array"

  let to_record_exn = function
    | Record x -> x
    | _ -> failwith "PL/OCaml: Expected Record"

  let to_int_opt = function Int x -> Some x | _ -> None
  let to_float_opt = function Float x -> Some x | _ -> None
  let to_string_opt = function String x -> Some x | _ -> None
  let to_bool_opt = function Bool x -> Some x | _ -> None
  let to_array_opt = function Array x -> Some x | _ -> None
  let to_record_opt = function Record x -> Some x | _ -> None

  (* Look up a field of a composite (Record) datum by column name. *)
  let field name = function
    | Record fields -> List.assoc name fields
    | _ -> failwith "PL/OCaml: Expected Record"

  let to_int ~default = function Int x -> x | _ -> default
  let to_float ~default = function Float x -> x | _ -> default
  let to_string ~default = function String x -> x | _ -> default
  let to_bool ~default = function Bool x -> x | _ -> default
  let to_array ~default = function Array x -> x | _ -> default

  type spi_result = {
    status : int;
    nrows : int;
    rows : (string * datum) list array;
  }

  (* A [store] holds values of ANY type, mirroring the free-form nature of
     PL/Python's GD/SD dictionaries. Because OCaml is statically typed this is
     achieved with an unchecked cast ([Obj.repr]/[Obj.obj]): the type you read
     a key back at MUST match the type it was written with, otherwise behaviour
     is undefined. Always use [set]/[get]/[get_opt] rather than touching the
     underlying [Obj.t] directly. Structural operations that don't inspect the
     value (Hashtbl.mem, .remove, .clear, .length, ...) can be used as-is. *)
  type store = (string, Obj.t) Hashtbl.t

  let set (t : store) (key : string) (v : 'a) : unit =
    Hashtbl.replace t key (Obj.repr v)

  let get_opt (t : store) (key : string) : 'a option =
    match Hashtbl.find_opt t key with
    | Some v -> Some (Obj.obj v)
    | None -> None

  let get (t : store) (key : string) : 'a =
    match Hashtbl.find_opt t key with
    | Some v -> Obj.obj v
    | None ->
        failwith (Printf.sprintf "PL/OCaml: no GD/SD entry for key %S" key)

  (* GD: one global store shared by all functions in the session. Its lifetime
     is the backend session, mirroring PL/Python's GD. *)
  let gd : store = Hashtbl.create 16

  (* SD: one store per function OID, persisting across calls to the same
     function within the session, mirroring PL/Python's SD. The OID is baked
     into each compiled function (see runtime.ml), so [get_sd] returns the
     store belonging to the currently executing function. *)
  let _sd_registry : (int, store) Hashtbl.t = Hashtbl.create 16

  let get_sd (oid : int) : store =
    match Hashtbl.find_opt _sd_registry oid with
    | Some t -> t
    | None ->
        let t = Hashtbl.create 16 in
        Hashtbl.add _sd_registry oid t;
        t

  type log_level =
    | Debug5
    | Debug4
    | Debug3
    | Debug2
    | Debug1
    | Log
    | Info
    | Notice
    | Warning
    | Error

  let log_level_to_int = function
    | Debug5 -> 10
    | Debug4 -> 11
    | Debug3 -> 12
    | Debug2 -> 13
    | Debug1 -> 14
    | Log -> 15
    | Info -> 17
    | Notice -> 18
    | Warning -> 19
    | Error -> 21

  external execute : string -> spi_result = "plocaml_spi_execute"
  external prepare : string -> string array -> plan = "plocaml_spi_prepare"
  external quote_literal : string -> string = "plocaml_quote_literal"
  external quote_nullable : string option -> string = "plocaml_quote_nullable"
  external quote_ident : string -> string = "plocaml_quote_ident"

  external execute_with_args : string -> datum array -> spi_result
    = "plocaml_spi_execute_with_args"

  external execute_plan : plan -> datum array -> spi_result
    = "plocaml_spi_execute_plan"

  external cursor : string -> cursor = "plocaml_spi_cursor"

  external cursor_plan : plan -> datum array -> cursor
    = "plocaml_spi_cursor_plan"

  external fetch : cursor -> int -> spi_result = "plocaml_spi_fetch"
  external close : cursor -> unit = "plocaml_spi_close"

  (* Structured error/notice report, mirroring PL/Python's plpy.error and
     friends. The optional fields map to the PostgreSQL error fields. For a
     level of [Error] (or higher) the report raises; lower levels emit a
     message and return. *)
  type error_info = {
    e_message : string;
    e_detail : string option;
    e_hint : string option;
    e_sqlstate : string option;
    e_schema_name : string option;
    e_table_name : string option;
    e_column_name : string option;
    e_datatype_name : string option;
    e_constraint_name : string option;
  }

  external _report : int -> error_info -> unit = "plocaml_report"

  (* [level] comes first so the plpy-style helpers below are just partial
     applications: fixing the level stops before the optional fields, leaving
     them intact for the caller. *)
  let report level ?detail ?hint ?sqlstate ?schema_name ?table_name ?column_name
      ?datatype_name ?constraint_name message =
    _report (log_level_to_int level)
      {
        e_message = message;
        e_detail = detail;
        e_hint = hint;
        e_sqlstate = sqlstate;
        e_schema_name = schema_name;
        e_table_name = table_name;
        e_column_name = column_name;
        e_datatype_name = datatype_name;
        e_constraint_name = constraint_name;
      }

  let debug = report Debug1
  let log = report Log
  let info = report Info
  let notice = report Notice
  let warning = report Warning
  let error = report Error

  (* Backward-compatible plain message log. *)
  let elog level message = report level message
end

module PL = PLOCaml
