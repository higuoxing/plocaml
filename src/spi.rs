use pgrx::pg_sys::{self, panic::CaughtError, PgTryBuilder};

pub struct SpiPlan {
    pub(crate) plan: pg_sys::SPIPlanPtr,
    pub(crate) nargs: usize,
    pub(crate) types: Vec<pg_sys::Oid>,
}

impl Drop for SpiPlan {
    fn drop(&mut self) {
        if !self.plan.is_null() {
            unsafe {
                pg_sys::SPI_freeplan(self.plan);
            }
            self.plan = std::ptr::null_mut();
        }
    }
}

ocaml::custom!(SpiPlan);

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

#[no_mangle]
pub unsafe extern "C" fn plocaml_spi_prepare(
    query_val: ocaml::sys::Value,
    argtypes_val: ocaml::sys::Value,
) -> ocaml::sys::Value {
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

    let nargs = ocaml::sys::wosize_val(argtypes_val);

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
        let mut type_oids = Vec::with_capacity(nargs);
        for i in 0..nargs {
            let elem = *ocaml::sys::field(argtypes_val, i);
            let s: &str = ocaml::FromValue::from_value(ocaml::Value::new(elem));
            let cs = std::ffi::CString::new(s)
                .map_err(|_| "Type name contains null byte".to_string())?;

            let mut type_id = pg_sys::InvalidOid;
            let mut typmod = -1;
            #[cfg(any(feature = "pg13", feature = "pg14", feature = "pg15"))]
            pg_sys::parseTypeString(cs.as_ptr(), &mut type_id, &mut typmod, false);
            #[cfg(not(any(feature = "pg13", feature = "pg14", feature = "pg15")))]
            pg_sys::parseTypeString(cs.as_ptr(), &mut type_id, &mut typmod, std::ptr::null_mut());
            type_oids.push(type_id);
        }

        let plan = pg_sys::SPI_prepare(
            query_str.as_ptr(),
            nargs as i32,
            if nargs > 0 {
                type_oids.as_mut_ptr()
            } else {
                std::ptr::null_mut()
            },
        );

        if plan.is_null() {
            Err("SPI_prepare failed".to_string())
        } else {
            let keepplan_res = pg_sys::SPI_keepplan(plan);
            if keepplan_res != 0 {
                Err(format!(
                    "SPI_keepplan failed with status code {keepplan_res}"
                ))
            } else {
                Ok((plan, type_oids))
            }
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
        Ok((plan, types)) => {
            pg_sys::ReleaseCurrentSubTransaction();
            pg_sys::MemoryContextSwitchTo(oldcontext);
            pg_sys::CurrentResourceOwner = oldowner;
            pg_sys::SPI_finish();

            let spi_plan = SpiPlan { plan, nargs, types };
            let ptr = ocaml::Pointer::alloc_custom(spi_plan);
            ptr.0.raw().0
        }
        Err(err_msg) => {
            pg_sys::RollbackAndReleaseCurrentSubTransaction();
            pg_sys::MemoryContextSwitchTo(oldcontext);
            pg_sys::CurrentResourceOwner = oldowner;
            pg_sys::SPI_finish();

            let err_c_str = std::ffi::CString::new(err_msg)
                .unwrap_or_else(|_| std::ffi::CString::new("SPI prepare error").unwrap());
            ocaml::sys::caml_failwith(err_c_str.as_ptr());
            unreachable!()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn plocaml_spi_execute_plan(
    plan_val: ocaml::sys::Value,
    args_val: ocaml::sys::Value,
) -> ocaml::sys::Value {
    let plan_ptr: ocaml::Pointer<SpiPlan> =
        ocaml::FromValue::from_value(ocaml::Value::new(plan_val));
    let spi_plan = plan_ptr.as_ref();

    if spi_plan.plan.is_null() {
        let err = c"PL/OCaml: attempt to execute a freed plan";
        ocaml::sys::caml_failwith(err.as_ptr());
        unreachable!()
    }

    let nargs = ocaml::sys::wosize_val(args_val);
    if nargs != spi_plan.nargs {
        let err = std::ffi::CString::new(format!(
            "PL/OCaml: incorrect number of arguments for plan (expected {}, got {})",
            spi_plan.nargs, nargs
        ))
        .unwrap();
        ocaml::sys::caml_failwith(err.as_ptr());
        unreachable!()
    }

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
        let mut values: Vec<pg_sys::Datum> = Vec::with_capacity(nargs);
        let mut nulls: Vec<std::os::raw::c_char> = Vec::with_capacity(nargs);

        for i in 0..nargs {
            let elem = *ocaml::sys::field(args_val, i);
            let target_oid = spi_plan.types[i];
            let (datum, isnull) = crate::typeio::ocaml_datum_to_pg_datum(elem, target_oid)?;
            values.push(datum);
            nulls.push(if isnull {
                b'n' as std::os::raw::c_char
            } else {
                b' ' as std::os::raw::c_char
            });
        }

        let res = pg_sys::SPI_execute_plan(
            spi_plan.plan,
            if nargs > 0 {
                values.as_mut_ptr()
            } else {
                std::ptr::null_mut()
            },
            if nargs > 0 {
                nulls.as_mut_ptr()
            } else {
                std::ptr::null_mut()
            },
            false,
            0,
        );

        if res < 0 {
            Err(format!("SPI_execute_plan failed with status code {res}"))
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
                .unwrap_or_else(|_| std::ffi::CString::new("SPI execute_plan error").unwrap());
            ocaml::sys::caml_failwith(err_c_str.as_ptr());
            unreachable!()
        }
    }
}
