module PLOCaml = struct
  type datum =
    | Null
    | Int of int
    | Float of float
    | String of string
    | Bool of bool
    | Array of datum array

  external execute : string -> int = "plocaml_spi_execute"
  external notice : string -> unit = "plocaml_notice"
end;;

module PL = PLOCaml;;
