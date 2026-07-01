CREATE EXTENSION IF NOT EXISTS plocamlu;

--
-- Mirror of PL/Python's plpython_ereport test. PL/OCaml exposes the error
-- fields (detail, hint, sqlstate, schema_name, ...) as optional labeled
-- arguments on PL.debug/log/info/notice/warning/error (and PL.report). For a
-- level of Error the call raises, propagating every field to the final error.
--
-- The Python-specific parts of the original test have no PL/OCaml equivalent
-- and are omitted: keyword-argument validation (e.g. an unknown 'blabla'
-- kwarg, or a message given both positionally and by name), non-string detail
-- objects (tuples/dates/lists), and exception-object attributes (e.spidata,
-- e.sqlstate).
--

CREATE FUNCTION elog_test() RETURNS void
LANGUAGE plocamlu
AS $$
  PL.debug   ~detail:"some detail" "debug";
  PL.log     ~detail:"some detail" "log";
  PL.info    ~detail:"some detail" "info";
  PL.info "the question" ~detail:"42";
  PL.info "This is message text."
    ~detail:"This is detail text"
    ~hint:"This is hint text."
    ~sqlstate:"XX000"
    ~schema_name:"any info about schema"
    ~table_name:"any info about table"
    ~column_name:"any info about column"
    ~datatype_name:"any info about datatype"
    ~constraint_name:"any info about constraint";
  PL.notice  ~detail:"some detail" "notice";
  PL.warning ~detail:"some detail" "warning";
  PL.error   ~detail:"some detail" ~hint:"some hint" "stop on error";
  PL.Null
$$;

SELECT elog_test();

DO $$ PL.info ~detail:"detail from a DO block" "message from a DO block" $$ LANGUAGE plocamlu;

-- should fail: invalid SQLSTATE code
DO $$ PL.info ~sqlstate:"54444A" "wrong sqlstate" $$ LANGUAGE plocamlu;

-- raise error in PL/OCaml, handle it in plpgsql
CREATE OR REPLACE FUNCTION raise_exception(_message text, _detail text DEFAULT NULL, _hint text DEFAULT NULL,
                                           _sqlstate text DEFAULT NULL,
                                           _schema_name text DEFAULT NULL,
                                           _table_name text DEFAULT NULL,
                                           _column_name text DEFAULT NULL,
                                           _datatype_name text DEFAULT NULL,
                                           _constraint_name text DEFAULT NULL)
RETURNS void
LANGUAGE plocamlu
AS $$
  let opt i = PL.to_string_opt args.(i) in
  PL.error (PL.to_string ~default:"" args.(0))
    ?detail:(opt 1) ?hint:(opt 2) ?sqlstate:(opt 3) ?schema_name:(opt 4)
    ?table_name:(opt 5) ?column_name:(opt 6) ?datatype_name:(opt 7)
    ?constraint_name:(opt 8);
  PL.Null
$$;

SELECT raise_exception('hello', 'world');
SELECT raise_exception('message text', 'detail text', _sqlstate => 'YY333');
SELECT raise_exception(_message => 'message text',
                       _detail => 'detail text',
                       _hint => 'hint text',
                       _sqlstate => 'XX555',
                       _schema_name => 'schema text',
                       _table_name => 'table text',
                       _column_name => 'column text',
                       _datatype_name => 'datatype text',
                       _constraint_name => 'constraint text');

SELECT raise_exception(_message => 'message text',
                       _hint => 'hint text',
                       _schema_name => 'schema text',
                       _column_name => 'column text',
                       _constraint_name => 'constraint text');

DO $$
DECLARE
  __message text;
  __detail text;
  __hint text;
  __sqlstate text;
  __schema_name text;
  __table_name text;
  __column_name text;
  __datatype_name text;
  __constraint_name text;
BEGIN
  BEGIN
    PERFORM raise_exception(_message => 'message text',
                            _detail => 'detail text',
                            _hint => 'hint text',
                            _sqlstate => 'XX555',
                            _schema_name => 'schema text',
                            _table_name => 'table text',
                            _column_name => 'column text',
                            _datatype_name => 'datatype text',
                            _constraint_name => 'constraint text');
  EXCEPTION WHEN SQLSTATE 'XX555' THEN
    GET STACKED DIAGNOSTICS __message = MESSAGE_TEXT,
                            __detail = PG_EXCEPTION_DETAIL,
                            __hint = PG_EXCEPTION_HINT,
                            __sqlstate = RETURNED_SQLSTATE,
                            __schema_name = SCHEMA_NAME,
                            __table_name = TABLE_NAME,
                            __column_name = COLUMN_NAME,
                            __datatype_name = PG_DATATYPE_NAME,
                            __constraint_name = CONSTRAINT_NAME;
    RAISE NOTICE 'handled exception'
      USING DETAIL = format('message:(%s), detail:(%s), hint: (%s), sqlstate: (%s), '
                            'schema_name:(%s), table_name:(%s), column_name:(%s), datatype_name:(%s), constraint_name:(%s)',
                            __message, __detail, __hint, __sqlstate, __schema_name,
                            __table_name, __column_name, __datatype_name, __constraint_name);
  END;
END;
$$;

DROP FUNCTION elog_test;
DROP FUNCTION raise_exception;
