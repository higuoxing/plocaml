use pgrx::pg_sys;
use pgrx::PgTupleDesc;
use std::ffi::CStr;

const DATUM_TAG_INT: u8 = 0;
const DATUM_TAG_FLOAT: u8 = 1;
const DATUM_TAG_STRING: u8 = 2;
const DATUM_TAG_BOOL: u8 = 3;
#[allow(dead_code)]
const DATUM_TAG_ARRAY: u8 = 4;
const DATUM_TAG_RECORD: u8 = 5;

unsafe fn make_ocaml_null() -> ocaml::Value {
    ocaml::Value::new(ocaml::sys::val_int(0))
}

unsafe fn make_ocaml_int(i: isize) -> ocaml::Value {
    let block = ocaml::sys::caml_alloc(1, DATUM_TAG_INT);
    *ocaml::sys::field(block, 0) = ocaml::sys::val_int(i);
    ocaml::Value::new(block)
}

unsafe fn make_ocaml_float(f: f64) -> ocaml::Value {
    let d = ocaml::sys::caml_copy_double(f);
    let block = ocaml::sys::caml_alloc(1, DATUM_TAG_FLOAT);
    *ocaml::sys::field(block, 0) = d;
    ocaml::Value::new(block)
}

unsafe fn make_ocaml_string(s: &str) -> ocaml::Value {
    let str_val = ocaml::Value::string(s);
    let block = ocaml::sys::caml_alloc(1, DATUM_TAG_STRING);
    *ocaml::sys::field(block, 0) = str_val.raw().0;
    ocaml::Value::new(block)
}

unsafe fn make_ocaml_bool(b: bool) -> ocaml::Value {
    let block = ocaml::sys::caml_alloc(1, DATUM_TAG_BOOL);
    *ocaml::sys::field(block, 0) = ocaml::sys::val_int(if b { 1 } else { 0 });
    ocaml::Value::new(block)
}

unsafe fn composite_to_ocaml_datum(arg: pg_sys::Datum) -> ocaml::Value {
    let th = pg_sys::pg_detoast_datum(arg.cast_mut_ptr()) as pg_sys::HeapTupleHeader;
    let tup_type = pgrx::htup::heap_tuple_header_get_type_id(th);
    let tup_typmod = pgrx::htup::heap_tuple_header_get_typmod(th);
    let tupdesc = pg_sys::lookup_rowtype_tupdesc(tup_type, tup_typmod);

    let mut tuple = pg_sys::HeapTupleData {
        t_len: pgrx::htup::heap_tuple_header_get_datum_length(th) as u32,
        t_self: pg_sys::ItemPointerData::default(),
        t_tableOid: pg_sys::InvalidOid,
        t_data: th,
    };

    let list = heap_tuple_to_row_list(&mut tuple, tupdesc);
    pgrx::tupdesc::release_tupdesc(tupdesc);

    let record_block = ocaml::sys::caml_alloc(1, DATUM_TAG_RECORD);
    *ocaml::sys::field(record_block, 0) = list.raw().0;
    ocaml::Value::new(record_block)
}

unsafe fn make_ocaml_datum(
    type_oid: pg_sys::Oid,
    val: pg_sys::Datum,
    isnull: bool,
) -> ocaml::Value {
    if isnull {
        return make_ocaml_null();
    }

    if type_oid == pg_sys::INT2OID {
        let i = pg_sys::DatumGetInt16(val) as isize;
        make_ocaml_int(i)
    } else if type_oid == pg_sys::INT4OID {
        let i = pg_sys::DatumGetInt32(val) as isize;
        make_ocaml_int(i)
    } else if type_oid == pg_sys::INT8OID {
        let i = pg_sys::DatumGetInt64(val) as isize;
        make_ocaml_int(i)
    } else if type_oid == pg_sys::FLOAT4OID {
        let f = pg_sys::DatumGetFloat4(val) as f64;
        make_ocaml_float(f)
    } else if type_oid == pg_sys::FLOAT8OID {
        let f = pg_sys::DatumGetFloat8(val);
        make_ocaml_float(f)
    } else if type_oid == pg_sys::BOOLOID {
        let b = pg_sys::DatumGetBool(val);
        make_ocaml_bool(b)
    } else if pg_sys::type_is_rowtype(type_oid) {
        composite_to_ocaml_datum(val)
    } else {
        let mut typoutput = pg_sys::InvalidOid;
        let mut typisvarlena = false;
        pg_sys::getTypeOutputInfo(type_oid, &mut typoutput, &mut typisvarlena);
        let cstr_ptr = pg_sys::OidOutputFunctionCall(typoutput, val);
        let s = CStr::from_ptr(cstr_ptr).to_str().unwrap_or("");
        let ocaml_datum = make_ocaml_string(s);
        pg_sys::pfree(cstr_ptr.cast());
        ocaml_datum
    }
}

pub(crate) unsafe fn heap_tuple_to_row_list(
    tuple: pg_sys::HeapTuple,
    tupdesc_ptr: pg_sys::TupleDesc,
) -> ocaml::Value {
    let pg_tupdesc = PgTupleDesc::from_pg_unchecked(tupdesc_ptr);
    let natts = pg_tupdesc.len();
    let mut list = ocaml::Value::new(ocaml::sys::val_int(0)); // []

    for j in (0..natts).rev() {
        let att = match pg_tupdesc.get(j) {
            Some(a) => a,
            None => continue,
        };

        if att.attisdropped {
            continue;
        }

        let mut isnull = false;
        let val = pg_sys::heap_getattr(tuple, (j + 1) as i32, tupdesc_ptr, &mut isnull);
        let type_oid = att.atttypid;
        let col_name_cstr = CStr::from_ptr(att.attname.data.as_ptr());
        let col_name = col_name_cstr.to_str().unwrap_or("");

        let name_val = ocaml::Value::string(col_name);
        let datum_val = make_ocaml_datum(type_oid, val, isnull);

        let pair = ocaml::sys::caml_alloc(2, 0);
        *ocaml::sys::field(pair, 0) = name_val.raw().0;
        *ocaml::sys::field(pair, 1) = datum_val.raw().0;
        let pair_val = ocaml::Value::new(pair);

        let node = ocaml::sys::caml_alloc(2, 0);
        *ocaml::sys::field(node, 0) = pair_val.raw().0;
        *ocaml::sys::field(node, 1) = list.raw().0;
        list = ocaml::Value::new(node);
    }

    list
}
