use pgrx::pg_sys;

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
