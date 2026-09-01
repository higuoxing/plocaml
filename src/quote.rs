use pgrx::pg_sys;
use std::ffi::{CStr, CString};

/// Quote a string literal for SQL statements.
#[no_mangle]
pub unsafe extern "C" fn plocaml_quote_literal(val: ocaml::sys::Value) -> ocaml::sys::Value {
    let ptr = ocaml::sys::string_val(val);
    let len = ocaml::sys::caml_string_length(val);
    let slice = std::slice::from_raw_parts(ptr, len);

    let c_str = match CString::new(slice) {
        Ok(s) => s,
        Err(_) => {
            let err = CString::new("cannot quote string containing null bytes").unwrap();
            ocaml::sys::caml_invalid_argument(err.as_ptr());
            unreachable!()
        }
    };

    let quoted_ptr = pg_sys::quote_literal_cstr(c_str.as_ptr());
    if quoted_ptr.is_null() {
        let err = CString::new("quote_literal failed").unwrap();
        ocaml::sys::caml_failwith(err.as_ptr());
        unreachable!()
    }

    let quoted_cstr = CStr::from_ptr(quoted_ptr);
    let quoted_str = match quoted_cstr.to_str() {
        Ok(s) => s,
        Err(_) => {
            let err = CString::new("invalid UTF-8 in quoted string").unwrap();
            ocaml::sys::caml_failwith(err.as_ptr());
            unreachable!()
        }
    };

    let res = ocaml::Value::string(quoted_str);
    res.raw().0
}

/// Quote an identifier for SQL statements.
#[no_mangle]
pub unsafe extern "C" fn plocaml_quote_ident(val: ocaml::sys::Value) -> ocaml::sys::Value {
    let ptr = ocaml::sys::string_val(val);
    let len = ocaml::sys::caml_string_length(val);
    let slice = std::slice::from_raw_parts(ptr, len);

    let c_str = match CString::new(slice) {
        Ok(s) => s,
        Err(_) => {
            let err = CString::new("cannot quote identifier containing null bytes").unwrap();
            ocaml::sys::caml_invalid_argument(err.as_ptr());
            unreachable!()
        }
    };

    let quoted_ptr = pg_sys::quote_identifier(c_str.as_ptr());
    if quoted_ptr.is_null() {
        let err = CString::new("quote_identifier failed").unwrap();
        ocaml::sys::caml_failwith(err.as_ptr());
        unreachable!()
    }

    let quoted_cstr = CStr::from_ptr(quoted_ptr);
    let quoted_str = match quoted_cstr.to_str() {
        Ok(s) => s,
        Err(_) => {
            let err = CString::new("invalid UTF-8 in quoted identifier").unwrap();
            ocaml::sys::caml_failwith(err.as_ptr());
            unreachable!()
        }
    };

    let res = ocaml::Value::string(quoted_str);
    res.raw().0
}
