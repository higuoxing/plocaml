--
-- Universal Newline Support
--

CREATE OR REPLACE FUNCTION newline_lf() RETURNS integer AS
E'let x = 100 in\nlet y = 23 in\nPL.Int (x + y)\n'
LANGUAGE plocamlu;

CREATE OR REPLACE FUNCTION newline_cr() RETURNS integer AS
E'let x = 100 in\rlet y = 23 in\rPL.Int (x + y)\r'
LANGUAGE plocamlu;

CREATE OR REPLACE FUNCTION newline_crlf() RETURNS integer AS
E'let x = 100 in\r\nlet y = 23 in\r\nPL.Int (x + y)\r\n'
LANGUAGE plocamlu;


SELECT newline_lf();
SELECT newline_cr();
SELECT newline_crlf();

DROP FUNCTION newline_lf;
DROP FUNCTION newline_cr;
DROP FUNCTION newline_crlf;
