use crate::spi::SpiPlan;
use pgrx::pg_sys::{self, panic::CaughtError, PgTryBuilder};
use std::ffi::{CStr, CString};

pub struct SpiCursor {
    pub(crate) name: String,
    pub(crate) closed: bool,
}

impl Drop for SpiCursor {
    fn drop(&mut self) {
        if !self.closed {
            if let Ok(c_name) = CString::new(self.name.as_str()) {
                unsafe {
                    let portal = pg_sys::SPI_cursor_find(c_name.as_ptr());
                    if !portal.is_null() {
                        pg_sys::SPI_cursor_close(portal);
                    }
                }
            }
            self.closed = true;
        }
    }
}

ocaml::custom!(SpiCursor);

#[no_mangle]
pub unsafe extern "C" fn plocaml_spi_cursor_open(
    query_val: ocaml::sys::Value,
) -> ocaml::sys::Value {
    let query_str = {
        let v = ocaml::Value::new(query_val);
        let s: &str = ocaml::FromValue::from_value(v);
        match CString::new(s) {
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
        let portal = pg_sys::SPI_cursor_open_with_args(
            std::ptr::null_mut(),
            query_str.as_ptr(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            false,
            0,
        );

        if portal.is_null() {
            Err("SPI_cursor_open failed".to_string())
        } else {
            let name = CStr::from_ptr((*portal).name)
                .to_str()
                .map_err(|_| "Invalid UTF-8 in cursor name")?
                .to_string();
            Ok(name)
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
        Ok(name) => {
            pg_sys::ReleaseCurrentSubTransaction();
            pg_sys::MemoryContextSwitchTo(oldcontext);
            pg_sys::CurrentResourceOwner = oldowner;
            pg_sys::SPI_finish();

            let spi_cursor = SpiCursor {
                name,
                closed: false,
            };
            let ptr = ocaml::Pointer::alloc_custom(spi_cursor);
            ptr.0.raw().0
        }
        Err(err_msg) => {
            pg_sys::RollbackAndReleaseCurrentSubTransaction();
            pg_sys::MemoryContextSwitchTo(oldcontext);
            pg_sys::CurrentResourceOwner = oldowner;
            pg_sys::SPI_finish();

            let err_c_str = CString::new(err_msg)
                .unwrap_or_else(|_| CString::new("SPI cursor open error").unwrap());
            ocaml::sys::caml_failwith(err_c_str.as_ptr());
            unreachable!()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn plocaml_spi_cursor_open_plan(
    plan_val: ocaml::sys::Value,
    args_val: ocaml::sys::Value,
) -> ocaml::sys::Value {
    let plan_ptr: ocaml::Pointer<SpiPlan> =
        ocaml::FromValue::from_value(ocaml::Value::new(plan_val));
    let spi_plan = plan_ptr.as_ref();

    if spi_plan.plan.is_null() {
        let err = c"PL/OCaml: attempt to open cursor with a freed plan";
        ocaml::sys::caml_failwith(err.as_ptr());
        unreachable!()
    }

    let nargs = ocaml::sys::wosize_val(args_val);
    if nargs != spi_plan.nargs {
        let err = CString::new(format!(
            "PL/OCaml: incorrect number of arguments for cursor plan (expected {}, got {})",
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

        let portal = pg_sys::SPI_cursor_open(
            std::ptr::null_mut(),
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
        );

        if portal.is_null() {
            Err("SPI_cursor_open with plan failed".to_string())
        } else {
            let name = CStr::from_ptr((*portal).name)
                .to_str()
                .map_err(|_| "Invalid UTF-8 in cursor name")?
                .to_string();
            Ok(name)
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
        Ok(name) => {
            pg_sys::ReleaseCurrentSubTransaction();
            pg_sys::MemoryContextSwitchTo(oldcontext);
            pg_sys::CurrentResourceOwner = oldowner;
            pg_sys::SPI_finish();

            let spi_cursor = SpiCursor {
                name,
                closed: false,
            };
            let ptr = ocaml::Pointer::alloc_custom(spi_cursor);
            ptr.0.raw().0
        }
        Err(err_msg) => {
            pg_sys::RollbackAndReleaseCurrentSubTransaction();
            pg_sys::MemoryContextSwitchTo(oldcontext);
            pg_sys::CurrentResourceOwner = oldowner;
            pg_sys::SPI_finish();

            let err_c_str = CString::new(err_msg)
                .unwrap_or_else(|_| CString::new("SPI cursor open plan error").unwrap());
            ocaml::sys::caml_failwith(err_c_str.as_ptr());
            unreachable!()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn plocaml_spi_cursor_fetch(
    cursor_val: ocaml::sys::Value,
    count_val: ocaml::sys::Value,
) -> ocaml::sys::Value {
    let cursor_ptr: ocaml::Pointer<SpiCursor> =
        ocaml::FromValue::from_value(ocaml::Value::new(cursor_val));
    let spi_cursor = cursor_ptr.as_ref();

    if spi_cursor.closed {
        let err = c"PL/OCaml: cursor is closed";
        ocaml::sys::caml_failwith(err.as_ptr());
        unreachable!()
    }

    let count = ocaml::sys::int_val(count_val);
    if count <= 0 {
        let res = crate::spi::build_spi_result(pg_sys::SPI_OK_FETCH as i32, 0);
        return res.raw().0;
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
        let c_name = CString::new(spi_cursor.name.as_str())
            .map_err(|_| "Cursor name contains null byte".to_string())?;

        let portal = pg_sys::SPI_cursor_find(c_name.as_ptr());
        if portal.is_null() {
            return Err(format!(
                "PL/OCaml: cursor \"{}\" does not exist or has been closed",
                spi_cursor.name
            ));
        }

        pg_sys::SPI_cursor_fetch(portal, true, count as ::core::ffi::c_long);
        Ok(pg_sys::SPI_processed as usize)
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
        Ok(nrows) => {
            pg_sys::ReleaseCurrentSubTransaction();
            pg_sys::MemoryContextSwitchTo(oldcontext);
            pg_sys::CurrentResourceOwner = oldowner;

            let result = crate::spi::build_spi_result(pg_sys::SPI_OK_FETCH as i32, nrows);
            pg_sys::SPI_finish();
            result.raw().0
        }
        Err(err_msg) => {
            pg_sys::RollbackAndReleaseCurrentSubTransaction();
            pg_sys::MemoryContextSwitchTo(oldcontext);
            pg_sys::CurrentResourceOwner = oldowner;
            pg_sys::SPI_finish();

            let err_c_str = CString::new(err_msg)
                .unwrap_or_else(|_| CString::new("SPI cursor fetch error").unwrap());
            ocaml::sys::caml_failwith(err_c_str.as_ptr());
            unreachable!()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn plocaml_spi_cursor_close(
    cursor_val: ocaml::sys::Value,
) -> ocaml::sys::Value {
    let mut cursor_ptr: ocaml::Pointer<SpiCursor> =
        ocaml::FromValue::from_value(ocaml::Value::new(cursor_val));
    let spi_cursor = cursor_ptr.as_mut();

    if spi_cursor.closed {
        return ocaml::sys::UNIT;
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
        let c_name = CString::new(spi_cursor.name.as_str())
            .map_err(|_| "Cursor name contains null byte".to_string())?;

        let portal = pg_sys::SPI_cursor_find(c_name.as_ptr());
        if !portal.is_null() {
            pg_sys::SPI_cursor_close(portal);
        }
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

    spi_cursor.closed = true;

    match try_res {
        Ok(()) => {
            pg_sys::ReleaseCurrentSubTransaction();
            pg_sys::MemoryContextSwitchTo(oldcontext);
            pg_sys::CurrentResourceOwner = oldowner;
            pg_sys::SPI_finish();
            ocaml::sys::UNIT
        }
        Err(err_msg) => {
            pg_sys::RollbackAndReleaseCurrentSubTransaction();
            pg_sys::MemoryContextSwitchTo(oldcontext);
            pg_sys::CurrentResourceOwner = oldowner;
            pg_sys::SPI_finish();

            let err_c_str = CString::new(err_msg)
                .unwrap_or_else(|_| CString::new("SPI cursor close error").unwrap());
            ocaml::sys::caml_failwith(err_c_str.as_ptr());
            unreachable!()
        }
    }
}
