// clang-format off
#include <postgres.h>
// clang-format on

#include <access/htup_details.h>
#include <catalog/pg_proc.h>
#include <catalog/pg_type.h>
#include <commands/event_trigger.h>
#include <commands/extension.h>
#include <commands/trigger.h>
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
#include <utils/typcache.h>

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
      sqlstate =
          MAKE_SQLSTATE(sqlstate_str[0], sqlstate_str[1], sqlstate_str[2],
                        sqlstate_str[3], sqlstate_str[4]);
    }

    ereport(
        elevel,
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
         (table_name) ? err_generic_string(PG_DIAG_TABLE_NAME, table_name) : 0,
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
    value boot_val = Val_unit, stdlib_val = Val_unit;
    Begin_roots2(boot_val, stdlib_val);
    boot_val = caml_copy_string(plocaml_bootstrap_code);
    stdlib_val =
        caml_copy_string(plocaml_stdlib_path ? plocaml_stdlib_path : "");
    caml_callback2(*init_top_level_fn, boot_val, stdlib_val);
    End_roots();
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

  args_arr = caml_alloc(0, 0);        // Empty array
  value arg_names = caml_alloc(0, 0); // No named parameters in a DO block

  const value *execute_fn = caml_named_value("plocaml_execute");
  if (!execute_fn) {
    ereport(ERROR,
            (errcode(ERRCODE_INTERNAL_ERROR), errmsg("PL/OCaml engine error"),
             errdetail("Execute function not found.")));
  }

  value callback_args[] = {Val_int(oid),
                           caml_copy_string(func_name),
                           caml_copy_string(user_sql_code),
                           arg_names,
                           args_arr,
                           Val_int(0)};
  res = caml_callbackN_exn(*execute_fn, 6, callback_args);

  if (Is_exception_result(res) ||
      (Is_block(res) && Tag_val(res) != RESULT_TAG_OK)) {
    plocaml_handle_error(res);
  }

  PG_RETURN_VOID();
}

/* Build a string array of the input-argument names (aligned with the input
   args, "" for unnamed ones), so the OCaml wrapper can bind each as a local. */
static value plocaml_build_arg_names(char **argnames, char *argmodes,
                                     int total_args, int nargs) {
  CAMLparam0();
  CAMLlocal1(names_arr);
  names_arr = caml_alloc(nargs, 0);
  int idx = 0;
  for (int i = 0; i < total_args && idx < nargs; i++) {
    char mode = argmodes ? argmodes[i] : PROARGMODE_IN;
    if (mode == PROARGMODE_IN || mode == PROARGMODE_INOUT ||
        mode == PROARGMODE_VARIADIC) {
      const char *nm = (argnames && argnames[i]) ? argnames[i] : "";
      Store_field(names_arr, idx, caml_copy_string(nm));
      idx++;
    }
  }
  for (; idx < nargs; idx++)
    Store_field(names_arr, idx, caml_copy_string(""));
  CAMLreturn(names_arr);
}

/* Convert a composite (row) Datum into a PL.Record: a (column-name, value)
   association list in column order, giving field-by-name access like
   PL/Python's dict (and matching the shape of SPI result rows). */
static Datum plocaml_handle_setof(FunctionCallInfo fcinfo, value datum_val) {
  ReturnSetInfo *rsinfo = (ReturnSetInfo *)fcinfo->resultinfo;
  if (rsinfo == NULL || !IsA(rsinfo, ReturnSetInfo) ||
      (rsinfo->allowedModes & SFRM_Materialize) == 0)
    ereport(ERROR, (errcode(ERRCODE_FEATURE_NOT_SUPPORTED),
                    errmsg("set-valued function called in context that cannot "
                           "accept a set")));

  if (get_call_result_type(fcinfo, NULL, NULL) == TYPEFUNC_COMPOSITE) {
    InitMaterializedSRF(fcinfo, 0);
  } else {
    InitMaterializedSRF(fcinfo, MAT_SRF_USE_EXPECTED_DESC);
  }

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
      if (Is_long(row_val) || (Tag_val(row_val) != DATUM_TAG_ARRAY &&
                               Tag_val(row_val) != DATUM_TAG_RECORD)) {
        ereport(ERROR,
                (errcode(ERRCODE_DATATYPE_MISMATCH),
                 errmsg("SETOF record must return an array of PL.Array or "
                        "PL.Record rows")));
      }
      HeapTuple tuple = plocaml_composite_to_heap_tuple(row_val, tupdesc);
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

static value build_trigger_data(FunctionCallInfo fcinfo);

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

  value td_val = build_trigger_data(fcinfo);

  procTup = SearchSysCache1(PROCOID, ObjectIdGetDatum(oid));
  if (!HeapTupleIsValid(procTup)) {
    elog(ERROR, "cache lookup failed for function %u", oid);
  }

  bool name_isnull;
  Datum proname_datum =
      SysCacheGetAttr(PROCOID, procTup, Anum_pg_proc_proname, &name_isnull);
  func_name = NameStr(*DatumGetName(proname_datum));

  /* Fetch argument names/modes (palloc'd copies survive ReleaseSysCache) so the
     wrapper can expose named parameters as locals. */
  Oid *argtypes;
  char **argnames;
  char *argmodes;
  int total_args = get_func_arg_info(procTup, &argtypes, &argnames, &argmodes);
  (void)argtypes;

  prosrc = SysCacheGetAttr(PROCOID, procTup, Anum_pg_proc_prosrc, &isnull);
  if (isnull) {
    ReleaseSysCache(procTup);
    elog(ERROR, "null prosrc for function %u", oid);
  }

  user_sql_code = TextDatumGetCString(prosrc);
  ReleaseSysCache(procTup);
  prosrc = (Datum)0;

  args_arr = plocaml_build_args(fcinfo);
  value arg_names =
      plocaml_build_arg_names(argnames, argmodes, total_args, PG_NARGS());

  const value *execute_fn = caml_named_value("plocaml_execute");
  if (!execute_fn) {
    ereport(ERROR,
            (errcode(ERRCODE_INTERNAL_ERROR), errmsg("PL/OCaml engine error"),
             errdetail("Execute function not found.")));
  }

  value callback_args[] = {Val_int(oid),
                           caml_copy_string(func_name),
                           caml_copy_string(user_sql_code),
                           arg_names,
                           args_arr,
                           td_val};
  res = caml_callbackN_exn(*execute_fn, 6, callback_args);

  if (Is_exception_result(res) ||
      (Is_block(res) && Tag_val(res) != RESULT_TAG_OK)) {
    plocaml_handle_error(res);
  }

  value datum_val = Field(res, 0);

  if (CALLED_AS_TRIGGER(fcinfo)) {
    TriggerData *trigdata = (TriggerData *)fcinfo->context;

    /* Null return suppresses the row. */
    if (Is_long(datum_val))
      return PointerGetDatum(NULL);

    /* A Record becomes the (possibly modified) row to use. */
    if (Tag_val(datum_val) == DATUM_TAG_RECORD) {
      TupleDesc tupdesc = RelationGetDescr(trigdata->tg_relation);
      return PointerGetDatum(
          plocaml_composite_to_heap_tuple(datum_val, tupdesc));
    }

    /* A string command: "SKIP" suppresses the row, "OK" proceeds unchanged. */
    if (Tag_val(datum_val) == DATUM_TAG_STRING) {
      const char *ret_str = String_val(Field(datum_val, 0));
      if (strcmp(ret_str, "SKIP") == 0)
        return PointerGetDatum(NULL);
      if (strcmp(ret_str, "OK") == 0)
        return PointerGetDatum(TRIGGER_FIRED_BY_UPDATE(trigdata->tg_event)
                                   ? trigdata->tg_newtuple
                                   : trigdata->tg_trigtuple);
    }

    ereport(ERROR, (errcode(ERRCODE_DATATYPE_MISMATCH),
                    errmsg("trigger function must return PL.Null, PL.String "
                           "\"SKIP\", PL.String \"OK\", or a PL.Record")));
  }

  if (fcinfo->context != NULL && IsA(fcinfo->context, EventTriggerData)) {
    return (Datum)0;
  }

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

CAMLprim value plocaml_quote_literal(value str_val) {
  CAMLparam1(str_val);
  CAMLlocal1(res);
  char *quoted;

  quoted = quote_literal_cstr(String_val(str_val));
  res = caml_copy_string(quoted);
  pfree(quoted);

  CAMLreturn(res);
}

CAMLprim value plocaml_quote_nullable(value str_opt_val) {
  CAMLparam1(str_opt_val);
  CAMLlocal1(res);
  char *quoted;

  if (str_opt_val == Val_int(0)) { /* None */
    res = caml_copy_string("NULL");
  } else {
    quoted = quote_literal_cstr(String_val(Field(str_opt_val, 0)));
    res = caml_copy_string(quoted);
    pfree(quoted);
  }

  CAMLreturn(res);
}

CAMLprim value plocaml_quote_ident(value str_val) {
  CAMLparam1(str_val);
  CAMLlocal1(res);
  const char *quoted;

  quoted = quote_identifier(String_val(str_val));
  res = caml_copy_string(quoted);

  CAMLreturn(res);
}

#define ADD_FIELD(name, val)                                                   \
  do {                                                                         \
    value pair = caml_alloc_tuple(2);                                          \
    Store_field(pair, 0, caml_copy_string(name));                              \
    Store_field(pair, 1, val);                                                 \
    value cons = caml_alloc(2, 0);                                             \
    Store_field(cons, 0, pair);                                                \
    Store_field(cons, 1, td_list);                                             \
    td_list = cons;                                                            \
  } while (0)

#define ADD_STRING_FIELD(name, str)                                            \
  do {                                                                         \
    value s = caml_alloc(1, DATUM_TAG_STRING);                                 \
    Store_field(s, 0, caml_copy_string(str));                                  \
    ADD_FIELD(name, s);                                                        \
  } while (0)

static value build_trigger_data(FunctionCallInfo fcinfo) {
  CAMLparam0();
  CAMLlocal4(td_val, new_row, old_row, args_list);

  if (CALLED_AS_TRIGGER(fcinfo)) {
    TriggerData *trigdata = (TriggerData *)fcinfo->context;
    TriggerEvent event = trigdata->tg_event;
    Trigger *trigger = trigdata->tg_trigger;
    Relation rel = trigdata->tg_relation;

    char *event_str = "";
    if (TRIGGER_FIRED_BY_INSERT(event))
      event_str = "INSERT";
    else if (TRIGGER_FIRED_BY_UPDATE(event))
      event_str = "UPDATE";
    else if (TRIGGER_FIRED_BY_DELETE(event))
      event_str = "DELETE";
    else if (TRIGGER_FIRED_BY_TRUNCATE(event))
      event_str = "TRUNCATE";

    char *when_str = "";
    if (TRIGGER_FIRED_BEFORE(event))
      when_str = "BEFORE";
    else if (TRIGGER_FIRED_AFTER(event))
      when_str = "AFTER";
    else if (TRIGGER_FIRED_INSTEAD(event))
      when_str = "INSTEAD OF";

    char *level_str = "";
    if (TRIGGER_FIRED_FOR_ROW(event))
      level_str = "ROW";
    else if (TRIGGER_FIRED_FOR_STATEMENT(event))
      level_str = "STATEMENT";

    new_row = Val_int(0); // Null
    old_row = Val_int(0); // Null

    if (TRIGGER_FIRED_FOR_ROW(event)) {
      if (TRIGGER_FIRED_BY_INSERT(event)) {
        new_row = plocaml_heap_tuple_to_record(trigdata->tg_trigtuple,
                                               RelationGetDescr(rel));
      } else if (TRIGGER_FIRED_BY_UPDATE(event)) {
        old_row = plocaml_heap_tuple_to_record(trigdata->tg_trigtuple,
                                               RelationGetDescr(rel));
        new_row = plocaml_heap_tuple_to_record(trigdata->tg_newtuple,
                                               RelationGetDescr(rel));
      } else if (TRIGGER_FIRED_BY_DELETE(event)) {
        old_row = plocaml_heap_tuple_to_record(trigdata->tg_trigtuple,
                                               RelationGetDescr(rel));
      }
    }

    value ocaml_arr = caml_alloc(trigger->tgnargs, 0);
    for (int i = 0; i < trigger->tgnargs; i++) {
      value s = caml_alloc(1, DATUM_TAG_STRING);
      Store_field(s, 0, caml_copy_string(trigger->tgargs[i]));
      Store_field(ocaml_arr, i, s);
    }
    args_list = caml_alloc(1, DATUM_TAG_ARRAY);
    Store_field(args_list, 0, ocaml_arr);

    value td_list = Val_int(0); // []

    ADD_FIELD("args", args_list);

    char relid_str[32];
    snprintf(relid_str, sizeof(relid_str), "%u", RelationGetRelid(rel));
    ADD_STRING_FIELD("relid", relid_str);

    ADD_STRING_FIELD("table_schema", SPI_getnspname(rel));
    ADD_STRING_FIELD("table_name", SPI_getrelname(rel));
    ADD_STRING_FIELD("name", trigger->tgname);
    ADD_FIELD("old", old_row);
    ADD_FIELD("new", new_row);
    ADD_STRING_FIELD("level", level_str);
    ADD_STRING_FIELD("when", when_str);
    ADD_STRING_FIELD("event", event_str);

    td_val = caml_alloc(1, DATUM_TAG_RECORD);
    Store_field(td_val, 0, td_list);

    CAMLreturn(td_val);
  } else if (CALLED_AS_EVENT_TRIGGER(fcinfo)) {
    EventTriggerData *trigdata = (EventTriggerData *)fcinfo->context;

    value td_list = Val_int(0);

    ADD_STRING_FIELD("tag", GetCommandTagName(trigdata->tag));
    ADD_STRING_FIELD("event", trigdata->event);

    td_val = caml_alloc(1, DATUM_TAG_RECORD);
    Store_field(td_val, 0, td_list);

    CAMLreturn(td_val);
  }

  CAMLreturn(Val_int(0)); // Null
}
