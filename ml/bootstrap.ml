module Plocaml = struct
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

  type spi_result = {
    status : int;
    nrows : int;
    rows : (string * datum) list array;
  }

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

  (* SPI Operations *)
  module SPI = struct
    external execute : string -> spi_result = "plocaml_spi_execute"
    external prepare : string -> string array -> plan = "plocaml_spi_prepare"

    external execute_plan : plan -> datum array -> spi_result
      = "plocaml_spi_execute_plan"

    external cursor : string -> cursor = "plocaml_spi_cursor_open"

    external cursor_plan : plan -> datum array -> cursor
      = "plocaml_spi_cursor_open_plan"

    external fetch : cursor -> int -> spi_result = "plocaml_spi_cursor_fetch"
    external close : cursor -> unit = "plocaml_spi_cursor_close"
  end

  (* Logging and Error Reporting *)
  module Log = struct
    external elog_record : log_level -> error_info -> unit = "plocaml_elog"

    let report (level : log_level) ?detail ?hint ?sqlstate ?schema_name
        ?table_name ?column_name ?datatype_name ?constraint_name
        (message : string) : unit =
      elog_record level
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

    let debug ?detail ?hint ?sqlstate ?schema_name ?table_name ?column_name
        ?datatype_name ?constraint_name message =
      report Debug1 ?detail ?hint ?sqlstate ?schema_name ?table_name
        ?column_name ?datatype_name ?constraint_name message

    let log ?detail ?hint ?sqlstate ?schema_name ?table_name ?column_name
        ?datatype_name ?constraint_name message =
      report Log ?detail ?hint ?sqlstate ?schema_name ?table_name ?column_name
        ?datatype_name ?constraint_name message

    let info ?detail ?hint ?sqlstate ?schema_name ?table_name ?column_name
        ?datatype_name ?constraint_name message =
      report Info ?detail ?hint ?sqlstate ?schema_name ?table_name ?column_name
        ?datatype_name ?constraint_name message

    let notice ?detail ?hint ?sqlstate ?schema_name ?table_name ?column_name
        ?datatype_name ?constraint_name message =
      report Notice ?detail ?hint ?sqlstate ?schema_name ?table_name
        ?column_name ?datatype_name ?constraint_name message

    let warning ?detail ?hint ?sqlstate ?schema_name ?table_name ?column_name
        ?datatype_name ?constraint_name message =
      report Warning ?detail ?hint ?sqlstate ?schema_name ?table_name
        ?column_name ?datatype_name ?constraint_name message

    let error ?detail ?hint ?sqlstate ?schema_name ?table_name ?column_name
        ?datatype_name ?constraint_name message =
      report Error ?detail ?hint ?sqlstate ?schema_name ?table_name ?column_name
        ?datatype_name ?constraint_name message

    let elog (level : log_level) (message : string) : unit =
      report level message
  end

  (* String Quoting *)
  module Quote = struct
    external literal : string -> string = "plocaml_quote_literal"
    external ident : string -> string = "plocaml_quote_ident"

    let nullable (s : string option) : string =
      match s with None -> "NULL" | Some s -> literal s
  end

  (* Transaction & Subtransaction Control *)
  external subtransaction : (unit -> 'a) -> 'a = "plocaml_subtransaction"
  external commit : unit -> unit = "plocaml_commit"
  external rollback : unit -> unit = "plocaml_rollback"

  (* Session / Function Storage *)
  type store = (string, Obj.t) Hashtbl.t

  let gd : store = Hashtbl.create 16
  let get_sd (_oid : int) : store = Hashtbl.create 16

  (* Direct convenience shortcuts on Plocaml / PL *)
  let execute = SPI.execute
  let prepare = SPI.prepare
  let execute_plan = SPI.execute_plan
  let cursor = SPI.cursor
  let cursor_plan = SPI.cursor_plan
  let fetch = SPI.fetch
  let close = SPI.close
  let debug = Log.debug
  let log = Log.log
  let info = Log.info
  let notice = Log.notice
  let warning = Log.warning
  let error = Log.error
  let elog = Log.elog
  let quote_literal = Quote.literal
  let quote_nullable = Quote.nullable
  let quote_ident = Quote.ident
end

module PL = Plocaml
