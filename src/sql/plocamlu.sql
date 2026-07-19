CREATE FUNCTION plocaml_call_handler() RETURNS language_handler
  LANGUAGE C AS 'MODULE_PATHNAME';

CREATE FUNCTION plocaml_inline_handler(internal) RETURNS void
  STRICT LANGUAGE C AS 'MODULE_PATHNAME';


CREATE LANGUAGE plocamlu
  HANDLER plocaml_call_handler
  INLINE plocaml_inline_handler
  VALIDATOR plocaml_validator;

COMMENT ON LANGUAGE plocamlu IS 'PL/OCamlU untrusted procedural language';
