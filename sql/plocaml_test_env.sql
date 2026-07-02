CREATE FUNCTION test_env_lookup() RETURNS SETOF text AS $$
  let env = !Toploop.toplevel_env in
  let (path, md) = Env.lookup_module ~loc:Location.none (Longident.Lident "PL") env in
  let names = ref [] in
  let rec get_names mty =
    match mty with
    | Types.Mty_signature s ->
        List.iter (fun item ->
          match item with
          | Types.Sig_value (id, _, _) -> names := Ident.name id :: !names
          | Types.Sig_type (id, _, _, _) -> names := Ident.name id :: !names
          | Types.Sig_module (id, _, _, _, _) -> names := Ident.name id :: !names
          | _ -> ()
        ) s
    | _ -> ()
  in
  get_names md.Types.md_type;
  let sorted = List.sort String.compare !names in
  PL.Array (Array.of_list (List.map (fun s -> PL.String s) sorted))
$$ LANGUAGE plocamlu;

SELECT * FROM test_env_lookup();
