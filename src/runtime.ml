open Format

type execution_result =
  | Ok of Bootstrap.PLOCaml.datum
  | SyntaxError of string
  | RuntimeError of string

let function_cache : (int, Bootstrap.PLOCaml.datum array -> Bootstrap.PLOCaml.datum) Hashtbl.t = Hashtbl.create 16

(* These C primitives are declared in Bootstrap.PLOCaml but only ever called by
   dynamically-compiled (toploop) user code, so nothing in this executable
   applies them directly. Reference them here so the bytecode linker keeps them
   in the shared object's primitive table, where the toplevel resolves them. *)
let () =
  let open Bootstrap.PLOCaml in
  ignore execute; ignore prepare; ignore execute_with_args; ignore execute_plan;
  ignore cursor; ignore cursor_plan; ignore fetch; ignore close; ignore _report

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

(* Universal newline support: OCaml's lexer accepts LF and CRLF but rejects a
   bare CR, so normalize CR and CRLF in the user source to LF before compiling.
   Only literal CR bytes are affected, not "\r" escape sequences. *)
let normalize_newlines s =
  let b = Buffer.create (String.length s) in
  let n = String.length s in
  let i = ref 0 in
  while !i < n do
    (match s.[!i] with
     | '\r' ->
         Buffer.add_char b '\n';
         if !i + 1 < n && s.[!i + 1] = '\n' then incr i
     | c -> Buffer.add_char b c);
    incr i
  done;
  Buffer.contents b

let compile_function oid func_name code_str buf fmt =
  let code_str = normalize_newlines code_str in
  let wrapper =
    if oid = 0 then
      (* For DO blocks (oid = 0), we don't expect a return value, just unit.
         We wrap it to return PL.Null so the rest of the engine works normally.
         All DO blocks share the OID-0 SD table. *)
      Printf.sprintf "let _plocaml_fn_%d (args : PL.datum array) : PL.datum =\nlet gd = PL.gd in let sd = PL.get_sd %d in ignore gd; ignore sd;\n# 1 \"[PL/OCaml function %s]\"\n%s;\nPL.Null;;" oid oid func_name code_str
    else
      Printf.sprintf "let _plocaml_fn_%d (args : PL.datum array) : PL.datum =\nlet gd = PL.gd in let sd = PL.get_sd %d in ignore gd; ignore sd;\n# 1 \"[PL/OCaml function %s]\"\n%s;;" oid oid func_name code_str
  in
  let lexbuf = Lexing.from_string wrapper in
  try
    let phrase = !Toploop.parse_toplevel_phrase lexbuf in
    if Toploop.execute_phrase true fmt phrase then
      let val_name = Printf.sprintf "_plocaml_fn_%d" oid in
      let v = Toploop.getvalue val_name in
      let f : Bootstrap.PLOCaml.datum array -> Bootstrap.PLOCaml.datum = Obj.magic v in
      if oid <> 0 then Hashtbl.add function_cache oid f;
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
    if oid == 0 then
      compile_function oid func_name code_str buf fmt
    else
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
