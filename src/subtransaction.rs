use pgrx::pg_sys::{self, panic::CaughtError, PgTryBuilder};

/// Execute an OCaml thunk inside an internal PostgreSQL subtransaction.
///
/// If the thunk completes normally, the subtransaction is committed.
/// If an OCaml exception is raised, the subtransaction is rolled back and the
/// exception is re-raised.
#[no_mangle]
pub unsafe extern "C" fn plocaml_subtransaction(thunk: ocaml::sys::Value) -> ocaml::sys::Value {
    let oldcontext = pg_sys::CurrentMemoryContext;
    let oldowner = pg_sys::CurrentResourceOwner;

    pg_sys::BeginInternalSubTransaction(std::ptr::null_mut());
    // Run the code in the caller's memory context
    pg_sys::MemoryContextSwitchTo(oldcontext);

    let res = ocaml::sys::caml_callback_exn(thunk, ocaml::sys::UNIT);

    if ocaml::sys::is_exception_result(res) {
        let exc = ocaml::sys::extract_exception(res);
        pg_sys::RollbackAndReleaseCurrentSubTransaction();
        pg_sys::MemoryContextSwitchTo(oldcontext);
        pg_sys::CurrentResourceOwner = oldowner;
        ocaml::sys::caml_raise(exc);
    }

    pg_sys::ReleaseCurrentSubTransaction();
    pg_sys::MemoryContextSwitchTo(oldcontext);
    pg_sys::CurrentResourceOwner = oldowner;

    res
}

/// Commit the current transaction in a non-atomic execution context (e.g. DO block or procedure).
#[no_mangle]
pub unsafe extern "C" fn plocaml_commit(_unit: ocaml::sys::Value) -> ocaml::sys::Value {
    let oldcontext = pg_sys::CurrentMemoryContext;

    let try_res = PgTryBuilder::new(|| {
        pg_sys::SPI_commit();
        Ok(())
    })
    .catch_others(|err| {
        let msg = match err {
            CaughtError::PostgresError(e) | CaughtError::ErrorReport(e) => e.message().to_string(),
            CaughtError::RustPanic { ereport, .. } => ereport.message().to_string(),
        };
        Err(msg)
    })
    .execute();

    match try_res {
        Ok(()) => ocaml::sys::UNIT,
        Err(err_msg) => {
            pg_sys::MemoryContextSwitchTo(oldcontext);
            let err_c_str = std::ffi::CString::new(err_msg)
                .unwrap_or_else(|_| std::ffi::CString::new("SPI_commit error").unwrap());
            ocaml::sys::caml_failwith(err_c_str.as_ptr());
            unreachable!()
        }
    }
}

/// Roll back the current transaction in a non-atomic execution context (e.g. DO block or procedure).
#[no_mangle]
pub unsafe extern "C" fn plocaml_rollback(_unit: ocaml::sys::Value) -> ocaml::sys::Value {
    let oldcontext = pg_sys::CurrentMemoryContext;

    let try_res = PgTryBuilder::new(|| {
        pg_sys::SPI_rollback();
        Ok(())
    })
    .catch_others(|err| {
        let msg = match err {
            CaughtError::PostgresError(e) | CaughtError::ErrorReport(e) => e.message().to_string(),
            CaughtError::RustPanic { ereport, .. } => ereport.message().to_string(),
        };
        Err(msg)
    })
    .execute();

    match try_res {
        Ok(()) => ocaml::sys::UNIT,
        Err(err_msg) => {
            pg_sys::MemoryContextSwitchTo(oldcontext);
            let err_c_str = std::ffi::CString::new(err_msg)
                .unwrap_or_else(|_| std::ffi::CString::new("SPI_rollback error").unwrap());
            ocaml::sys::caml_failwith(err_c_str.as_ptr());
            unreachable!()
        }
    }
}
