// clang-format off
#include <postgres.h>
// clang-format on

#include <access/htup_details.h>
#include <catalog/pg_proc.h>
#include <catalog/pg_type.h>
#include <commands/extension.h>
#include <executor/spi.h>
#include <fmgr.h>
#include <funcapi.h>
#include <parser/parse_type.h>
#include <utils/builtins.h>
#include <utils/elog.h>
#include <utils/lsyscache.h>
#include <utils/memutils.h>
#include <utils/syscache.h>
#include <utils/tuplestore.h>

#include <caml/alloc.h>
#include <caml/callback.h>
#include <caml/custom.h>
#include <caml/fail.h>
#include <caml/memory.h>
#include <caml/mlvalues.h>
#include <utils/guc.h>

#include "plocaml.h"

PG_MODULE_MAGIC;

static char *plocaml_stdlib_path = NULL;

/*
 * When a PL.error/PL.report at ERROR level fires, plocaml_report captures the
 * full ErrorData here and unwinds through OCaml via caml_failwith. The call
 * boundary (plocaml_handle_error) re-throws it so all error fields (detail,
 * hint, sqlstate, schema/table/column/datatype/constraint) survive to the
 * final ereport. The copy lives in plocaml_error_cxt so it outlives SPI/exec
 * memory contexts torn down during unwinding.
 */
static MemoryContext plocaml_error_cxt = NULL;
static ErrorData *plocaml_pending_edata = NULL;

const char plocaml_bootstrap_code[] = {
#embed "bootstrap.ml"
    , '\0'};

CAMLprim value plocaml_magic_keepalive(value unit) {
  CAMLparam1(unit);
  extern const Pg_magic_struct *Pg_magic_func(void);
  const void *dummy = Pg_magic_func();
  CAMLreturn(Val_unit);
}

/*
 * Capture the in-flight PostgreSQL error and mark it pending so that
 * plocaml_handle_error re-throws it at the call boundary, preserving every
 * field (sqlstate, detail, hint, ...). Must be called from within a PG_CATCH,
 * with the current memory context switched away from ErrorContext. Returns the
 * error message (valid until the next plocaml_reset_error_state).
 */
const char *plocaml_stash_pending_error(void) {
  MemoryContext old = MemoryContextSwitchTo(plocaml_error_cxt);
  ErrorData *edata = CopyErrorData();
  FlushErrorState();
  MemoryContextSwitchTo(old);
  plocaml_pending_edata = edata;
  return edata->message;
}

/* Read a [string option] record field: None -> NULL, Some s -> s. */
#define OPT_STR(info, i)                                                       \
  (Is_block(Field((info), (i))) ? String_val(Field(Field((info), (i)), 0))     \
                                : NULL)

CAMLprim value plocaml_report(value level_val, value info) {
  CAMLparam2(level_val, info);
  int elevel = Int_val(level_val);

  const char *message = String_val(Field(info, 0));
  const char *detail = OPT_STR(info, 1);
  const char *hint = OPT_STR(info, 2);
  const char *sqlstate_str = OPT_STR(info, 3);
  const char *schema_name = OPT_STR(info, 4);
  const char *table_name = OPT_STR(info, 5);
  const char *column_name = OPT_STR(info, 6);
  const char *datatype_name = OPT_STR(info, 7);
  const char *constraint_name = OPT_STR(info, 8);

  MemoryContext caller_context = CurrentMemoryContext;
  volatile bool failed = false;
  const char *errmsg_copy = NULL;

  PG_TRY();
  {
    int sqlstate = 0;
    if (sqlstate_str != NULL) {
      if (strlen(sqlstate_str) != 5 ||
          strspn(sqlstate_str, "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ") != 5)
        ereport(ERROR, (errcode(ERRCODE_INVALID_PARAMETER_VALUE),
                        errmsg("invalid SQLSTATE code")));
      sqlstate = MAKE_SQLSTATE(sqlstate_str[0], sqlstate_str[1], sqlstate_str[2],
                               sqlstate_str[3], sqlstate_str[4]);
    }

    ereport(elevel,
            ((sqlstate != 0) ? errcode(sqlstate) : 0,
             errmsg_internal("%s", message),
             (detail) ? errdetail_internal("%s", detail) : 0,
             (hint) ? errhint("%s", hint) : 0,
             (column_name) ? err_generic_string(PG_DIAG_COLUMN_NAME, column_name)
                           : 0,
             (constraint_name)
                 ? err_generic_string(PG_DIAG_CONSTRAINT_NAME, constraint_name)
                 : 0,
             (datatype_name)
                 ? err_generic_string(PG_DIAG_DATATYPE_NAME, datatype_name)
                 : 0,
             (table_name) ? err_generic_string(PG_DIAG_TABLE_NAME, table_name)
                          : 0,
             (schema_name) ? err_generic_string(PG_DIAG_SCHEMA_NAME, schema_name)
                           : 0));
    /* elevel < ERROR returns here; ERROR longjmps to PG_CATCH. */
  }
  PG_CATCH();
  {
    MemoryContextSwitchTo(caller_context);
    errmsg_copy = plocaml_stash_pending_error();
    failed = true;
  }
  PG_END_TRY();

  if (failed) {
    /* Unwind through OCaml; the boundary re-throws the stashed error. */
    caml_failwith(errmsg_copy ? errmsg_copy : "PL/OCaml error");
  }

  CAMLreturn(Val_unit);
}

value make_ocaml_datum(Oid type_oid, Datum val, bool isnull) {
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

void _PG_init(void) {
  DefineCustomStringVariable(
      "plocaml.stdlib_path", "Path to the OCaml standard library (.cmi files)",
      "Needed for Toploop to compile OCaml code dynamically.",
      &plocaml_stdlib_path, "", /* default value */
      PGC_SUSET, 0, NULL, NULL, NULL);

  plocaml_error_cxt = AllocSetContextCreate(TopMemoryContext, "PL/OCaml error",
                                            ALLOCSET_SMALL_SIZES);

  char *caml_argv[] = {"postgres_plocaml", NULL};
  caml_startup(caml_argv);

  /* Initialize top level. */
  const value *init_top_level_fn = caml_named_value("plocaml_init_toplevel");
  if (init_top_level_fn) {
    value boot_val = caml_copy_string(plocaml_bootstrap_code);
    value stdlib_val =
        caml_copy_string(plocaml_stdlib_path ? plocaml_stdlib_path : "");
    caml_callback2(*init_top_level_fn, boot_val, stdlib_val);
  }
}

static void plocaml_handle_error(value res);

/* Drop any error state left over from a prior top-level call (e.g. a PL.error
   whose OCaml exception was caught and swallowed by user code). */
static void plocaml_reset_error_state(void) {
  plocaml_pending_edata = NULL;
  if (plocaml_error_cxt != NULL)
    MemoryContextReset(plocaml_error_cxt);
}

PG_FUNCTION_INFO_V1(plocamlu_inline_handler);
Datum plocamlu_inline_handler(PG_FUNCTION_ARGS) {
  InlineCodeBlock *codeblock =
      (InlineCodeBlock *)DatumGetPointer(PG_GETARG_DATUM(0));
  char *user_sql_code = codeblock->source_text;
  char *func_name = "inline_code_block";
  int oid = 0;
  value args_arr, res;

  plocaml_reset_error_state();

  args_arr = caml_alloc(0, 0); // Empty array

  const value *execute_fn = caml_named_value("plocaml_execute");
  if (!execute_fn) {
    ereport(ERROR,
            (errcode(ERRCODE_INTERNAL_ERROR), errmsg("PL/OCaml engine error"),
             errdetail("Execute function not found.")));
  }

  value callback_args[] = {Val_int(oid), caml_copy_string(func_name),
                           caml_copy_string(user_sql_code), args_arr};
  res = caml_callbackN_exn(*execute_fn, 4, callback_args);

  if (Is_exception_result(res) || (Is_block(res) && Tag_val(res) != RESULT_TAG_OK)) {
    plocaml_handle_error(res);
  }

  PG_RETURN_VOID();
}

static value plocaml_build_args(FunctionCallInfo fcinfo) {
  int nargs = PG_NARGS();
  value args_arr = caml_alloc(nargs, 0); // 0 tag for Tuple/Array
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
  return args_arr;
}

static HeapTuple plocaml_build_heap_tuple(value arr, TupleDesc tupdesc) {
  int arr_len = Wosize_val(arr);
  if (arr_len != tupdesc->natts) {
    ereport(ERROR, (errcode(ERRCODE_DATATYPE_MISMATCH),
                    errmsg("length of returned sequence did not match number "
                           "of columns in row")));
  }

  Datum *values = palloc(tupdesc->natts * sizeof(Datum));
  bool *nulls = palloc(tupdesc->natts * sizeof(bool));

  for (int i = 0; i < tupdesc->natts; i++) {
    value elem = Field(arr, i);
    if (Is_long(elem)) {
      nulls[i] = true;
      values[i] = (Datum)0;
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
        elog(ERROR, "PL/OCaml engine error: Unexpected datum variant tag in "
                    "array element.");
      }
    }
  }

  HeapTuple tuple = heap_form_tuple(tupdesc, values, nulls);
  pfree(values);
  pfree(nulls);
  return tuple;
}

static Datum plocaml_convert_datum(FunctionCallInfo fcinfo, value datum_val,
                                   bool *isnull) {
  if (Is_long(datum_val)) {
    *isnull = true;
    return (Datum)0;
  }

  *isnull = false;
  int d_tag = Tag_val(datum_val);

  if (d_tag == DATUM_TAG_INT) {
    return Int32GetDatum(Int_val(Field(datum_val, 0)));
  } else if (d_tag == DATUM_TAG_FLOAT) {
    return Float8GetDatum(Double_val(Field(datum_val, 0)));
  } else if (d_tag == DATUM_TAG_STRING) {
    return CStringGetTextDatum(String_val(Field(datum_val, 0)));
  } else if (d_tag == DATUM_TAG_BOOL) {
    return BoolGetDatum(Int_val(Field(datum_val, 0)) != 0);
  } else if (d_tag == DATUM_TAG_ARRAY) {
    TupleDesc tupdesc;
    if (get_call_result_type(fcinfo, NULL, &tupdesc) != TYPEFUNC_COMPOSITE) {
      ereport(ERROR, (errcode(ERRCODE_FEATURE_NOT_SUPPORTED),
                      errmsg("function returning record called in context that "
                             "cannot accept type record")));
    }
    BlessTupleDesc(tupdesc);
    HeapTuple tuple = plocaml_build_heap_tuple(Field(datum_val, 0), tupdesc);
    Datum return_datum = heap_copy_tuple_as_datum(tuple, tupdesc);
    heap_freetuple(tuple);
    return return_datum;
  } else {
    elog(ERROR, "PL/OCaml engine error: Unexpected datum variant tag.");
  }
  return (Datum)0;
}

static Datum plocaml_handle_setof(FunctionCallInfo fcinfo, value datum_val) {
  ReturnSetInfo *rsinfo = (ReturnSetInfo *)fcinfo->resultinfo;
  if (rsinfo == NULL || !IsA(rsinfo, ReturnSetInfo) ||
      (rsinfo->allowedModes & SFRM_Materialize) == 0)
    ereport(ERROR, (errcode(ERRCODE_FEATURE_NOT_SUPPORTED),
                    errmsg("set-valued function called in context that cannot "
                           "accept a set")));

  InitMaterializedSRF(fcinfo, 0);

  if (Is_long(datum_val)) {
    return (Datum)0; // empty set
  }

  if (Tag_val(datum_val) != DATUM_TAG_ARRAY) {
    ereport(ERROR,
            (errcode(ERRCODE_DATATYPE_MISMATCH),
             errmsg("SETOF returning functions must return a PL.Array")));
  }

  value rows_arr = Field(datum_val, 0);
  int num_rows = Wosize_val(rows_arr);
  TupleDesc tupdesc = rsinfo->setDesc;
  BlessTupleDesc(tupdesc);

  bool is_composite =
      (tupdesc->natts > 1 ||
       get_call_result_type(fcinfo, NULL, NULL) == TYPEFUNC_COMPOSITE);

  for (int r = 0; r < num_rows; r++) {
    value row_val = Field(rows_arr, r);

    if (is_composite) {
      if (Is_long(row_val) || Tag_val(row_val) != DATUM_TAG_ARRAY) {
        ereport(ERROR, (errcode(ERRCODE_DATATYPE_MISMATCH),
                        errmsg("SETOF record must return an array of arrays")));
      }
      HeapTuple tuple = plocaml_build_heap_tuple(Field(row_val, 0), tupdesc);
      tuplestore_puttuple(rsinfo->setResult, tuple);
      heap_freetuple(tuple);
    } else {
      bool is_null;
      Datum value_datum = plocaml_convert_datum(fcinfo, row_val, &is_null);
      tuplestore_putvalues(rsinfo->setResult, tupdesc, &value_datum, &is_null);
    }
  }
  return (Datum)0;
}

static void plocaml_handle_error(value res) {
  /* A PL.error/PL.report(Error) captured a full ErrorData; re-throw it so all
     error fields survive to the final ereport (and GET STACKED DIAGNOSTICS). */
  if (plocaml_pending_edata != NULL) {
    ErrorData *edata = plocaml_pending_edata;
    plocaml_pending_edata = NULL;
    ReThrowError(edata);
  }

  if (Is_exception_result(res)) {
    res = Extract_exception(res);
    char *err_msg =
        strdup(String_val(Field(res, 0))); // simplified exception message
    ereport(ERROR, (errcode(ERRCODE_INTERNAL_ERROR),
                    errmsg("PL/OCaml fatal engine exception"),
                    errdetail("%s", err_msg)));
  }

  if (Is_block(res)) {
    int tag = Tag_val(res);
    if (tag == RESULT_TAG_SYNTAX_ERROR) {
      const char *err_msg = String_val(Field(res, 0));
      ereport(ERROR,
              (errcode(ERRCODE_SYNTAX_ERROR), errmsg("PL/OCaml syntax error"),
               errdetail("%s", err_msg)));
    } else if (tag == RESULT_TAG_RUNTIME_ERROR) {
      const char *err_msg = String_val(Field(res, 0));
      ereport(ERROR,
              (errcode(ERRCODE_EXTERNAL_ROUTINE_EXCEPTION),
               errmsg("PL/OCaml execution failed"), errdetail("%s", err_msg)));
    }
  }

  elog(ERROR, "PL/OCaml engine error: Unexpected return variant from OCaml.");
  pg_unreachable();
}

PG_FUNCTION_INFO_V1(plocamlu_call_handler);
Datum plocamlu_call_handler(PG_FUNCTION_ARGS) {
  int oid = fcinfo->flinfo->fn_oid;
  bool isnull;
  HeapTuple procTup;
  Datum prosrc;
  char *user_sql_code;
  char *func_name;
  value args_arr, res;

  plocaml_reset_error_state();

  procTup = SearchSysCache1(PROCOID, ObjectIdGetDatum(oid));
  if (!HeapTupleIsValid(procTup)) {
    elog(ERROR, "cache lookup failed for function %u", oid);
  }

  bool name_isnull;
  Datum proname_datum =
      SysCacheGetAttr(PROCOID, procTup, Anum_pg_proc_proname, &name_isnull);
  func_name = NameStr(*DatumGetName(proname_datum));

  prosrc = SysCacheGetAttr(PROCOID, procTup, Anum_pg_proc_prosrc, &isnull);
  if (isnull) {
    ReleaseSysCache(procTup);
    elog(ERROR, "null prosrc for function %u", oid);
  }

  user_sql_code = TextDatumGetCString(prosrc);
  ReleaseSysCache(procTup);
  prosrc = (Datum)0;

  args_arr = plocaml_build_args(fcinfo);

  const value *execute_fn = caml_named_value("plocaml_execute");
  if (!execute_fn) {
    ereport(ERROR,
            (errcode(ERRCODE_INTERNAL_ERROR), errmsg("PL/OCaml engine error"),
             errdetail("Execute function not found.")));
  }

  value callback_args[] = {Val_int(oid), caml_copy_string(func_name),
                           caml_copy_string(user_sql_code), args_arr};
  res = caml_callbackN_exn(*execute_fn, 4, callback_args);

  if (Is_exception_result(res) ||
      (Is_block(res) && Tag_val(res) != RESULT_TAG_OK)) {
    plocaml_handle_error(res);
  }

  value datum_val = Field(res, 0);

  // If it's a procedure (void return type), enforce returning Null
  if (get_fn_expr_rettype(fcinfo->flinfo) == VOIDOID) {
    if (!Is_long(datum_val)) {
      ereport(ERROR, (errcode(ERRCODE_DATATYPE_MISMATCH),
                      errmsg("PL/OCaml function with return type \"void\" "
                             "did not return Null")));
    }
    fcinfo->isnull = false;
    return (Datum)0;
  }

  if (fcinfo->flinfo->fn_retset) {
    return plocaml_handle_setof(fcinfo, datum_val);
  }

  bool ret_isnull;
  Datum return_datum = plocaml_convert_datum(fcinfo, datum_val, &ret_isnull);
  fcinfo->isnull = ret_isnull;
  return return_datum;
}
