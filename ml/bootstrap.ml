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

    let cursor (_query : string) : cursor = failwith "not implemented"

    let cursor_plan (_p : plan) (_args : datum array) : cursor =
      failwith "not implemented"

    let fetch (_c : cursor) (_count : int) : spi_result =
      failwith "not implemented"

    let close (_c : cursor) : unit = failwith "not implemented"
  end

  (* Logging and Error Reporting *)
  module Log = struct
    let report (_level : log_level) ?detail ?hint ?sqlstate ?schema_name
        ?table_name ?column_name ?datatype_name ?constraint_name
        (_message : string) : unit =
      ignore detail;
      ignore hint;
      ignore sqlstate;
      ignore schema_name;
      ignore table_name;
      ignore column_name;
      ignore datatype_name;
      ignore constraint_name;
      failwith "not implemented"

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
    let literal (_s : string) : string = failwith "not implemented"
    let nullable (_s : string option) : string = failwith "not implemented"
    let ident (_s : string) : string = failwith "not implemented"
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
