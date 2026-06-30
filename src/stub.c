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

#include <caml/mlvalues.h>
#include <caml/callback.h>
#include <caml/memory.h>
#include <utils/guc.h>
#include <caml/alloc.h>
#include <caml/fail.h>

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

CAMLprim value plocaml_spi_execute(value query_val) {
  CAMLparam1(query_val);
  const char *query = String_val(query_val);
  
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
  SPI_finish();
  CAMLreturn(Val_int(rows));
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
