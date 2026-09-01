(* Keep C primitives in bytecode symbol table so Toploop can dynamically resolve them *)
external _keep_subtransaction : (unit -> 'a) -> 'a = "plocaml_subtransaction"
external _keep_spi_execute : string -> unit = "plocaml_spi_execute"

external _keep_spi_prepare : string -> string array -> unit
  = "plocaml_spi_prepare"

external _keep_spi_execute_plan : 'a -> 'b array -> unit
  = "plocaml_spi_execute_plan"

external _keep_commit : unit -> unit = "plocaml_commit"
external _keep_rollback : unit -> unit = "plocaml_rollback"
external _keep_elog : 'a -> 'b -> unit = "plocaml_elog"
external _keep_quote_literal : string -> string = "plocaml_quote_literal"
external _keep_quote_ident : string -> string = "plocaml_quote_ident"
external _keep_cursor_open : string -> 'a = "plocaml_spi_cursor_open"

external _keep_cursor_open_plan : 'a -> 'b array -> 'c
  = "plocaml_spi_cursor_open_plan"

external _keep_cursor_fetch : 'a -> int -> 'b = "plocaml_spi_cursor_fetch"
external _keep_cursor_close : 'a -> unit = "plocaml_spi_cursor_close"

let () =
  ignore _keep_subtransaction;
  ignore _keep_spi_execute;
  ignore _keep_spi_prepare;
  ignore _keep_spi_execute_plan;
  ignore _keep_commit;
  ignore _keep_rollback;
  ignore _keep_elog;
  ignore _keep_quote_literal;
  ignore _keep_quote_ident;
  ignore _keep_cursor_open;
  ignore _keep_cursor_open_plan;
  ignore _keep_cursor_fetch;
  ignore _keep_cursor_close

let toplevel_initialized = ref false

let execute_phrases (source : string) : unit =
  let buf = Buffer.create 128 in
  let fmt = Format.formatter_of_buffer buf in
  let lexbuf = Lexing.from_string (source ^ "\n;;") in
  let phrases = ref [] in
  (try
     let rec loop () =
       match !Toploop.parse_toplevel_phrase lexbuf with
       | p ->
           phrases := p :: !phrases;
           loop ()
       | exception End_of_file -> ()
     in
     loop ()
   with e ->
     (try Location.report_exception fmt e with _ -> ());
     Format.pp_print_flush fmt ();
     let msg = Buffer.contents buf in
     let msg = if msg = "" then Printexc.to_string e else msg in
     failwith msg);
  List.iter
    (fun p ->
      Buffer.clear buf;
      if not (Toploop.execute_phrase false fmt p) then (
        Format.pp_print_flush fmt ();
        let msg = String.trim (Buffer.contents buf) in
        failwith (if msg = "" then "Execution failed" else msg)))
    (List.rev !phrases)

let init_toplevel (bootstrap_code : string) =
  if not !toplevel_initialized then (
    Toploop.initialize_toplevel_env ();
    execute_phrases bootstrap_code;
    toplevel_initialized := true)

let execute_inline (source_text : string) : unit = execute_phrases source_text

let is_ocaml_keyword = function
  | "and" | "as" | "assert" | "asr" | "begin" | "class" | "constraint" | "do"
  | "done" | "downto" | "else" | "end" | "exception" | "external" | "false"
  | "for" | "fun" | "function" | "functor" | "if" | "in" | "include" | "inherit"
  | "initializer" | "land" | "lazy" | "let" | "lor" | "lsl" | "lsr" | "lxor"
  | "match" | "method" | "mod" | "module" | "mutable" | "new" | "nonrec"
  | "object" | "of" | "open" | "or" | "private" | "rec" | "sig" | "struct"
  | "then" | "to" | "true" | "try" | "type" | "val" | "virtual" | "when"
  | "while" | "with" ->
      true
  | _ -> false

let is_valid_ident s =
  let len = String.length s in
  if len = 0 then false
  else
    let first = s.[0] in
    let valid_first = (first >= 'a' && first <= 'z') || first = '_' in
    if not valid_first then false
    else if is_ocaml_keyword s then false
    else
      let rec check i =
        if i >= len then true
        else
          let c = s.[i] in
          let valid_char =
            (c >= 'a' && c <= 'z')
            || (c >= 'A' && c <= 'Z')
            || (c >= '0' && c <= '9')
            || c = '_' || c = '\''
          in
          if valid_char then check (i + 1) else false
      in
      check 1

type compiled_entry = { src_code : string; fn : Obj.t array -> Obj.t }

let compiled_functions : (int, compiled_entry) Hashtbl.t = Hashtbl.create 32

let compile_function (fn_oid : int) (prosrc : string) (arg_names : string array)
    : Obj.t array -> Obj.t =
  let var_name = Printf.sprintf "__plocaml_fn_%d" fn_oid in
  let nargs = Array.length arg_names in
  let buf = Buffer.create (String.length prosrc + 256) in
  Buffer.add_string buf
    (Printf.sprintf "let %s (args : Plocaml.datum array) =\n" var_name);
  for i = 0 to nargs - 1 do
    let arg_idx = string_of_int i in
    Buffer.add_string buf
      (Printf.sprintf "  let arg%d = args.(%s) in\n" (i + 1) arg_idx);
    let name = arg_names.(i) in
    if is_valid_ident name && name <> "arg" ^ string_of_int (i + 1) then
      Buffer.add_string buf
        (Printf.sprintf "  let %s = args.(%s) in\n" name arg_idx)
  done;
  Buffer.add_string buf
    (Printf.sprintf "  let sd = Plocaml.get_sd %d in\n" fn_oid);
  Buffer.add_string buf "  Obj.repr (begin\n";
  Buffer.add_string buf prosrc;
  Buffer.add_string buf "\n  end)\n;;\n";
  execute_phrases (Buffer.contents buf);
  let (fn : Obj.t array -> Obj.t) = Obj.obj (Toploop.getvalue var_name) in
  Hashtbl.replace compiled_functions fn_oid { src_code = prosrc; fn };
  fn

let invoke_function (fn_oid : int) (prosrc : string) (arg_names : string array)
    (args : Obj.t array) : Obj.t =
  let fn =
    match Hashtbl.find_opt compiled_functions fn_oid with
    | Some entry when String.equal entry.src_code prosrc -> entry.fn
    | _ -> compile_function fn_oid prosrc arg_names
  in
  fn args

let () =
  Callback.register "plocaml_init_toplevel" init_toplevel;
  Callback.register "plocaml_execute" execute_inline;
  Callback.register "plocaml_invoke_function" invoke_function
