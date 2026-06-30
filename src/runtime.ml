open Format

type execution_result =
  | Ok of Bootstrap.PLOCaml.datum
  | SyntaxError of string
  | RuntimeError of string

let function_cache : (int, Bootstrap.PLOCaml.datum array -> Bootstrap.PLOCaml.datum) Hashtbl.t = Hashtbl.create 16

external plocaml_spi_execute : string -> int = "plocaml_spi_execute"
let () = ignore plocaml_spi_execute

external plocaml_elog : int -> string -> unit = "plocaml_elog"
let () = ignore plocaml_elog

let init_toplevel bootstrap_code guc_stdlib_path =
  Compmisc.init_path ();
  Toploop.initialize_toplevel_env ();

  let stdlib_path =
    if guc_stdlib_path <> "" then guc_stdlib_path
    else match Sys.getenv_opt "PLOCAML_STDLIB_PATH" with
    | Some p when p <> "" -> p
    | _ -> Plocaml_config.stdlib_path
  in
  Topdirs.dir_directory stdlib_path;
  let lexbuf = Lexing.from_string bootstrap_code in
  (* Parse and execute all phrases in the bootstrap code sequentially.
     This is required because bootstrap.ml contains multiple top-level
     statements (e.g., defining PLOCaml, and then alias PL = PLOCaml). *)
  try
    let rec loop () =
      let phrase = !Toploop.parse_toplevel_phrase lexbuf in
      ignore (Toploop.execute_phrase false Format.std_formatter phrase);
      loop ()
    in
    loop ()
  with
  | End_of_file -> ()

let compile_function oid func_name code_str buf fmt =
  let wrapper = Printf.sprintf "let _plocaml_fn_%d (args : PL.datum array) : PL.datum =\n# 1 \"[PL/OCaml function %s]\"\n%s;;" oid func_name code_str in
  let lexbuf = Lexing.from_string wrapper in
  try
    let phrase = !Toploop.parse_toplevel_phrase lexbuf in
    if Toploop.execute_phrase true fmt phrase then
      let val_name = Printf.sprintf "_plocaml_fn_%d" oid in
      let v = Toploop.getvalue val_name in
      let f : Bootstrap.PLOCaml.datum array -> Bootstrap.PLOCaml.datum = Obj.magic v in
      Hashtbl.add function_cache oid f;
      Stdlib.Result.Ok f
    else
      Stdlib.Result.Error (RuntimeError (Buffer.contents buf))
  with
  | Syntaxerr.Error _ | Lexer.Error _ as e ->
      (try Location.report_exception fmt e with _ -> ());
      Format.pp_print_flush fmt ();
      Stdlib.Result.Error (SyntaxError (Buffer.contents buf))
  | e ->
      (try Location.report_exception fmt e with _ -> ());
      Format.pp_print_flush fmt ();
      let msg = Buffer.contents buf in
      let msg = if msg = "" then Printexc.to_string e else msg in
      Stdlib.Result.Error (RuntimeError msg)

let execute_pl_code oid func_name code_str args =
  let buf = Buffer.create 128 in
  let fmt = formatter_of_buffer buf in
  let func_opt =
    match Hashtbl.find_opt function_cache oid with
    | Some f -> Stdlib.Result.Ok f
    | None -> compile_function oid func_name code_str buf fmt
  in
  match func_opt with
  | Stdlib.Result.Error e -> e
  | Stdlib.Result.Ok f ->
      try
        Ok (f args)
      with e ->
        RuntimeError ("Exception during execution: " ^ Printexc.to_string e)

external postgres_magic_keepalive : unit -> unit = "plocaml_magic_keepalive"

let () =
  postgres_magic_keepalive ();
  Callback.register "plocaml_init_toplevel" init_toplevel;
  Callback.register "plocaml_execute" execute_pl_code
