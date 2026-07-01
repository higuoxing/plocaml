open Format

type execution_result =
  | Ok of Bootstrap.PLOCaml.datum
  | SyntaxError of string
  | RuntimeError of string

let function_cache :
    (int, Bootstrap.PLOCaml.datum array -> Bootstrap.PLOCaml.datum) Hashtbl.t =
  Hashtbl.create 16

(* These C primitives are declared in Bootstrap.PLOCaml but only ever called by
   dynamically-compiled (toploop) user code, so nothing in this executable
   applies them directly. Reference them here so the bytecode linker keeps them
   in the shared object's primitive table, where the toplevel resolves them. *)
let () =
  let open Bootstrap.PLOCaml in
  ignore execute;
  ignore prepare;
  ignore execute_with_args;
  ignore execute_plan;
  ignore cursor;
  ignore cursor_plan;
  ignore fetch;
  ignore close;
  ignore _report

let init_toplevel bootstrap_code guc_stdlib_path =
  Compmisc.init_path ();
  Toploop.initialize_toplevel_env ();

  let stdlib_path =
    if guc_stdlib_path <> "" then guc_stdlib_path
    else
      match Sys.getenv_opt "PLOCAML_STDLIB_PATH" with
      | Some p when p <> "" -> p
      | _ -> Plocaml_config.stdlib_path
  in
  Topdirs.dir_directory stdlib_path;
  let lexbuf = Lexing.from_string bootstrap_code in
  (* Parse and execute all phrases in the bootstrap code sequentially
     (the PLOCaml module definition, then the PL alias). *)
  try
    let rec loop () =
      let phrase = !Toploop.parse_toplevel_phrase lexbuf in
      ignore (Toploop.execute_phrase false Format.std_formatter phrase);
      loop ()
    in
    loop ()
  with End_of_file -> ()

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

let ocaml_keywords =
  [
    "and";
    "as";
    "assert";
    "begin";
    "class";
    "constraint";
    "do";
    "done";
    "downto";
    "else";
    "end";
    "exception";
    "external";
    "false";
    "for";
    "fun";
    "function";
    "functor";
    "if";
    "in";
    "include";
    "inherit";
    "initializer";
    "land";
    "lazy";
    "let";
    "lor";
    "lsl";
    "lsr";
    "lxor";
    "match";
    "method";
    "mod";
    "module";
    "mutable";
    "new";
    "nonrec";
    "object";
    "of";
    "open";
    "or";
    "private";
    "rec";
    "sig";
    "struct";
    "then";
    "to";
    "true";
    "try";
    "type";
    "val";
    "virtual";
    "when";
    "while";
    "with";
  ]

(* A SQL argument name is exposed as a local only if it is a valid lowercase
   OCaml identifier and not a keyword; otherwise the caller uses args.(i). *)
let is_valid_ident name =
  let n = String.length name in
  n > 0
  && (match name.[0] with 'a' .. 'z' | '_' -> true | _ -> false)
  && (let ok = ref true in
      for i = 1 to n - 1 do
        match name.[i] with
        | 'a' .. 'z' | 'A' .. 'Z' | '0' .. '9' | '_' | '\'' -> ()
        | _ -> ok := false
      done;
      !ok)
  && not (List.mem name ocaml_keywords)

(* Bind each named parameter to args.(i) before the user body, mirroring
   PL/Python exposing named parameters as variables. *)
let named_param_prelude arg_names =
  let buf = Buffer.create 64 in
  Array.iteri
    (fun i name ->
      if is_valid_ident name then
        Buffer.add_string buf (Printf.sprintf "let %s = args.(%d) in " name i))
    arg_names;
  Array.iter
    (fun name ->
      if is_valid_ident name then
        Buffer.add_string buf (Printf.sprintf "ignore %s; " name))
    arg_names;
  Buffer.contents buf

let compile_function oid func_name arg_names code_str buf fmt =
  let code_str = normalize_newlines code_str in
  let params = named_param_prelude arg_names in
  let wrapper =
    if oid = 0 then
      (* For DO blocks (oid = 0), we don't expect a return value, just unit.
         We wrap it to return PL.Null so the rest of the engine works normally.
         All DO blocks share the OID-0 SD table. *)
      Printf.sprintf
        "let _plocaml_fn_%d (args : PL.datum array) : PL.datum =\n\
         let gd = PL.gd in let sd = PL.get_sd %d in ignore gd; ignore sd; %s\n\
         # 1 \"[PL/OCaml function %s]\"\n\
         %s;\n\
         PL.Null;;"
        oid oid params func_name code_str
    else
      Printf.sprintf
        "let _plocaml_fn_%d (args : PL.datum array) : PL.datum =\n\
         let gd = PL.gd in let sd = PL.get_sd %d in ignore gd; ignore sd; %s\n\
         # 1 \"[PL/OCaml function %s]\"\n\
         %s;;"
        oid oid params func_name code_str
  in
  let lexbuf = Lexing.from_string wrapper in
  try
    let phrase = !Toploop.parse_toplevel_phrase lexbuf in
    if Toploop.execute_phrase true fmt phrase then (
      let val_name = Printf.sprintf "_plocaml_fn_%d" oid in
      let v = Toploop.getvalue val_name in
      let f : Bootstrap.PLOCaml.datum array -> Bootstrap.PLOCaml.datum =
        Obj.magic v
      in
      if oid <> 0 then Hashtbl.add function_cache oid f;
      Stdlib.Result.Ok f)
    else Stdlib.Result.Error (RuntimeError (Buffer.contents buf))
  with
  | (Syntaxerr.Error _ | Lexer.Error _) as e ->
      (try Location.report_exception fmt e with _ -> ());
      Format.pp_print_flush fmt ();
      Stdlib.Result.Error (SyntaxError (Buffer.contents buf))
  | e ->
      (try Location.report_exception fmt e with _ -> ());
      Format.pp_print_flush fmt ();
      let msg = Buffer.contents buf in
      let msg = if msg = "" then Printexc.to_string e else msg in
      Stdlib.Result.Error (RuntimeError msg)

let execute_pl_code oid func_name code_str arg_names args =
  let buf = Buffer.create 128 in
  let fmt = formatter_of_buffer buf in
  let func_opt =
    if oid == 0 then compile_function oid func_name arg_names code_str buf fmt
    else
      match Hashtbl.find_opt function_cache oid with
      | Some f -> Stdlib.Result.Ok f
      | None -> compile_function oid func_name arg_names code_str buf fmt
  in
  match func_opt with
  | Stdlib.Result.Error e -> e
  | Stdlib.Result.Ok f -> (
      try Ok (f args)
      with e ->
        RuntimeError ("Exception during execution: " ^ Printexc.to_string e))

external postgres_magic_keepalive : unit -> unit = "plocaml_magic_keepalive"

let () =
  postgres_magic_keepalive ();
  Callback.register "plocaml_init_toplevel" init_toplevel;
  Callback.register "plocaml_execute" execute_pl_code
