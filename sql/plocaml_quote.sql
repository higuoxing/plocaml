-- test quoting functions

CREATE FUNCTION quote(t text, how text) RETURNS text AS $$
  let t_str_opt = PL.to_string_opt t in
  let how_str = PL.to_string_exn how in
  let res = match how_str with
    | "literal" -> PL.quote_literal (Option.get t_str_opt)
    | "nullable" -> PL.quote_nullable t_str_opt
    | "ident" -> PL.quote_ident (Option.get t_str_opt)
    | _ -> failwith ("unrecognized quote type " ^ how_str)
  in
  PL.String res
$$ LANGUAGE plocamlu;

SELECT quote(t, 'literal') FROM (VALUES
       ('abc'),
       ('a''bc'),
       ('''abc'''),
       (''),
       (''''),
       ('xyzv')) AS v(t);

SELECT quote(t, 'nullable') FROM (VALUES
       ('abc'),
       ('a''bc'),
       ('''abc'''),
       (''),
       (''''),
       (NULL)) AS v(t);

SELECT quote(t, 'ident') FROM (VALUES
       ('abc'),
       ('a b c'),
       ('a " ''abc''')) AS v(t);
