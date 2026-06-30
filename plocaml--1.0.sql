-- complain if script is sourced in psql, rather than via CREATE EXTENSION
\echo Use "CREATE EXTENSION plocaml" to load this file. \quit

CREATE OR REPLACE FUNCTION plocaml_call_handler() RETURNS language_handler 
  AS 'plocaml' LANGUAGE c;

CREATE LANGUAGE plocaml HANDLER plocaml_call_handler;
