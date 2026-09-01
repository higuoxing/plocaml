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

pub(crate) unsafe fn make_ocaml_datum(
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

/// Converts a string into PostgreSQL's internal binary Datum representation
/// for `type_oid` by invoking the type's registered input function (`typinput`).
///
/// This serves as the counterpart to `OidOutputFunctionCall` used in `make_ocaml_datum`,
/// allowing arbitrary PostgreSQL types (such as `numeric`, `timestamp`, `uuid`, `jsonb`, etc.)
/// to be parsed from string representations when passing parameters to prepared SPI plans.
pub(crate) unsafe fn string_to_pg_datum(
    s: &str,
    type_oid: pg_sys::Oid,
) -> Result<pg_sys::Datum, String> {
    let mut typinput = pg_sys::InvalidOid;
    let mut typioparam = pg_sys::InvalidOid;
    pg_sys::getTypeInputInfo(type_oid, &mut typinput, &mut typioparam);
    let cstr =
        std::ffi::CString::new(s).map_err(|_| "String argument contains null byte".to_string())?;
    let datum = pg_sys::OidInputFunctionCall(typinput, cstr.as_ptr().cast_mut(), typioparam, -1);
    Ok(datum)
}

pub(crate) unsafe fn ocaml_datum_to_pg_datum(
    val: ocaml::sys::Value,
    type_oid: pg_sys::Oid,
) -> Result<(pg_sys::Datum, bool), String> {
    if ocaml::sys::is_long(val) {
        // Tag Null (int 0)
        return Ok((pg_sys::Datum::from(0), true));
    }

    let tag = ocaml::sys::tag_val(val);
    let datum = match tag {
        DATUM_TAG_INT => {
            let n = ocaml::sys::int_val(*ocaml::sys::field(val, 0));
            if type_oid == pg_sys::INT2OID {
                pg_sys::Int16GetDatum(n as i16)
            } else if type_oid == pg_sys::INT4OID {
                pg_sys::Int32GetDatum(n as i32)
            } else if type_oid == pg_sys::INT8OID {
                pg_sys::Int64GetDatum(n as i64)
            } else if type_oid == pg_sys::FLOAT4OID {
                pg_sys::Float4GetDatum(n as f32)
            } else if type_oid == pg_sys::FLOAT8OID {
                pg_sys::Float8GetDatum(n as f64)
            } else if type_oid == pg_sys::BOOLOID {
                pg_sys::BoolGetDatum(n != 0)
            } else {
                string_to_pg_datum(&n.to_string(), type_oid)?
            }
        }
        DATUM_TAG_FLOAT => {
            let f = ocaml::Value::new(*ocaml::sys::field(val, 0)).double_val();
            if type_oid == pg_sys::FLOAT4OID {
                pg_sys::Float4GetDatum(f as f32)
            } else if type_oid == pg_sys::FLOAT8OID {
                pg_sys::Float8GetDatum(f)
            } else if type_oid == pg_sys::INT2OID {
                pg_sys::Int16GetDatum(f as i16)
            } else if type_oid == pg_sys::INT4OID {
                pg_sys::Int32GetDatum(f as i32)
            } else if type_oid == pg_sys::INT8OID {
                pg_sys::Int64GetDatum(f as i64)
            } else {
                string_to_pg_datum(&f.to_string(), type_oid)?
            }
        }
        DATUM_TAG_STRING => {
            let s_val = ocaml::Value::new(*ocaml::sys::field(val, 0));
            let s: &str = ocaml::FromValue::from_value(s_val);
            string_to_pg_datum(s, type_oid)?
        }
        DATUM_TAG_BOOL => {
            let b = ocaml::sys::int_val(*ocaml::sys::field(val, 0)) != 0;
            if type_oid == pg_sys::BOOLOID {
                pg_sys::BoolGetDatum(b)
            } else if type_oid == pg_sys::INT4OID {
                pg_sys::Int32GetDatum(if b { 1 } else { 0 })
            } else {
                string_to_pg_datum(if b { "true" } else { "false" }, type_oid)?
            }
        }
        _ => {
            return Err(format!("Unsupported datum tag {tag} for SPI argument"));
        }
    };

    Ok((datum, false))
}

pub(crate) unsafe fn ocaml_value_to_pg_datum(
    val: ocaml::sys::Value,
    type_oid: pg_sys::Oid,
) -> Result<(pg_sys::Datum, bool), String> {
    if type_oid == pg_sys::VOIDOID {
        return Ok((pg_sys::Datum::from(0), false));
    }

    if ocaml::sys::is_long(val) {
        let n = ocaml::sys::int_val(val);
        if type_oid == pg_sys::BOOLOID {
            return Ok((pg_sys::BoolGetDatum(n != 0), false));
        } else if type_oid == pg_sys::INT2OID {
            return Ok((pg_sys::Int16GetDatum(n as i16), false));
        } else if type_oid == pg_sys::INT4OID {
            return Ok((pg_sys::Int32GetDatum(n as i32), false));
        } else if type_oid == pg_sys::INT8OID {
            return Ok((pg_sys::Int64GetDatum(n as i64), false));
        } else if type_oid == pg_sys::FLOAT4OID {
            return Ok((pg_sys::Float4GetDatum(n as f32), false));
        } else if type_oid == pg_sys::FLOAT8OID {
            return Ok((pg_sys::Float8GetDatum(n as f64), false));
        } else if n == 0 {
            // Null or None
            return Ok((pg_sys::Datum::from(0), true));
        } else {
            let datum = string_to_pg_datum(&n.to_string(), type_oid)?;
            return Ok((datum, false));
        }
    }

    let tag = ocaml::sys::tag_val(val);
    let size = ocaml::sys::wosize_val(val);

    if tag == ocaml::sys::STRING {
        let s_val = ocaml::Value::new(val);
        let s: &str = ocaml::FromValue::from_value(s_val);
        let datum = string_to_pg_datum(s, type_oid)?;
        return Ok((datum, false));
    }

    if tag == ocaml::sys::DOUBLE {
        let f = ocaml::Value::new(val).double_val();
        if type_oid == pg_sys::FLOAT4OID {
            return Ok((pg_sys::Float4GetDatum(f as f32), false));
        } else if type_oid == pg_sys::FLOAT8OID {
            return Ok((pg_sys::Float8GetDatum(f), false));
        } else if type_oid == pg_sys::INT2OID {
            return Ok((pg_sys::Int16GetDatum(f as i16), false));
        } else if type_oid == pg_sys::INT4OID {
            return Ok((pg_sys::Int32GetDatum(f as i32), false));
        } else if type_oid == pg_sys::INT8OID {
            return Ok((pg_sys::Int64GetDatum(f as i64), false));
        } else {
            let datum = string_to_pg_datum(&f.to_string(), type_oid)?;
            return Ok((datum, false));
        }
    }

    match tag {
        DATUM_TAG_INT if size == 1 => {
            let field0 = *ocaml::sys::field(val, 0);
            if ocaml::sys::is_long(field0) {
                let n = ocaml::sys::int_val(field0);
                if type_oid == pg_sys::INT2OID {
                    return Ok((pg_sys::Int16GetDatum(n as i16), false));
                } else if type_oid == pg_sys::INT4OID {
                    return Ok((pg_sys::Int32GetDatum(n as i32), false));
                } else if type_oid == pg_sys::INT8OID {
                    return Ok((pg_sys::Int64GetDatum(n as i64), false));
                } else if type_oid == pg_sys::FLOAT4OID {
                    return Ok((pg_sys::Float4GetDatum(n as f32), false));
                } else if type_oid == pg_sys::FLOAT8OID {
                    return Ok((pg_sys::Float8GetDatum(n as f64), false));
                } else if type_oid == pg_sys::BOOLOID {
                    return Ok((pg_sys::BoolGetDatum(n != 0), false));
                } else {
                    let datum = string_to_pg_datum(&n.to_string(), type_oid)?;
                    return Ok((datum, false));
                }
            } else {
                return ocaml_value_to_pg_datum(field0, type_oid);
            }
        }
        DATUM_TAG_FLOAT if size == 1 => {
            let f = ocaml::Value::new(*ocaml::sys::field(val, 0)).double_val();
            if type_oid == pg_sys::FLOAT4OID {
                return Ok((pg_sys::Float4GetDatum(f as f32), false));
            } else if type_oid == pg_sys::FLOAT8OID {
                return Ok((pg_sys::Float8GetDatum(f), false));
            } else if type_oid == pg_sys::INT2OID {
                return Ok((pg_sys::Int16GetDatum(f as i16), false));
            } else if type_oid == pg_sys::INT4OID {
                return Ok((pg_sys::Int32GetDatum(f as i32), false));
            } else if type_oid == pg_sys::INT8OID {
                return Ok((pg_sys::Int64GetDatum(f as i64), false));
            } else {
                let datum = string_to_pg_datum(&f.to_string(), type_oid)?;
                return Ok((datum, false));
            }
        }
        DATUM_TAG_STRING if size == 1 => {
            let s_val = ocaml::Value::new(*ocaml::sys::field(val, 0));
            let s: &str = ocaml::FromValue::from_value(s_val);
            let datum = string_to_pg_datum(s, type_oid)?;
            return Ok((datum, false));
        }
        DATUM_TAG_BOOL if size == 1 => {
            let b = ocaml::sys::int_val(*ocaml::sys::field(val, 0)) != 0;
            if type_oid == pg_sys::BOOLOID {
                return Ok((pg_sys::BoolGetDatum(b), false));
            } else if type_oid == pg_sys::INT4OID {
                return Ok((pg_sys::Int32GetDatum(if b { 1 } else { 0 }), false));
            } else {
                let datum = string_to_pg_datum(if b { "true" } else { "false" }, type_oid)?;
                return Ok((datum, false));
            }
        }
        _ => {
            return Err(format!(
                "Unsupported return value tag {tag} for PostgreSQL type {type_oid:?}"
            ));
        }
    }
}
