use ocaml::{sys, Value};
use pgrx::pg_sys::errcodes::PgSqlErrorCode;
use pgrx::prelude::*;

/// Safely invokes an OCaml closure with arguments, catching any OCaml exceptions
/// without triggering `ocaml-rs`'s buggy `caml_modify` on stack variables.
pub(crate) unsafe fn call_exn(closure: Value, args: &[Value]) -> Result<Value, String> {
    let raw_res = match args {
        [] => sys::caml_callback_exn(closure.raw().0, sys::UNIT),
        [arg] => sys::caml_callback_exn(closure.raw().0, arg.raw().0),
        _ => {
            let mut raw_args: Vec<sys::Value> = args.iter().map(|v| v.raw().0).collect();
            sys::caml_callbackN_exn(closure.raw().0, raw_args.len(), raw_args.as_mut_ptr())
        }
    };

    if sys::is_exception_result(raw_res) {
        let exc = sys::extract_exception(raw_res);
        let exc_val = Value::new(exc);
        let msg = exc_val
            .exception_to_string()
            .unwrap_or_else(|_| "Unknown OCaml exception".to_string());
        Err(msg)
    } else {
        Ok(Value::new(raw_res))
    }
}

/// Report an OCaml exception or error message as a PostgreSQL error.
pub(crate) fn raise_ocaml_error(detail: impl AsRef<str>) -> ! {
    ereport!(
        ERROR,
        PgSqlErrorCode::ERRCODE_EXTERNAL_ROUTINE_EXCEPTION,
        "PL/OCaml execution failed",
        detail.as_ref()
    );
}
