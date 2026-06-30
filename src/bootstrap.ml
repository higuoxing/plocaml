module PLOCaml = struct
  type datum =
    | Null
    | Int of int
    | Float of float
    | String of string
    | Bool of bool
    | Array of datum array

  let to_int_exn = function Int x -> x | _ -> failwith "PL/OCaml: Expected Int"
  let to_float_exn = function Float x -> x | _ -> failwith "PL/OCaml: Expected Float"
  let to_string_exn = function String x -> x | _ -> failwith "PL/OCaml: Expected String"
  let to_bool_exn = function Bool x -> x | _ -> failwith "PL/OCaml: Expected Bool"
  let to_array_exn = function Array x -> x | _ -> failwith "PL/OCaml: Expected Array"

  let to_int_opt = function Int x -> Some x | _ -> None
  let to_float_opt = function Float x -> Some x | _ -> None
  let to_string_opt = function String x -> Some x | _ -> None
  let to_bool_opt = function Bool x -> Some x | _ -> None
  let to_array_opt = function Array x -> Some x | _ -> None

  let to_int ~default = function Int x -> x | _ -> default
  let to_float ~default = function Float x -> x | _ -> default
  let to_string ~default = function String x -> x | _ -> default
  let to_bool ~default = function Bool x -> x | _ -> default
  let to_array ~default = function Array x -> x | _ -> default

  external execute : string -> int = "plocaml_spi_execute"
  external notice : string -> unit = "plocaml_notice"
end;;

module PL = PLOCaml;;
