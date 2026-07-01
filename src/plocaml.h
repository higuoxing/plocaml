#ifndef PLOCAML_H
#define PLOCAML_H

// clang-format off
#include <postgres.h>
// clang-format on

#include <caml/mlvalues.h>

#define DATUM_TAG_INT 0
#define DATUM_TAG_FLOAT 1
#define DATUM_TAG_STRING 2
#define DATUM_TAG_BOOL 3
#define DATUM_TAG_ARRAY 4

#define RESULT_TAG_OK 0
#define RESULT_TAG_SYNTAX_ERROR 1
#define RESULT_TAG_RUNTIME_ERROR 2

value make_ocaml_datum(Oid type_oid, Datum val, bool isnull);

#endif
