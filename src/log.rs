use pgrx::pg_sys;
use std::ffi::CString;

unsafe extern "C-unwind" {
    fn errstart(elevel: ::std::os::raw::c_int, domain: *const ::std::os::raw::c_char) -> bool;
    fn errcode(sqlerrcode: ::std::os::raw::c_int) -> ::std::os::raw::c_int;
    fn errmsg(fmt: *const ::std::os::raw::c_char, ...) -> ::std::os::raw::c_int;
    fn errdetail(fmt: *const ::std::os::raw::c_char, ...) -> ::std::os::raw::c_int;
    fn errhint(fmt: *const ::std::os::raw::c_char, ...) -> ::std::os::raw::c_int;
    fn err_generic_string(
        field: ::std::os::raw::c_int,
        str: *const ::std::os::raw::c_char,
    ) -> ::std::os::raw::c_int;
    fn errfinish(
        filename: *const ::std::os::raw::c_char,
        lineno: ::std::os::raw::c_int,
        funcname: *const ::std::os::raw::c_char,
    );
}

fn sqlstate_to_errcode(s: &str) -> Option<i32> {
    let bytes = s.as_bytes();
    if bytes.len() == 5 {
        let pgsixbit = |ch: u8| ((ch as i32) - ('0' as i32)) & 0x3F;
        Some(
            pgsixbit(bytes[0])
                + (pgsixbit(bytes[1]) << 6)
                + (pgsixbit(bytes[2]) << 12)
                + (pgsixbit(bytes[3]) << 18)
                + (pgsixbit(bytes[4]) << 24),
        )
    } else {
        None
    }
}

unsafe fn extract_string(val: ocaml::sys::Value) -> String {
    let ptr = ocaml::sys::string_val(val);
    let len = ocaml::sys::caml_string_length(val);
    let slice = std::slice::from_raw_parts(ptr, len);
    String::from_utf8_lossy(slice).into_owned()
}

unsafe fn extract_string_option(opt_val: ocaml::sys::Value) -> Option<String> {
    if ocaml::sys::is_block(opt_val) {
        let str_val = *ocaml::sys::field(opt_val, 0);
        Some(extract_string(str_val))
    } else {
        None
    }
}

/// Emit a PostgreSQL log message or raise an OCaml error from PL/OCaml.
#[no_mangle]
pub unsafe extern "C" fn plocaml_elog(
    level_val: ocaml::sys::Value,
    info_val: ocaml::sys::Value,
) -> ocaml::sys::Value {
    let tag = ocaml::sys::int_val(level_val);

    let message = extract_string(*ocaml::sys::field(info_val, 0));
    let detail = extract_string_option(*ocaml::sys::field(info_val, 1));
    let hint = extract_string_option(*ocaml::sys::field(info_val, 2));
    let sqlstate = extract_string_option(*ocaml::sys::field(info_val, 3));
    let schema_name = extract_string_option(*ocaml::sys::field(info_val, 4));
    let table_name = extract_string_option(*ocaml::sys::field(info_val, 5));
    let column_name = extract_string_option(*ocaml::sys::field(info_val, 6));
    let datatype_name = extract_string_option(*ocaml::sys::field(info_val, 7));
    let constraint_name = extract_string_option(*ocaml::sys::field(info_val, 8));

    let elevel = match tag {
        0 => pg_sys::DEBUG5 as i32,
        1 => pg_sys::DEBUG4 as i32,
        2 => pg_sys::DEBUG3 as i32,
        3 => pg_sys::DEBUG2 as i32,
        4 => pg_sys::DEBUG1 as i32,
        5 => pg_sys::LOG as i32,
        6 => pg_sys::INFO as i32,
        7 => pg_sys::NOTICE as i32,
        8 => pg_sys::WARNING as i32,
        9 => pg_sys::PGERROR as i32,
        _ => pg_sys::NOTICE as i32,
    };

    if elevel == pg_sys::PGERROR as i32 {
        let mut full_msg = message;
        if let Some(d) = &detail {
            full_msg.push_str("\nDETAIL: ");
            full_msg.push_str(d);
        }
        if let Some(h) = &hint {
            full_msg.push_str("\nHINT: ");
            full_msg.push_str(h);
        }
        let c_str =
            CString::new(full_msg).unwrap_or_else(|_| CString::new("PL/OCaml error").unwrap());
        ocaml::sys::caml_failwith(c_str.as_ptr());
        unreachable!()
    }

    let domain_ptr: *const std::os::raw::c_char = std::ptr::null();
    if errstart(elevel, domain_ptr) {
        let msg_c = CString::new(message).unwrap_or_default();
        errmsg(c"%s".as_ptr(), msg_c.as_ptr());

        if let Some(d) = detail {
            let detail_c = CString::new(d).unwrap_or_default();
            errdetail(c"%s".as_ptr(), detail_c.as_ptr());
        }
        if let Some(h) = hint {
            let hint_c = CString::new(h).unwrap_or_default();
            errhint(c"%s".as_ptr(), hint_c.as_ptr());
        }
        if let Some(s) = sqlstate {
            if let Some(code) = sqlstate_to_errcode(&s) {
                errcode(code as ::std::os::raw::c_int);
            }
        }
        if let Some(s) = schema_name {
            let s_c = CString::new(s).unwrap_or_default();
            err_generic_string(pg_sys::PG_DIAG_SCHEMA_NAME as i32, s_c.as_ptr());
        }
        if let Some(s) = table_name {
            let s_c = CString::new(s).unwrap_or_default();
            err_generic_string(pg_sys::PG_DIAG_TABLE_NAME as i32, s_c.as_ptr());
        }
        if let Some(s) = column_name {
            let s_c = CString::new(s).unwrap_or_default();
            err_generic_string(pg_sys::PG_DIAG_COLUMN_NAME as i32, s_c.as_ptr());
        }
        if let Some(s) = datatype_name {
            let s_c = CString::new(s).unwrap_or_default();
            err_generic_string(pg_sys::PG_DIAG_DATATYPE_NAME as i32, s_c.as_ptr());
        }
        if let Some(s) = constraint_name {
            let s_c = CString::new(s).unwrap_or_default();
            err_generic_string(pg_sys::PG_DIAG_CONSTRAINT_NAME as i32, s_c.as_ptr());
        }

        let file_c = c"plocaml";
        let func_c = c"plocaml_elog";
        errfinish(file_c.as_ptr(), 0, func_c.as_ptr());
    }

    ocaml::sys::UNIT
}
