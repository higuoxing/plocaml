(* Keep C primitives in bytecode symbol table so Toploop can dynamically resolve them *)
external _keep_subtransaction : (unit -> 'a) -> 'a = "plocaml_subtransaction"
external _keep_spi_execute : string -> unit = "plocaml_spi_execute"

external _keep_spi_prepare : string -> string array -> unit
  = "plocaml_spi_prepare"

external _keep_spi_execute_plan : 'a -> 'b array -> unit
  = "plocaml_spi_execute_plan"

external _keep_commit : unit -> unit = "plocaml_commit"
external _keep_rollback : unit -> unit = "plocaml_rollback"

let () =
  ignore _keep_subtransaction;
  ignore _keep_spi_execute;
  ignore _keep_spi_prepare;
  ignore _keep_spi_execute_plan;
  ignore _keep_commit;
  ignore _keep_rollback

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

let () =
  Callback.register "plocaml_init_toplevel" init_toplevel;
  Callback.register "plocaml_execute" execute_inline
