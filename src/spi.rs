use pgrx::pg_sys::{self, panic::CaughtError, PgTryBuilder};

pub(crate) unsafe fn build_spi_result(status: i32, nrows: usize) -> ocaml::Value {
    let rows_arr = if !pg_sys::SPI_tuptable.is_null() && nrows > 0 {
        let tuptable = &*pg_sys::SPI_tuptable;
        let tupdesc = tuptable.tupdesc;
        let vals = tuptable.vals;

        let arr = ocaml::sys::caml_alloc(nrows, 0);
        let arr_val = ocaml::Value::new(arr);

        for i in 0..nrows {
            let tuple = *vals.add(i);
            let row_list = crate::typeio::heap_tuple_to_row_list(tuple, tupdesc);
            *ocaml::sys::field(arr_val.raw().0, i) = row_list.raw().0;
        }
        arr_val
    } else {
        ocaml::Value::new(ocaml::sys::caml_alloc(0, 0))
    };

    let res = ocaml::sys::caml_alloc(3, 0);
    *ocaml::sys::field(res, 0) = ocaml::sys::val_int(status as isize);
    *ocaml::sys::field(res, 1) = ocaml::sys::val_int(nrows as isize);
    *ocaml::sys::field(res, 2) = rows_arr.raw().0;

    ocaml::Value::new(res)
}

#[no_mangle]
pub unsafe extern "C" fn plocaml_spi_execute(query_val: ocaml::sys::Value) -> ocaml::sys::Value {
    let query_str = {
        let v = ocaml::Value::new(query_val);
        let s: &str = ocaml::FromValue::from_value(v);
        match std::ffi::CString::new(s) {
            Ok(cs) => cs,
            Err(_) => {
                let err = c"PL/OCaml: query contains null byte";
                ocaml::sys::caml_failwith(err.as_ptr());
                unreachable!()
            }
        }
    };

    let oldcontext = pg_sys::CurrentMemoryContext;
    let oldowner = pg_sys::CurrentResourceOwner;

    if pg_sys::SPI_connect() != pg_sys::SPI_OK_CONNECT as i32 {
        let err = c"PL/OCaml: could not connect to SPI manager";
        ocaml::sys::caml_failwith(err.as_ptr());
        unreachable!()
    }

    pg_sys::BeginInternalSubTransaction(std::ptr::null_mut());
    pg_sys::MemoryContextSwitchTo(oldcontext);

    let try_res = PgTryBuilder::new(|| {
        let res = pg_sys::SPI_execute(query_str.as_ptr(), false, 0);
        if res < 0 {
            Err(format!("SPI_execute failed with status code {res}"))
        } else {
            Ok(res)
        }
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
        Ok(status) => {
            pg_sys::ReleaseCurrentSubTransaction();
            pg_sys::MemoryContextSwitchTo(oldcontext);
            pg_sys::CurrentResourceOwner = oldowner;

            let result = build_spi_result(status, pg_sys::SPI_processed as usize);
            pg_sys::SPI_finish();
            result.raw().0
        }
        Err(err_msg) => {
            pg_sys::RollbackAndReleaseCurrentSubTransaction();
            pg_sys::MemoryContextSwitchTo(oldcontext);
            pg_sys::CurrentResourceOwner = oldowner;
            pg_sys::SPI_finish();

            let err_c_str = std::ffi::CString::new(err_msg)
                .unwrap_or_else(|_| std::ffi::CString::new("SPI execute error").unwrap());
            ocaml::sys::caml_failwith(err_c_str.as_ptr());
            unreachable!()
        }
    }
}
