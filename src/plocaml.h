#ifndef PLOCAML_H
#define PLOCAML_H

// clang-format off
#include <postgres.h>
// clang-format on

#include <access/htup.h>
#include <access/tupdesc.h>
#include <fmgr.h>

#include <caml/mlvalues.h>

#define DATUM_TAG_INT 0
#define DATUM_TAG_FLOAT 1
#define DATUM_TAG_STRING 2
#define DATUM_TAG_BOOL 3
#define DATUM_TAG_ARRAY 4
#define DATUM_TAG_RECORD 5

#define RESULT_TAG_OK 0
#define RESULT_TAG_SYNTAX_ERROR 1
#define RESULT_TAG_RUNTIME_ERROR 2

/* Value marshalling (typeio.c). */
value make_ocaml_datum(Oid type_oid, Datum val, bool isnull);
value plocaml_build_args(FunctionCallInfo fcinfo);
Datum plocaml_convert_datum(FunctionCallInfo fcinfo, value datum_val,
                            bool *isnull);
HeapTuple plocaml_composite_to_heap_tuple(value composite, TupleDesc tupdesc);

/*
 * Capture the in-flight PostgreSQL error (inside a PG_CATCH) and mark it
 * pending so the PL/OCaml call boundary re-throws it with all fields intact.
 * Returns the error message. Defined in stub.c.
 */
const char *plocaml_stash_pending_error(void);

#endif
