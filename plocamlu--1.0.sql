-- complain if script is sourced in psql, rather than via CREATE EXTENSION
\echo Use "CREATE EXTENSION plocamlu" to load this file. \quit

CREATE OR REPLACE FUNCTION plocamlu_call_handler() RETURNS language_handler 
  AS 'plocamlu' LANGUAGE c;

CREATE OR REPLACE FUNCTION plocamlu_inline_handler(internal) RETURNS void
  AS 'plocamlu' LANGUAGE c;

CREATE LANGUAGE plocamlu HANDLER plocamlu_call_handler INLINE plocamlu_inline_handler;
