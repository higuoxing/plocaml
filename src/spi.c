// clang-format off
#include "plocaml.h"
// clang-format on

#include <caml/alloc.h>
#include <caml/custom.h>
#include <caml/fail.h>
#include <caml/memory.h>
#include <executor/spi.h>
#include <parser/parse_type.h>
#include <utils/builtins.h>
#include <utils/elog.h>
#include <utils/memutils.h>

#define Custom_plan_val(v) (*((SPIPlanPtr *)Data_custom_val(v)))
#define Custom_cursor_val(v) (*((Portal *)Data_custom_val(v)))

static void finalize_spi_cursor(value v) {
  Portal cursor = Custom_cursor_val(v);
  if (cursor != NULL) {
    SPI_cursor_close(cursor);
  }
}

static struct custom_operations spi_cursor_ops = {
    "plocaml.spi_cursor",       finalize_spi_cursor,
    custom_compare_default,     custom_hash_default,
    custom_serialize_default,   custom_deserialize_default,
    custom_compare_ext_default, custom_fixed_length_default};

static void finalize_spi_plan(value v) {
  SPIPlanPtr plan = Custom_plan_val(v);
  if (plan != NULL) {
    SPI_freeplan(plan);
  }
}

static struct custom_operations spi_plan_ops = {
    "plocaml.spi_plan",         finalize_spi_plan,
    custom_compare_default,     custom_hash_default,
    custom_serialize_default,   custom_deserialize_default,
    custom_compare_ext_default, custom_fixed_length_default};

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
  const char *errmsg = NULL;
  SPIPlanPtr plan = NULL;

  int nargs = Wosize_val(argtypes_val);
  Oid *argtypes = palloc(nargs * sizeof(Oid));

  PG_TRY();
  {
    for (int i = 0; i < nargs; i++) {
      const char *type_name = String_val(Field(argtypes_val, i));
      Oid type_id;
      int32 typmod;
      parseTypeString(type_name, &type_id, &typmod, NULL);
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
    errmsg = plocaml_stash_pending_error();
    failed = true;
  }
  PG_END_TRY();

  pfree(argtypes);

  if (failed) {
    SPI_finish();
    if (errmsg) {
      caml_failwith(errmsg);
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
  const char *errmsg = NULL;

  Datum *Values = palloc(nargs * sizeof(Datum));
  char *Nulls = palloc(nargs * sizeof(char));

  for (int i = 0; i < nargs; i++) {
    value elem = Field(args_val, i);
    if (Is_long(elem)) {
      Values[i] = (Datum)0;
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
        caml_failwith(
            "PL/OCaml: unsupported argument type for SPI_execute_plan");
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
    errmsg = plocaml_stash_pending_error();
    failed = true;
  }
  PG_END_TRY();

  pfree(Values);
  pfree(Nulls);

  if (failed) {
    SPI_finish();
    if (errmsg) {
      caml_failwith(errmsg);
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
  const char *errmsg = NULL;
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
    errmsg = plocaml_stash_pending_error();
    failed = true;
  }
  PG_END_TRY();

  if (failed) {
    SPI_finish();
    if (errmsg) {
      caml_failwith(errmsg);
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
  const char *errmsg = NULL;
  Portal cursor = NULL;

  Datum *Values = palloc(nargs * sizeof(Datum));
  char *Nulls = palloc(nargs * sizeof(char));

  for (int i = 0; i < nargs; i++) {
    value elem = Field(args_val, i);
    if (Is_long(elem)) {
      Values[i] = (Datum)0;
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
        caml_failwith(
            "PL/OCaml: unsupported argument type for SPI_cursor_plan");
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
    errmsg = plocaml_stash_pending_error();
    failed = true;
  }
  PG_END_TRY();

  pfree(Values);
  pfree(Nulls);

  if (failed) {
    SPI_finish();
    if (errmsg) {
      caml_failwith(errmsg);
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
  const char *errmsg = NULL;

  PG_TRY();
  {
    SPI_cursor_fetch(cursor, true, count);
    res = SPI_processed;
  }
  PG_CATCH();
  {
    MemoryContextSwitchTo(caller_context);
    errmsg = plocaml_stash_pending_error();
    failed = true;
  }
  PG_END_TRY();

  if (failed) {
    SPI_finish();
    if (errmsg) {
      caml_failwith(errmsg);
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
  const char *errmsg = NULL;

  int nargs = Wosize_val(args_val);
  Oid *argtypes = palloc(nargs * sizeof(Oid));
  Datum *Values = palloc(nargs * sizeof(Datum));
  char *Nulls = palloc(nargs * sizeof(char));

  for (int i = 0; i < nargs; i++) {
    value elem = Field(args_val, i);
    if (Is_long(elem)) {
      argtypes[i] = TEXTOID; // Default to text for nulls
      Values[i] = (Datum)0;
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
    res =
        SPI_execute_with_args(query, nargs, argtypes, Values, Nulls, false, 0);
    if (res < 0) {
      failed = true;
    }
  }
  PG_CATCH();
  {
    MemoryContextSwitchTo(caller_context);
    errmsg = plocaml_stash_pending_error();
    failed = true;
  }
  PG_END_TRY();

  pfree(argtypes);
  pfree(Values);
  pfree(Nulls);

  if (failed) {
    SPI_finish();
    if (errmsg) {
      caml_failwith(errmsg);
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
  const char *errmsg = NULL;

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
    errmsg = plocaml_stash_pending_error();
    failed = true;
  }
  PG_END_TRY();

  if (failed) {
    SPI_finish();
    if (errmsg) {
      caml_failwith(errmsg);
    } else {
      caml_failwith("PL/OCaml SPI_execute failed");
    }
  }

  int rows = SPI_processed;
  value result = build_spi_result(res, rows);
  SPI_finish();
  CAMLreturn(result);
}
