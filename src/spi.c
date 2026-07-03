// clang-format off
#include "plocaml.h"
// clang-format on

#include <caml/alloc.h>
#include <caml/callback.h>
#include <caml/custom.h>
#include <caml/fail.h>
#include <caml/memory.h>
#include <access/xact.h>
#include <executor/spi.h>
#include <parser/parse_type.h>
#include <utils/builtins.h>
#include <utils/elog.h>
#include <utils/memutils.h>
#include <utils/portal.h>
#include <utils/resowner.h>

#define Custom_plan_val(v) (*((SPIPlanPtr *)Data_custom_val(v)))

/* A cursor value holds the portal *name* (in TopMemoryContext), not the raw
   Portal pointer. Portals opened inside a subtransaction are dropped by
   PostgreSQL if that subtransaction rolls back, which would leave a dangling
   pointer; resolving the name with GetPortalByName on each use lets us detect
   that case and fail cleanly instead. NULL means the cursor has been closed. */
#define Custom_cursor_name(v) (*((char **)Data_custom_val(v)))

static void finalize_spi_cursor(value v) {
  char *portalname = Custom_cursor_name(v);
  if (portalname != NULL) {
    Portal portal = GetPortalByName(portalname);
    if (PortalIsValid(portal)) {
      SPI_cursor_close(portal);
    }
    pfree(portalname);
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

/*
 * Implicit per-SPI-call subtransactions, mirroring PL/Python. Running each SPI
 * call inside its own internal subtransaction means a caught error (an OCaml
 * [Failure]) leaves the surrounding transaction usable rather than aborted.
 * [oldcontext] is the caller's memory context (captured before SPI_connect) and
 * [oldowner] the caller's resource owner (captured before the subtransaction).
 */
static void plocaml_subxact_begin(MemoryContext oldcontext) {
  BeginInternalSubTransaction(NULL);
  /* Run the SPI call in the caller's context, not the subtransaction's. */
  MemoryContextSwitchTo(oldcontext);
}

/* Commit the current subtransaction. Called on the non-throwing path of an SPI
   call — including when the call returned an error *status* rather than raising
   — since in that case the subtransaction did nothing that needs rolling back. */
static void plocaml_subxact_commit(MemoryContext oldcontext,
                                   ResourceOwner oldowner) {
  ReleaseCurrentSubTransaction();
  MemoryContextSwitchTo(oldcontext);
  CurrentResourceOwner = oldowner;
}

/* Roll back the current subtransaction and restore the caller's context/owner. */
static void plocaml_subxact_rollback(MemoryContext oldcontext,
                                     ResourceOwner oldowner) {
  RollbackAndReleaseCurrentSubTransaction();
  MemoryContextSwitchTo(oldcontext);
  CurrentResourceOwner = oldowner;
}

/* Abort the current subtransaction from within a PG_CATCH. Stashes the in-flight
   error (so the call boundary can re-raise it with all fields) and returns its
   message. Must copy/flush the error before rolling back, as the rollback tears
   down the error context. */
static const char *plocaml_subxact_abort(MemoryContext oldcontext,
                                         ResourceOwner oldowner) {
  const char *errmsg;
  MemoryContextSwitchTo(oldcontext);
  errmsg = plocaml_stash_pending_error();
  plocaml_subxact_rollback(oldcontext, oldowner);
  return errmsg;
}

/*
 * Explicit subtransaction: run an OCaml thunk inside an internal
 * subtransaction, committing if it returns normally and rolling back (then
 * re-raising) if it raises. This is the [PL.subtransaction] the user calls
 * directly; any PostgreSQL error raised inside was already caught and flushed
 * by the implicit per-call subtransaction above, so here we only need to
 * unwind the OCaml exception.
 */
CAMLprim value plocaml_subtransaction(value thunk) {
  CAMLparam1(thunk);
  CAMLlocal1(res);

  MemoryContext oldcontext = CurrentMemoryContext;
  ResourceOwner oldowner = CurrentResourceOwner;

  plocaml_subxact_begin(oldcontext);

  res = caml_callback_exn(thunk, Val_unit);

  if (Is_exception_result(res)) {
    /* Extract before the rollback: nothing between here and caml_raise
       allocates, so res never needs to survive a GC as a raw exception result. */
    res = Extract_exception(res);
    plocaml_subxact_rollback(oldcontext, oldowner);
    caml_raise(res);
  }

  plocaml_subxact_commit(oldcontext, oldowner);
  CAMLreturn(res);
}

/* Resolve a live portal from a cursor's stored name. The portal is dropped by
   PostgreSQL if the subtransaction it was opened in rolled back, so a lookup
   miss means the cursor is being used after such an abort; raise [errmsg].
   Never returns an invalid portal. */
static Portal plocaml_cursor_portal(const char *portalname,
                                    const char *errmsg) {
  Portal portal = GetPortalByName(portalname);
  if (!PortalIsValid(portal)) {
    caml_failwith(errmsg);
  }
  return portal;
}

static value build_spi_result(int status, int nrows) {
  CAMLparam0();
  CAMLlocal2(res, rows_arr);

  rows_arr = caml_alloc(nrows, 0);

  if (SPI_tuptable != NULL) {
    TupleDesc tupdesc = SPI_tuptable->tupdesc;
    for (int i = 0; i < nrows; i++) {
      HeapTuple tuple = SPI_tuptable->vals[i];
      /* spi_result stores each row as a bare (name, value) association list, so
         unwrap the Record produced by the shared helper. */
      value record_val = plocaml_heap_tuple_to_record(tuple, tupdesc);
      Store_field(rows_arr, i, Field(record_val, 0));
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
  ResourceOwner oldowner = CurrentResourceOwner;
  if (SPI_connect() != SPI_OK_CONNECT) {
    caml_failwith("PL/OCaml: could not connect to SPI manager");
  }

  volatile bool failed = false;
  const char *errmsg = NULL;
  SPIPlanPtr plan = NULL;

  int nargs = Wosize_val(argtypes_val);
  Oid *argtypes = palloc(nargs * sizeof(Oid));

  plocaml_subxact_begin(caller_context);
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
    plocaml_subxact_commit(caller_context, oldowner);
  }
  PG_CATCH();
  {
    errmsg = plocaml_subxact_abort(caller_context, oldowner);
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
  ResourceOwner oldowner = CurrentResourceOwner;
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

  plocaml_subxact_begin(caller_context);
  PG_TRY();
  {
    res = SPI_execute_plan(plan, Values, Nulls, false, 0);
    if (res < 0) {
      failed = true;
    }
    plocaml_subxact_commit(caller_context, oldowner);
  }
  PG_CATCH();
  {
    errmsg = plocaml_subxact_abort(caller_context, oldowner);
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
  ResourceOwner oldowner = CurrentResourceOwner;
  if (SPI_connect() != SPI_OK_CONNECT) {
    caml_failwith("PL/OCaml: could not connect to SPI manager");
  }

  volatile bool failed = false;
  const char *errmsg = NULL;
  char *portalname = NULL;
  SPIPlanPtr plan = NULL;

  plocaml_subxact_begin(caller_context);
  PG_TRY();
  {
    plan = SPI_prepare(query, 0, NULL);
    if (plan != NULL) {
      SPI_keepplan(plan);
      Portal cursor = SPI_cursor_open(NULL, plan, NULL, NULL, false);
      SPI_freeplan(plan);
      if (cursor != NULL) {
        portalname = MemoryContextStrdup(TopMemoryContext, cursor->name);
      } else {
        failed = true;
      }
    } else {
      failed = true;
    }
    plocaml_subxact_commit(caller_context, oldowner);
  }
  PG_CATCH();
  {
    errmsg = plocaml_subxact_abort(caller_context, oldowner);
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

  cursor_val = caml_alloc_custom(&spi_cursor_ops, sizeof(char *), 0, 1);
  Custom_cursor_name(cursor_val) = portalname;

  SPI_finish();
  CAMLreturn(cursor_val);
}

CAMLprim value plocaml_spi_cursor_plan(value plan_val, value args_val) {
  CAMLparam2(plan_val, args_val);
  CAMLlocal1(cursor_val);

  MemoryContext caller_context = CurrentMemoryContext;
  ResourceOwner oldowner = CurrentResourceOwner;
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
  char *portalname = NULL;

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

  plocaml_subxact_begin(caller_context);
  PG_TRY();
  {
    Portal cursor = SPI_cursor_open(NULL, plan, Values, Nulls, false);
    if (cursor != NULL) {
      portalname = MemoryContextStrdup(TopMemoryContext, cursor->name);
    } else {
      failed = true;
    }
    plocaml_subxact_commit(caller_context, oldowner);
  }
  PG_CATCH();
  {
    errmsg = plocaml_subxact_abort(caller_context, oldowner);
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

  cursor_val = caml_alloc_custom(&spi_cursor_ops, sizeof(char *), 0, 1);
  Custom_cursor_name(cursor_val) = portalname;

  SPI_finish();
  CAMLreturn(cursor_val);
}

CAMLprim value plocaml_spi_fetch(value cursor_val, value count_val) {
  CAMLparam2(cursor_val, count_val);

  MemoryContext caller_context = CurrentMemoryContext;
  ResourceOwner oldowner = CurrentResourceOwner;

  char *portalname = Custom_cursor_name(cursor_val);
  if (portalname == NULL) {
    caml_failwith("PL/OCaml: attempt to fetch from a closed cursor");
  }
  Portal cursor = plocaml_cursor_portal(
      portalname, "PL/OCaml: iterating a cursor in an aborted subtransaction");

  if (SPI_connect() != SPI_OK_CONNECT) {
    caml_failwith("PL/OCaml: could not connect to SPI manager");
  }

  int count = Int_val(count_val);
  int res = 0;
  volatile bool failed = false;
  const char *errmsg = NULL;

  plocaml_subxact_begin(caller_context);
  PG_TRY();
  {
    SPI_cursor_fetch(cursor, true, count);
    res = SPI_processed;
    plocaml_subxact_commit(caller_context, oldowner);
  }
  PG_CATCH();
  {
    errmsg = plocaml_subxact_abort(caller_context, oldowner);
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
  char *portalname = Custom_cursor_name(cursor_val);
  if (portalname != NULL) {
    Portal cursor = plocaml_cursor_portal(
        portalname, "PL/OCaml: closing a cursor in an aborted subtransaction");
    SPI_cursor_close(cursor);
    pfree(portalname);
    Custom_cursor_name(cursor_val) = NULL;
  }
  CAMLreturn(Val_unit);
}

CAMLprim value plocaml_spi_execute_with_args(value query_val, value args_val) {
  CAMLparam2(query_val, args_val);
  const char *query = String_val(query_val);

  MemoryContext caller_context = CurrentMemoryContext;
  ResourceOwner oldowner = CurrentResourceOwner;
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

  plocaml_subxact_begin(caller_context);
  PG_TRY();
  {
    res =
        SPI_execute_with_args(query, nargs, argtypes, Values, Nulls, false, 0);
    if (res < 0) {
      failed = true;
    }
    plocaml_subxact_commit(caller_context, oldowner);
  }
  PG_CATCH();
  {
    errmsg = plocaml_subxact_abort(caller_context, oldowner);
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
  ResourceOwner oldowner = CurrentResourceOwner;
  if (SPI_connect() != SPI_OK_CONNECT) {
    caml_failwith("PL/OCaml: could not connect to SPI manager");
  }

  int res = 0;
  volatile bool failed = false;
  const char *errmsg = NULL;

  plocaml_subxact_begin(caller_context);
  PG_TRY();
  {
    res = SPI_execute(query, false, 0);
    if (res < 0) {
      failed = true;
    }
    plocaml_subxact_commit(caller_context, oldowner);
  }
  PG_CATCH();
  {
    errmsg = plocaml_subxact_abort(caller_context, oldowner);
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
