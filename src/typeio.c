// clang-format off
#include <postgres.h>
// clang-format on

#include <access/htup_details.h>
#include <catalog/pg_type.h>
#include <fmgr.h>
#include <funcapi.h>
#include <utils/builtins.h>
#include <utils/elog.h>
#include <utils/lsyscache.h>
#include <utils/typcache.h>

#include <caml/alloc.h>
#include <caml/fail.h>
#include <caml/memory.h>
#include <caml/mlvalues.h>

#include "plocaml.h"

/*
 * Value marshalling between PostgreSQL Datums and the OCaml `datum` variant.
 * Input:  make_ocaml_datum / plocaml_build_args  (Datum -> datum)
 * Output: plocaml_convert_datum / *_heap_tuple    (datum -> Datum)
 */

static value plocaml_composite_to_datum(Datum arg);

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
  } else if (type_is_rowtype(type_oid)) {
    v = plocaml_composite_to_datum(val);
  } else {
    // Default to string for everything else (numeric, enum, uuid, json, ...)
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

/* Convert a composite (row) Datum into a PL.Record: a (column-name, value)
   association list in column order, giving field-by-name access like
   PL/Python's dict (and matching the shape of SPI result rows). */
value plocaml_composite_to_datum(Datum arg) {
  CAMLparam0();
  CAMLlocal5(v, lst, pair, name, fval_v);
  CAMLlocal1(node);

  HeapTupleHeader th = DatumGetHeapTupleHeader(arg);
  Oid tup_type = HeapTupleHeaderGetTypeId(th);
  int32 tup_typmod = HeapTupleHeaderGetTypMod(th);
  TupleDesc tupdesc = lookup_rowtype_tupdesc(tup_type, tup_typmod);

  HeapTupleData tuple;
  tuple.t_len = HeapTupleHeaderGetDatumLength(th);
  ItemPointerSetInvalid(&(tuple.t_self));
  tuple.t_tableOid = InvalidOid;
  tuple.t_data = th;

  /* Build the list backwards so it ends up in column order. */
  lst = Val_int(0); // []
  for (int i = tupdesc->natts - 1; i >= 0; i--) {
    Form_pg_attribute att = TupleDescAttr(tupdesc, i);
    if (att->attisdropped)
      continue;

    bool isnull;
    Datum fval = heap_getattr(&tuple, i + 1, tupdesc, &isnull);
    name = caml_copy_string(NameStr(att->attname));
    fval_v = make_ocaml_datum(att->atttypid, fval, isnull);

    pair = caml_alloc(2, 0);
    Store_field(pair, 0, name);
    Store_field(pair, 1, fval_v);

    node = caml_alloc(2, 0);
    Store_field(node, 0, pair);
    Store_field(node, 1, lst);
    lst = node;
  }
  ReleaseTupleDesc(tupdesc);

  v = caml_alloc(1, DATUM_TAG_RECORD);
  Store_field(v, 0, lst);
  CAMLreturn(v);
}

value plocaml_build_args(FunctionCallInfo fcinfo) {
  CAMLparam0();
  CAMLlocal2(args_arr, v);
  int nargs = PG_NARGS();
  args_arr = caml_alloc(nargs, 0); // 0 tag for Tuple/Array
  for (int i = 0; i < nargs; i++) {
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
        Store_field(v, 0, caml_copy_double(DatumGetFloat8(arg)));
      } else if (type_oid == TEXTOID || type_oid == VARCHAROID) {
        v = caml_alloc(1, DATUM_TAG_STRING);
        char *str = TextDatumGetCString(arg);
        Store_field(v, 0, caml_copy_string(str));
        pfree(str);
      } else if (type_oid == BOOLOID) {
        v = caml_alloc(1, DATUM_TAG_BOOL);
        Store_field(v, 0, Val_int(DatumGetBool(arg) ? 1 : 0));
      } else if (type_is_rowtype(type_oid)) {
        v = plocaml_composite_to_datum(arg);
      } else {
        // Any other type (numeric, enum, uuid, domain, ...) as its text form.
        v = make_ocaml_datum(type_oid, arg, false);
      }
    }
    Store_field(args_arr, i, v);
  }
  CAMLreturn(args_arr);
}

/* Convert one datum value into a column value (Datum + isnull flag). */
static void plocaml_elem_to_datum(value elem, Datum *value_out, bool *isnull) {
  if (Is_long(elem)) {
    *isnull = true;
    *value_out = (Datum)0;
    return;
  }
  *isnull = false;
  switch (Tag_val(elem)) {
  case DATUM_TAG_INT:
    *value_out = Int32GetDatum(Int_val(Field(elem, 0)));
    break;
  case DATUM_TAG_FLOAT:
    *value_out = Float8GetDatum(Double_val(Field(elem, 0)));
    break;
  case DATUM_TAG_STRING:
    *value_out = CStringGetTextDatum(String_val(Field(elem, 0)));
    break;
  case DATUM_TAG_BOOL:
    *value_out = BoolGetDatum(Int_val(Field(elem, 0)) != 0);
    break;
  default:
    elog(ERROR, "PL/OCaml engine error: Unexpected datum variant tag in "
                "composite field.");
  }
}

/* Build a HeapTuple from a PL.Array of field values, positionally. */
static HeapTuple plocaml_build_heap_tuple(value arr, TupleDesc tupdesc) {
  int arr_len = Wosize_val(arr);
  if (arr_len != tupdesc->natts) {
    ereport(ERROR, (errcode(ERRCODE_DATATYPE_MISMATCH),
                    errmsg("length of returned sequence did not match number "
                           "of columns in row")));
  }

  Datum *values = palloc(tupdesc->natts * sizeof(Datum));
  bool *nulls = palloc(tupdesc->natts * sizeof(bool));

  for (int i = 0; i < tupdesc->natts; i++)
    plocaml_elem_to_datum(Field(arr, i), &values[i], &nulls[i]);

  HeapTuple tuple = heap_form_tuple(tupdesc, values, nulls);
  pfree(values);
  pfree(nulls);
  return tuple;
}

/* Build a HeapTuple from a PL.Record ((column-name, value) list), matching each
   result column by name -- the return-side counterpart of a composite argument
   arriving as a Record. */
static HeapTuple plocaml_build_heap_tuple_from_record(value rec_list,
                                                      TupleDesc tupdesc) {
  Datum *values = palloc(tupdesc->natts * sizeof(Datum));
  bool *nulls = palloc(tupdesc->natts * sizeof(bool));

  for (int i = 0; i < tupdesc->natts; i++) {
    Form_pg_attribute att = TupleDescAttr(tupdesc, i);
    if (att->attisdropped) {
      nulls[i] = true;
      values[i] = (Datum)0;
      continue;
    }
    const char *colname = NameStr(att->attname);
    bool found = false;
    for (value node = rec_list; Is_block(node); node = Field(node, 1)) {
      value pair = Field(node, 0);
      if (strcmp(String_val(Field(pair, 0)), colname) == 0) {
        plocaml_elem_to_datum(Field(pair, 1), &values[i], &nulls[i]);
        found = true;
        break;
      }
    }
    if (!found)
      ereport(ERROR,
              (errcode(ERRCODE_DATATYPE_MISMATCH),
               errmsg("record result is missing column \"%s\"", colname)));
  }

  HeapTuple tuple = heap_form_tuple(tupdesc, values, nulls);
  pfree(values);
  pfree(nulls);
  return tuple;
}

/* Build a HeapTuple from a composite datum, which may be a PL.Array
   (positional) or a PL.Record (by column name). */
HeapTuple plocaml_composite_to_heap_tuple(value composite, TupleDesc tupdesc) {
  if (Is_block(composite) && Tag_val(composite) == DATUM_TAG_ARRAY)
    return plocaml_build_heap_tuple(Field(composite, 0), tupdesc);
  if (Is_block(composite) && Tag_val(composite) == DATUM_TAG_RECORD)
    return plocaml_build_heap_tuple_from_record(Field(composite, 0), tupdesc);
  ereport(ERROR, (errcode(ERRCODE_DATATYPE_MISMATCH),
                  errmsg("composite result must be a PL.Array or PL.Record")));
  return NULL; /* unreachable */
}

Datum plocaml_convert_datum(FunctionCallInfo fcinfo, value datum_val,
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
    char *str = (char *)String_val(Field(datum_val, 0));
    /* A string returned for a composite result is parsed via the row type's
       input function, mirroring PL/Python's PLyUnicode_ToComposite (e.g.
       returning "(1,foo)" for a composite type). */
    Oid rettype = get_fn_expr_rettype(fcinfo->flinfo);
    if (OidIsValid(rettype) && type_is_rowtype(rettype)) {
      Oid in_fn, ioparam;
      getTypeInputInfo(rettype, &in_fn, &ioparam);
      return OidInputFunctionCall(in_fn, str, ioparam, -1);
    }
    return CStringGetTextDatum(str);
  } else if (d_tag == DATUM_TAG_BOOL) {
    return BoolGetDatum(Int_val(Field(datum_val, 0)) != 0);
  } else if (d_tag == DATUM_TAG_ARRAY || d_tag == DATUM_TAG_RECORD) {
    TupleDesc tupdesc;
    if (get_call_result_type(fcinfo, NULL, &tupdesc) != TYPEFUNC_COMPOSITE) {
      ereport(ERROR, (errcode(ERRCODE_FEATURE_NOT_SUPPORTED),
                      errmsg("function returning record called in context that "
                             "cannot accept type record")));
    }
    BlessTupleDesc(tupdesc);
    HeapTuple tuple = plocaml_composite_to_heap_tuple(datum_val, tupdesc);
    Datum return_datum = heap_copy_tuple_as_datum(tuple, tupdesc);
    heap_freetuple(tuple);
    return return_datum;
  } else {
    elog(ERROR, "PL/OCaml engine error: Unexpected datum variant tag.");
  }
  return (Datum)0;
}

value plocaml_heap_tuple_to_record(HeapTuple tuple, TupleDesc tupdesc) {
  CAMLparam0();
  CAMLlocal4(row_list, pair, col_name, col_val);

  row_list = Val_int(0); // []

  // Build the list backwards so it ends up in the correct order
  for (int j = tupdesc->natts; j > 0; j--) {
    Form_pg_attribute att = TupleDescAttr(tupdesc, j - 1);
    if (att->attisdropped)
      continue;

    bool isnull;
    Datum val = heap_getattr(tuple, j, tupdesc, &isnull);
    Oid type_oid = att->atttypid;
    char *fname = NameStr(att->attname);

    col_name = caml_copy_string(fname);
    col_val = make_ocaml_datum(type_oid, val, isnull);

    pair = caml_alloc(2, 0);
    Store_field(pair, 0, col_name);
    Store_field(pair, 1, col_val);

    value new_node = caml_alloc(2, 0);
    Store_field(new_node, 0, pair);
    Store_field(new_node, 1, row_list);
    row_list = new_node;
  }

  value record_val = caml_alloc(1, DATUM_TAG_RECORD);
  Store_field(record_val, 0, row_list);
  CAMLreturn(record_val);
}
