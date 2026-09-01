use crate::fixme::SpiGuard;
use crate::pg_finfo_v1;
use pgrx::prelude::*;
use std::ffi::CStr;

pg_finfo_v1!(pg_finfo_plocaml_inline_handler);

#[no_mangle]
#[pg_guard]
pub extern "C-unwind" fn plocaml_inline_handler(fcinfo: pg_sys::FunctionCallInfo) -> pg_sys::Datum {
    if fcinfo.is_null() {
        pgrx::error!("plocaml_inline_handler: fcinfo is null");
    }

    // Retrieve InlineCodeBlock from the first argument (PG_GETARG_DATUM(0))
    let codeblock = unsafe {
        let slice = (*fcinfo).args.as_slice(1);
        slice[0].value.cast_mut_ptr::<pg_sys::InlineCodeBlock>()
    };

    if codeblock.is_null() {
        pgrx::error!("plocaml_inline_handler: codeblock is null");
    }

    let (source_text, _lang_oid, is_atomic) = unsafe {
        let cb = &*codeblock;
        if cb.source_text.is_null() {
            pgrx::error!("plocaml_inline_handler: source_text is null");
        }
        let src = CStr::from_ptr(cb.source_text)
            .to_str()
            .expect("invalid UTF-8 in inline code block source_text");
        (src, cb.langOid, cb.atomic)
    };

    // Connect to SPI manager with appropriate atomicity (mirrors PL/Python)
    unsafe {
        let rc = pg_sys::SPI_connect_ext(if is_atomic {
            0
        } else {
            pg_sys::SPI_OPT_NONATOMIC as i32
        });
        if rc != pg_sys::SPI_OK_CONNECT as i32 {
            pgrx::error!("SPI_connect_ext failed: {rc}");
        }
    }

    // Ensure SPI_finish() is called upon normal return or unwinding
    let _spi_guard = SpiGuard;

    // Execute the inline OCaml code if plocaml_execute callback is registered
    if let Some(execute_fn) = unsafe { ocaml::Value::named("plocaml_execute") } {
        let source_val = unsafe { ocaml::Value::string(source_text) };
        unsafe {
            if let Err(err_msg) = crate::plocaml_error::call_exn(execute_fn, &[source_val]) {
                crate::plocaml_error::raise_ocaml_error(err_msg);
            }
        }
    }

    unsafe {
        (*fcinfo).isnull = false;
    }

    pg_sys::Datum::from(0)
}
