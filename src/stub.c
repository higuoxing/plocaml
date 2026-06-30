#include <postgres.h>
#include <fmgr.h>
#include <utils/builtins.h>
#include <catalog/pg_proc.h>
#include <catalog/pg_type.h>
#include <utils/elog.h>
#include <utils/syscache.h>
#include <executor/spi.h>
#include <funcapi.h>
#include <access/htup_details.h>
#include <commands/extension.h>
#include <utils/lsyscache.h>
#include <utils/builtins.h>
#include <parser/parse_type.h>

#include <caml/mlvalues.h>
#include <caml/callback.h>
#include <caml/memory.h>
#include <utils/guc.h>
#include <caml/alloc.h>
#include <caml/fail.h>
#include <caml/custom.h>

#define DATUM_TAG_INT 0
#define DATUM_TAG_FLOAT 1
#define DATUM_TAG_STRING 2
#define DATUM_TAG_BOOL 3
#define DATUM_TAG_ARRAY 4

#define RESULT_TAG_OK 0
#define RESULT_TAG_SYNTAX_ERROR 1
#define RESULT_TAG_RUNTIME_ERROR 2

PG_MODULE_MAGIC;

static char *plocaml_stdlib_path = NULL;

const char plocaml_bootstrap_code[] = {
#embed "bootstrap.ml"
  , '\0'
};

CAMLprim value plocaml_magic_keepalive(value unit) {
  CAMLparam1(unit);
  extern const Pg_magic_struct * Pg_magic_func(void);
  const void *dummy = Pg_magic_func();
  CAMLreturn(Val_unit);
}

#define Custom_plan_val(v) (*((SPIPlanPtr *) Data_custom_val(v)))
#define Custom_cursor_val(v) (*((Portal *) Data_custom_val(v)))

static void finalize_spi_cursor(value v) {
  Portal cursor = Custom_cursor_val(v);
  if (cursor != NULL) {
    SPI_cursor_close(cursor);
  }
}

static struct custom_operations spi_cursor_ops = {
  "plocaml.spi_cursor",
  finalize_spi_cursor,
  custom_compare_default,
  custom_hash_default,
  custom_serialize_default,
  custom_deserialize_default,
  custom_compare_ext_default,
  custom_fixed_length_default
};

static void finalize_spi_plan(value v) {
  SPIPlanPtr plan = Custom_plan_val(v);
  if (plan != NULL) {
    SPI_freeplan(plan);
  }
}

static struct custom_operations spi_plan_ops = {
  "plocaml.spi_plan",
  finalize_spi_plan,
  custom_compare_default,
  custom_hash_default,
  custom_serialize_default,
  custom_deserialize_default,
  custom_compare_ext_default,
  custom_fixed_length_default
};

CAMLprim value plocaml_elog(value level_val, value msg_val) {
  CAMLparam2(level_val, msg_val);
  int elevel = Int_val(level_val);
  const char *msg = String_val(msg_val);

  if (elevel >= ERROR) {
    caml_failwith(msg);
  } else {
    ereport(elevel, (errmsg("%s", msg)));
    CAMLreturn(Val_unit);
  }
}

static value make_ocaml_datum(Oid type_oid, Datum val, bool isnull) {
  CAMLparam0();
  CAMLlocal2(v, s);

  if (isnull) {
    CAMLreturn(Val_int(0)); // Null variant
  }

  if (type_oid == INT4OID) {
    v = caml_alloc(1, DATUM_TAG_INT);
    Store_field(v, 0, Val_int(DatumGetInt32(val)));
  } else if (type_oid == INT8OID) {
    v = caml_alloc(1, DATUM_TAG_INT);
    Store_field(v, 0, Val_int(DatumGetInt64(val)));
  } else if (type_oid == INT2OID) {
    v = caml_alloc(1, DATUM_TAG_INT);
    Store_field(v, 0, Val_int(DatumGetInt16(val)));
  } else if (type_oid == FLOAT8OID) {
    v = caml_alloc(1, DATUM_TAG_FLOAT);
    value f = caml_copy_double(DatumGetFloat8(val));
    Store_field(v, 0, f);
  } else if (type_oid == BOOLOID) {
    v = caml_alloc(1, DATUM_TAG_BOOL);
    bool b = DatumGetBool(val);
    Store_field(v, 0, Val_int(b ? 1 : 0));
  } else {
    // Default to string for everything else
    v = caml_alloc(1, DATUM_TAG_STRING);
    char *str;
    Oid typoutput;
    bool typisvarlena;
    getTypeOutputInfo(type_oid, &typoutput, &typisvarlena);
    str = OidOutputFunctionCall(typoutput, val);
    s = caml_copy_string(str);
    Store_field(v, 0, s);
    pfree(str);
  }
  CAMLreturn(v);
}

static value build_spi_result(int status, int nrows) {
  CAMLparam0();
  CAMLlocal4(res, rows_arr, row_list, pair);
  CAMLlocal2(col_name, col_val);

  rows_arr = caml_alloc(nrows, 0);

  if (SPI_tuptable != NULL) {
    TupleDesc tupdesc = SPI_tuptable->tupdesc;
    for (int i = 0; i < nrows; i++) {
      HeapTuple tuple = SPI_tuptable->vals[i];
      row_list = Val_int(0); // []

      // Build the list backwards so it ends up in the correct order
      for (int j = tupdesc->natts; j > 0; j--) {
        bool isnull;
        Datum val = SPI_getbinval(tuple, tupdesc, j, &isnull);
        Oid type_oid = SPI_gettypeid(tupdesc, j);
        char *fname = SPI_fname(tupdesc, j);

        col_name = caml_copy_string(fname);
        pfree(fname);
        col_val = make_ocaml_datum(type_oid, val, isnull);

        pair = caml_alloc(2, 0);
        Store_field(pair, 0, col_name);
        Store_field(pair, 1, col_val);

        value new_node = caml_alloc(2, 0);
        Store_field(new_node, 0, pair);
        Store_field(new_node, 1, row_list);
        row_list = new_node;
      }
      Store_field(rows_arr, i, row_list);
    }
  }

  res = caml_alloc(3, 0);
  Store_field(res, 0, Val_int(status));
  Store_field(res, 1, Val_int(nrows));
  Store_field(res, 2, rows_arr);

  CAMLreturn(res);
}

CAMLprim value plocaml_spi_prepare(value query_val, value argtypes_val) {
  CAMLparam2(query_val, argtypes_val);
  CAMLlocal1(plan_val);
  const char *query = String_val(query_val);

  MemoryContext caller_context = CurrentMemoryContext;
  if (SPI_connect() != SPI_OK_CONNECT) {
    caml_failwith("PL/OCaml: could not connect to SPI manager");
  }

  volatile bool failed = false;
  ErrorData *edata = NULL;
  SPIPlanPtr plan = NULL;

  int nargs = Wosize_val(argtypes_val);
  Oid *argtypes = palloc(nargs * sizeof(Oid));

  PG_TRY();
  {
    for (int i = 0; i < nargs; i++) {
      char *type_name = String_val(Field(argtypes_val, i));
      Oid type_id;
      int32 typmod;
      parseTypeString(type_name, &type_id, &typmod, false);
      argtypes[i] = type_id;
    }

    plan = SPI_prepare(query, nargs, argtypes);
    if (plan != NULL) {
      SPI_keepplan(plan); // Keep it alive across SPI_finish
    } else {
      failed = true;
    }
  }
  PG_CATCH();
  {
    MemoryContextSwitchTo(caller_context);
    edata = CopyErrorData();
    FlushErrorState();
    failed = true;
  }
  PG_END_TRY();

  pfree(argtypes);

  if (failed) {
    SPI_finish();
    if (edata) {
      char *msg = pstrdup(edata->message);
      FreeErrorData(edata);
      caml_failwith(msg);
    } else {
      caml_failwith("PL/OCaml SPI_prepare failed");
    }
  }

  plan_val = caml_alloc_custom(&spi_plan_ops, sizeof(SPIPlanPtr), 0, 1);
  Custom_plan_val(plan_val) = plan;

  SPI_finish();
  CAMLreturn(plan_val);
}

CAMLprim value plocaml_spi_execute_plan(value plan_val, value args_val) {
  CAMLparam2(plan_val, args_val);

  MemoryContext caller_context = CurrentMemoryContext;
  if (SPI_connect() != SPI_OK_CONNECT) {
    caml_failwith("PL/OCaml: could not connect to SPI manager");
  }

  SPIPlanPtr plan = Custom_plan_val(plan_val);
  if (plan == NULL) {
    SPI_finish();
    caml_failwith("PL/OCaml: attempt to execute a freed plan");
  }

  int expected_nargs = SPI_getargcount(plan);
  int nargs = Wosize_val(args_val);
  if (nargs != expected_nargs) {
    SPI_finish();
    caml_failwith("PL/OCaml: incorrect number of arguments for plan");
  }

  int res = 0;
  volatile bool failed = false;
  ErrorData *edata = NULL;

  Datum *Values = palloc(nargs * sizeof(Datum));
  char *Nulls = palloc(nargs * sizeof(char));

  for (int i = 0; i < nargs; i++) {
    value elem = Field(args_val, i);
    if (Is_long(elem)) {
      Values[i] = (Datum) 0;
      Nulls[i] = 'n';
    } else {
      Nulls[i] = ' ';
      int e_tag = Tag_val(elem);
      if (e_tag == DATUM_TAG_INT) {
        Values[i] = Int32GetDatum(Int_val(Field(elem, 0)));
      } else if (e_tag == DATUM_TAG_FLOAT) {
        Values[i] = Float8GetDatum(Double_val(Field(elem, 0)));
      } else if (e_tag == DATUM_TAG_STRING) {
        Values[i] = CStringGetTextDatum(String_val(Field(elem, 0)));
      } else if (e_tag == DATUM_TAG_BOOL) {
        Values[i] = BoolGetDatum(Int_val(Field(elem, 0)) != 0);
      } else {
        caml_failwith("PL/OCaml: unsupported argument type for SPI_execute_plan");
      }
    }
  }

  PG_TRY();
  {
    res = SPI_execute_plan(plan, Values, Nulls, false, 0);
    if (res < 0) {
      failed = true;
    }
  }
  PG_CATCH();
  {
    MemoryContextSwitchTo(caller_context);
    edata = CopyErrorData();
    FlushErrorState();
    failed = true;
  }
  PG_END_TRY();

  pfree(Values);
  pfree(Nulls);

  if (failed) {
    SPI_finish();
    if (edata) {
      char *msg = pstrdup(edata->message);
      FreeErrorData(edata);
      caml_failwith(msg);
    } else {
      caml_failwith("PL/OCaml SPI_execute_plan failed");
    }
  }

  int rows = SPI_processed;
  value result = build_spi_result(res, rows);
  SPI_finish();
  CAMLreturn(result);
}

CAMLprim value plocaml_spi_cursor(value query_val) {
  CAMLparam1(query_val);
  CAMLlocal1(cursor_val);
  const char *query = String_val(query_val);

  MemoryContext caller_context = CurrentMemoryContext;
  if (SPI_connect() != SPI_OK_CONNECT) {
    caml_failwith("PL/OCaml: could not connect to SPI manager");
  }

  volatile bool failed = false;
  ErrorData *edata = NULL;
  Portal cursor = NULL;
  SPIPlanPtr plan = NULL;

  PG_TRY();
  {
    plan = SPI_prepare(query, 0, NULL);
    if (plan != NULL) {
      SPI_keepplan(plan);
      cursor = SPI_cursor_open(NULL, plan, NULL, NULL, false);
      SPI_freeplan(plan);
    } else {
      failed = true;
    }
  }
  PG_CATCH();
  {
    MemoryContextSwitchTo(caller_context);
    edata = CopyErrorData();
    FlushErrorState();
    failed = true;
  }
  PG_END_TRY();

  if (failed) {
    SPI_finish();
    if (edata) {
      char *msg = pstrdup(edata->message);
      FreeErrorData(edata);
      caml_failwith(msg);
    } else {
      caml_failwith("PL/OCaml SPI_cursor failed");
    }
  }

  cursor_val = caml_alloc_custom(&spi_cursor_ops, sizeof(Portal), 0, 1);
  Custom_cursor_val(cursor_val) = cursor;

  SPI_finish();
  CAMLreturn(cursor_val);
}

CAMLprim value plocaml_spi_cursor_plan(value plan_val, value args_val) {
  CAMLparam2(plan_val, args_val);
  CAMLlocal1(cursor_val);

  MemoryContext caller_context = CurrentMemoryContext;
  if (SPI_connect() != SPI_OK_CONNECT) {
    caml_failwith("PL/OCaml: could not connect to SPI manager");
  }

  SPIPlanPtr plan = Custom_plan_val(plan_val);
  if (plan == NULL) {
    SPI_finish();
    caml_failwith("PL/OCaml: attempt to create cursor from a freed plan");
  }

  int expected_nargs = SPI_getargcount(plan);
  int nargs = Wosize_val(args_val);
  if (nargs != expected_nargs) {
    SPI_finish();
    caml_failwith("PL/OCaml: incorrect number of arguments for plan");
  }

  volatile bool failed = false;
  ErrorData *edata = NULL;
  Portal cursor = NULL;

  Datum *Values = palloc(nargs * sizeof(Datum));
  char *Nulls = palloc(nargs * sizeof(char));

  for (int i = 0; i < nargs; i++) {
    value elem = Field(args_val, i);
    if (Is_long(elem)) {
      Values[i] = (Datum) 0;
      Nulls[i] = 'n';
    } else {
      Nulls[i] = ' ';
      int e_tag = Tag_val(elem);
      if (e_tag == DATUM_TAG_INT) {
        Values[i] = Int32GetDatum(Int_val(Field(elem, 0)));
      } else if (e_tag == DATUM_TAG_FLOAT) {
        Values[i] = Float8GetDatum(Double_val(Field(elem, 0)));
      } else if (e_tag == DATUM_TAG_STRING) {
        Values[i] = CStringGetTextDatum(String_val(Field(elem, 0)));
      } else if (e_tag == DATUM_TAG_BOOL) {
        Values[i] = BoolGetDatum(Int_val(Field(elem, 0)) != 0);
      } else {
        caml_failwith("PL/OCaml: unsupported argument type for SPI_cursor_plan");
      }
    }
  }

  PG_TRY();
  {
    cursor = SPI_cursor_open(NULL, plan, Values, Nulls, false);
    if (cursor == NULL) {
      failed = true;
    }
  }
  PG_CATCH();
  {
    MemoryContextSwitchTo(caller_context);
    edata = CopyErrorData();
    FlushErrorState();
    failed = true;
  }
  PG_END_TRY();

  pfree(Values);
  pfree(Nulls);

  if (failed) {
    SPI_finish();
    if (edata) {
      char *msg = pstrdup(edata->message);
      FreeErrorData(edata);
      caml_failwith(msg);
    } else {
      caml_failwith("PL/OCaml SPI_cursor_plan failed");
    }
  }

  cursor_val = caml_alloc_custom(&spi_cursor_ops, sizeof(Portal), 0, 1);
  Custom_cursor_val(cursor_val) = cursor;

  SPI_finish();
  CAMLreturn(cursor_val);
}

CAMLprim value plocaml_spi_fetch(value cursor_val, value count_val) {
  CAMLparam2(cursor_val, count_val);

  MemoryContext caller_context = CurrentMemoryContext;
  if (SPI_connect() != SPI_OK_CONNECT) {
    caml_failwith("PL/OCaml: could not connect to SPI manager");
  }

  Portal cursor = Custom_cursor_val(cursor_val);
  if (cursor == NULL) {
    SPI_finish();
    caml_failwith("PL/OCaml: attempt to fetch from a closed cursor");
  }

  int count = Int_val(count_val);
  int res = 0;
  volatile bool failed = false;
  ErrorData *edata = NULL;

  PG_TRY();
  {
    SPI_cursor_fetch(cursor, true, count);
    res = SPI_processed;
  }
  PG_CATCH();
  {
    MemoryContextSwitchTo(caller_context);
    edata = CopyErrorData();
    FlushErrorState();
    failed = true;
  }
  PG_END_TRY();

  if (failed) {
    SPI_finish();
    if (edata) {
      char *msg = pstrdup(edata->message);
      FreeErrorData(edata);
      caml_failwith(msg);
    } else {
      caml_failwith("PL/OCaml SPI_fetch failed");
    }
  }

  value result = build_spi_result(SPI_OK_FETCH, res);
  SPI_finish();
  CAMLreturn(result);
}

CAMLprim value plocaml_spi_close(value cursor_val) {
  CAMLparam1(cursor_val);
  Portal cursor = Custom_cursor_val(cursor_val);
  if (cursor != NULL) {
    SPI_cursor_close(cursor);
    Custom_cursor_val(cursor_val) = NULL;
  }
  CAMLreturn(Val_unit);
}

CAMLprim value plocaml_spi_execute_with_args(value query_val, value args_val) {
  CAMLparam2(query_val, args_val);
  const char *query = String_val(query_val);
  
  MemoryContext caller_context = CurrentMemoryContext;
  if (SPI_connect() != SPI_OK_CONNECT) {
    caml_failwith("PL/OCaml: could not connect to SPI manager");
  }

  int res = 0;
  volatile bool failed = false;
  ErrorData *edata = NULL;

  int nargs = Wosize_val(args_val);
  Oid *argtypes = palloc(nargs * sizeof(Oid));
  Datum *Values = palloc(nargs * sizeof(Datum));
  char *Nulls = palloc(nargs * sizeof(char));

  for (int i = 0; i < nargs; i++) {
    value elem = Field(args_val, i);
    if (Is_long(elem)) {
      argtypes[i] = TEXTOID; // Default to text for nulls
      Values[i] = (Datum) 0;
      Nulls[i] = 'n';
    } else {
      Nulls[i] = ' ';
      int e_tag = Tag_val(elem);
      if (e_tag == DATUM_TAG_INT) {
        argtypes[i] = INT4OID;
        Values[i] = Int32GetDatum(Int_val(Field(elem, 0)));
      } else if (e_tag == DATUM_TAG_FLOAT) {
        argtypes[i] = FLOAT8OID;
        Values[i] = Float8GetDatum(Double_val(Field(elem, 0)));
      } else if (e_tag == DATUM_TAG_STRING) {
        argtypes[i] = TEXTOID;
        Values[i] = CStringGetTextDatum(String_val(Field(elem, 0)));
      } else if (e_tag == DATUM_TAG_BOOL) {
        argtypes[i] = BOOLOID;
        Values[i] = BoolGetDatum(Int_val(Field(elem, 0)) != 0);
      } else {
        caml_failwith("PL/OCaml: unsupported argument type for SPI_execute");
      }
    }
  }

  PG_TRY();
  {
    res = SPI_execute_with_args(query, nargs, argtypes, Values, Nulls, false, 0);
    if (res < 0) {
      failed = true;
    }
  }
  PG_CATCH();
  {
    MemoryContextSwitchTo(caller_context);
    edata = CopyErrorData();
    FlushErrorState();
    failed = true;
  }
  PG_END_TRY();

  pfree(argtypes);
  pfree(Values);
  pfree(Nulls);

  if (failed) {
    SPI_finish();
    if (edata) {
      char *msg = pstrdup(edata->message);
      FreeErrorData(edata);
      caml_failwith(msg);
    } else {
      caml_failwith("PL/OCaml SPI_execute_with_args failed");
    }
  }
  
  int rows = SPI_processed;
  value result = build_spi_result(res, rows);
  SPI_finish();
  CAMLreturn(result);
}

CAMLprim value plocaml_spi_execute(value query_val) {
  CAMLparam1(query_val);
  const char *query = String_val(query_val);
  
  MemoryContext caller_context = CurrentMemoryContext;
  if (SPI_connect() != SPI_OK_CONNECT) {
    caml_failwith("PL/OCaml: could not connect to SPI manager");
  }

  int res = 0;
  volatile bool failed = false;
  ErrorData *edata = NULL;

  PG_TRY();
  {
    res = SPI_execute(query, false, 0);
    if (res < 0) {
      failed = true;
    }
  }
  PG_CATCH();
  {
    MemoryContextSwitchTo(caller_context);
    edata = CopyErrorData();
    FlushErrorState();
    failed = true;
  }
  PG_END_TRY();

  if (failed) {
    SPI_finish();
    if (edata) {
      char *msg = pstrdup(edata->message);
      FreeErrorData(edata);
      caml_failwith(msg);
    } else {
      caml_failwith("PL/OCaml SPI_execute failed");
    }
  }

  int rows = SPI_processed;
  value result = build_spi_result(res, rows);
  SPI_finish();
  CAMLreturn(result);
}

void _PG_init(void) {
  DefineCustomStringVariable(
    "plocaml.stdlib_path",
    "Path to the OCaml standard library (.cmi files)",
    "Needed for Toploop to compile OCaml code dynamically.",
    &plocaml_stdlib_path,
    "", /* default value */
    PGC_SUSET,
    0,
    NULL, NULL, NULL
  );

  char *caml_argv[] = {"postgres_plocaml", NULL};
  caml_startup(caml_argv);

  /* Initialize top level. */
  const value *init_top_level_fn = caml_named_value("plocaml_init_toplevel");
  if (init_top_level_fn) {
    value boot_val = caml_copy_string(plocaml_bootstrap_code);
    value stdlib_val = caml_copy_string(plocaml_stdlib_path ? plocaml_stdlib_path : "");
    caml_callback2(*init_top_level_fn, boot_val, stdlib_val);
  }
}

PG_FUNCTION_INFO_V1(plocaml_call_handler);
PG_FUNCTION_INFO_V1(plocaml_inline_handler);

Datum plocaml_inline_handler(PG_FUNCTION_ARGS) {
  InlineCodeBlock *codeblock = (InlineCodeBlock *) DatumGetPointer(PG_GETARG_DATUM(0));
  char *user_sql_code = codeblock->source_text;
  char *func_name = "inline_code_block";
  int oid = 0;
  value args_arr, res;

  args_arr = caml_alloc(0, 0); // Empty array

  const value *execute_fn = caml_named_value("plocaml_execute");
  if (!execute_fn) {
    ereport(ERROR,
            (errcode(ERRCODE_INTERNAL_ERROR),
             errmsg("PL/OCaml engine error"),
             errdetail("Execute function not found.")));
  }

  value callback_args[] = {Val_int(oid), caml_copy_string(func_name), caml_copy_string(user_sql_code), args_arr};
  res = caml_callbackN_exn(*execute_fn, 4, callback_args);
  
  if (Is_exception_result(res)) {
    res = Extract_exception(res);
    char *err_msg = strdup(String_val(Field(res, 0))); // simplified exception message
    ereport(ERROR,
            (errcode(ERRCODE_INTERNAL_ERROR),
             errmsg("PL/OCaml fatal engine exception"),
             errdetail("%s", err_msg)));
  }

  if (Is_block(res)) {
    int tag = Tag_val(res);
    
    if (tag == RESULT_TAG_OK) {
      PG_RETURN_VOID();
    } else if (tag == RESULT_TAG_SYNTAX_ERROR) {
      // SyntaxError of string
      const char *err_msg = String_val(Field(res, 0));
      ereport(ERROR,
              (errcode(ERRCODE_SYNTAX_ERROR),
               errmsg("PL/OCaml syntax error"),
               errdetail("%s", err_msg)));
    } else if (tag == RESULT_TAG_RUNTIME_ERROR) {
      // RuntimeError of string
      const char *err_msg = String_val(Field(res, 0));
      ereport(ERROR,
              (errcode(ERRCODE_EXTERNAL_ROUTINE_EXCEPTION),
               errmsg("PL/OCaml execution failed"),
               errdetail("%s", err_msg)));
    } else {
      elog(ERROR, "PL/OCaml engine error: Unexpected return variant tag from OCaml.");
    }
  }

  elog(ERROR, "PL/OCaml engine error: Unexpected return variant from OCaml.");
  pg_unreachable();
}

Datum plocaml_call_handler(PG_FUNCTION_ARGS) {
  int oid = fcinfo->flinfo->fn_oid;
  bool isnull;
  HeapTuple procTup;
  Datum prosrc;
  char *user_sql_code;
  char *func_name;
  int nargs;
  value args_arr, res;

  procTup = SearchSysCache1(PROCOID, ObjectIdGetDatum(oid));
  if (!HeapTupleIsValid(procTup)) {
    elog(ERROR, "cache lookup failed for function %u", oid);
  }

  bool name_isnull;
  Datum proname_datum = SysCacheGetAttr(PROCOID, procTup, Anum_pg_proc_proname, &name_isnull);
  func_name = NameStr(*DatumGetName(proname_datum));

  prosrc = SysCacheGetAttr(PROCOID, procTup, Anum_pg_proc_prosrc, &isnull);
  if (isnull) {
    ReleaseSysCache(procTup);
    elog(ERROR, "null prosrc for function %u", oid);
  }

  user_sql_code = TextDatumGetCString(prosrc);
  ReleaseSysCache(procTup);
  prosrc = (Datum) 0;

  // Remove all notices
  // Convert Postgres arguments to OCaml arguments array
  nargs = PG_NARGS();
  args_arr = caml_alloc(nargs, 0); // 0 tag for Tuple/Array
  for (int i = 0; i < nargs; i++) {
    value v;
    if (PG_ARGISNULL(i)) {
      v = Val_int(0); // Null variant (integer 0)
    } else {
      Oid type_oid = get_fn_expr_argtype(fcinfo->flinfo, i);
      Datum arg = PG_GETARG_DATUM(i);
      
      if (type_oid == INT4OID) {
        v = caml_alloc(1, DATUM_TAG_INT);
        Store_field(v, 0, Val_int(DatumGetInt32(arg)));
      } else if (type_oid == FLOAT8OID) {
        v = caml_alloc(1, DATUM_TAG_FLOAT);
        value f = caml_copy_double(DatumGetFloat8(arg));
        Store_field(v, 0, f);
      } else if (type_oid == TEXTOID || type_oid == VARCHAROID) {
        v = caml_alloc(1, DATUM_TAG_STRING);
        char *str = TextDatumGetCString(arg);
        value s = caml_copy_string(str);
        Store_field(v, 0, s);
        pfree(str);
      } else if (type_oid == BOOLOID) {
        v = caml_alloc(1, DATUM_TAG_BOOL);
        bool b = DatumGetBool(arg);
        Store_field(v, 0, Val_int(b ? 1 : 0));
      } else {
        elog(ERROR, "PL/OCaml: unsupported argument type OID %u", type_oid);
      }
    }
    Store_field(args_arr, i, v);
  }

  const value *execute_fn = caml_named_value("plocaml_execute");
  if (!execute_fn) {
    ereport(ERROR,
            (errcode(ERRCODE_INTERNAL_ERROR),
             errmsg("PL/OCaml engine error"),
             errdetail("Execute function not found.")));
  }

  value callback_args[] = {Val_int(oid), caml_copy_string(func_name), caml_copy_string(user_sql_code), args_arr};
  res = caml_callbackN_exn(*execute_fn, 4, callback_args);
  
  if (Is_exception_result(res)) {
    res = Extract_exception(res);
    char *err_msg = strdup(String_val(Field(res, 0))); // simplified exception message
    ereport(ERROR,
            (errcode(ERRCODE_INTERNAL_ERROR),
             errmsg("PL/OCaml fatal engine exception"),
             errdetail("%s", err_msg)));
  }

  if (Is_block(res)) {
    int tag = Tag_val(res);
    
    if (tag == RESULT_TAG_OK) {
      // Ok of datum
      value datum_val = Field(res, 0);
      
      // If it's a procedure (void return type), enforce returning Null
      if (get_fn_expr_rettype(fcinfo->flinfo) == VOIDOID) {
        if (!Is_long(datum_val)) {
          ereport(ERROR,
                  (errcode(ERRCODE_DATATYPE_MISMATCH),
                   errmsg("PL/OCaml function with return type \"void\" did not return Null")));
        }
        fcinfo->isnull = false;
        return (Datum) 0;
      }

      if (Is_long(datum_val)) {
        // Null
        fcinfo->isnull = true;
        return (Datum) 0;
      } else {
        int d_tag = Tag_val(datum_val);
        Datum return_datum;

        if (d_tag == DATUM_TAG_INT) {
          int result = Int_val(Field(datum_val, 0));
          return_datum = Int32GetDatum(result);
        } else if (d_tag == DATUM_TAG_FLOAT) {
          double result = Double_val(Field(datum_val, 0));
          return_datum = Float8GetDatum(result);
        } else if (d_tag == DATUM_TAG_STRING) {
          const char *result = String_val(Field(datum_val, 0));
          return_datum = CStringGetTextDatum(result);
        } else if (d_tag == DATUM_TAG_BOOL) {
          bool result = (Int_val(Field(datum_val, 0)) != 0);
          return_datum = BoolGetDatum(result);
        } else if (d_tag == DATUM_TAG_ARRAY) {
          TupleDesc tupdesc;
          if (get_call_result_type(fcinfo, NULL, &tupdesc) != TYPEFUNC_COMPOSITE) {
            ereport(ERROR, (errcode(ERRCODE_FEATURE_NOT_SUPPORTED), errmsg("function returning record called in context that cannot accept type record")));
          }
          
          BlessTupleDesc(tupdesc);
          
          value arr = Field(datum_val, 0);
          int arr_len = Wosize_val(arr);
          if (arr_len != tupdesc->natts) {
             ereport(ERROR, (errcode(ERRCODE_DATATYPE_MISMATCH), errmsg("PL/OCaml array length %d does not match expected record length %d", arr_len, tupdesc->natts)));
          }

          Datum *values = palloc(tupdesc->natts * sizeof(Datum));
          bool *nulls = palloc(tupdesc->natts * sizeof(bool));

          for (int i = 0; i < tupdesc->natts; i++) {
             value elem = Field(arr, i);
             if (Is_long(elem)) {
                 nulls[i] = true;
                 values[i] = (Datum) 0;
             } else {
                 nulls[i] = false;
                 int e_tag = Tag_val(elem);
                 if (e_tag == DATUM_TAG_INT) {
                     values[i] = Int32GetDatum(Int_val(Field(elem, 0)));
                 } else if (e_tag == DATUM_TAG_FLOAT) {
                     values[i] = Float8GetDatum(Double_val(Field(elem, 0)));
                 } else if (e_tag == DATUM_TAG_STRING) {
                     values[i] = CStringGetTextDatum(String_val(Field(elem, 0)));
                 } else if (e_tag == DATUM_TAG_BOOL) {
                     values[i] = BoolGetDatum(Int_val(Field(elem, 0)) != 0);
                 } else {
                     elog(ERROR, "PL/OCaml engine error: Unexpected datum variant tag in array element.");
                 }
             }
          }
          
          HeapTuple tuple = heap_form_tuple(tupdesc, values, nulls);
          return_datum = heap_copy_tuple_as_datum(tuple, tupdesc);
          heap_freetuple(tuple);
        } else {
          elog(ERROR, "PL/OCaml engine error: Unexpected datum variant tag.");
        }
        
        return return_datum;
      }
    } else if (tag == RESULT_TAG_SYNTAX_ERROR) {
      // SyntaxError of string
      const char *err_msg = String_val(Field(res, 0));
      ereport(ERROR,
              (errcode(ERRCODE_SYNTAX_ERROR),
               errmsg("PL/OCaml syntax error"),
               errdetail("%s", err_msg)));
    } else if (tag == RESULT_TAG_RUNTIME_ERROR) {
      // RuntimeError of string
      const char *err_msg = String_val(Field(res, 0));
      ereport(ERROR,
              (errcode(ERRCODE_EXTERNAL_ROUTINE_EXCEPTION),
               errmsg("PL/OCaml execution failed"),
               errdetail("%s", err_msg)));
    } else {
      elog(ERROR, "PL/OCaml engine error: Unexpected return variant tag from OCaml.");
    }
  }

  elog(ERROR, "PL/OCaml engine error: Unexpected return variant from OCaml.");
  pg_unreachable();
}
