use crate::fixme::SpiGuard;
use crate::pg_finfo_v1;
use pgrx::prelude::*;

pg_finfo_v1!(pg_finfo_plocaml_call_handler);

#[no_mangle]
#[pg_guard]
pub extern "C-unwind" fn plocaml_call_handler(fcinfo: pg_sys::FunctionCallInfo) -> pg_sys::Datum {
    if fcinfo.is_null() {
        pgrx::error!("plocaml_call_handler: fcinfo is null");
    }

    // Check if called as trigger
    let is_trigger = unsafe {
        let ctx = (*fcinfo).context;
        !ctx.is_null()
            && ((*ctx.cast::<pg_sys::Node>()).type_ == pg_sys::NodeTag::T_TriggerData
                || (*ctx.cast::<pg_sys::Node>()).type_ == pg_sys::NodeTag::T_EventTriggerData)
    };
    if is_trigger {
        pgrx::error!("PL/OCaml: triggers are not yet supported");
    }

    let flinfo = unsafe { &*(*fcinfo).flinfo };
    let fn_oid = flinfo.fn_oid;

    let proc = match pgrx::pg_catalog::pg_proc::PgProc::new(fn_oid) {
        Some(p) => p,
        None => pgrx::error!(
            "plocaml_call_handler: pg_proc entry not found for OID {:?}",
            fn_oid
        ),
    };

    let prosrc = proc.prosrc();
    let pronargs = proc.pronargs();
    let proargtypes = proc.proargtypes();
    let proargnames = proc.proargnames();
    let prorettype = proc.prorettype();

    let is_atomic = unsafe {
        let ctx = (*fcinfo).context;
        if !ctx.is_null() && (*ctx.cast::<pg_sys::Node>()).type_ == pg_sys::NodeTag::T_CallContext {
            (*ctx.cast::<pg_sys::CallContext>()).atomic
        } else {
            true
        }
    };

    // Connect to SPI manager
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
    let _spi_guard = SpiGuard;

    // Convert arguments to OCaml PL.datum array
    let args_slice = unsafe { (*fcinfo).args.as_slice(pronargs) };
    let arr_val = unsafe {
        let arr = ocaml::sys::caml_alloc(pronargs, 0);
        ocaml::Value::new(arr)
    };

    for i in 0..pronargs {
        let arg_val = args_slice[i].value;
        let arg_isnull = args_slice[i].isnull;
        let type_oid = if i < proargtypes.len() {
            proargtypes[i]
        } else {
            pg_sys::InvalidOid
        };

        let datum_val = unsafe { crate::typeio::make_ocaml_datum(type_oid, arg_val, arg_isnull) };
        unsafe {
            *ocaml::sys::field(arr_val.raw().0, i) = datum_val.raw().0;
        }
    }

    // Build argument names array for OCaml
    let names_arr_val = unsafe {
        let names_arr = ocaml::sys::caml_alloc(pronargs, 0);
        ocaml::Value::new(names_arr)
    };
    for i in 0..pronargs {
        let name_str = proargnames.get(i).and_then(|n| n.as_deref()).unwrap_or("");
        let name_val = unsafe { ocaml::Value::string(name_str) };
        unsafe {
            *ocaml::sys::field(names_arr_val.raw().0, i) = name_val.raw().0;
        }
    }

    // Invoke compiled OCaml function (automatically compiles and caches on first call or when source changes)
    let invoke_fn = unsafe { ocaml::Value::named("plocaml_invoke_function") }
        .unwrap_or_else(|| pgrx::error!("plocaml_invoke_function callback not registered"));
    let fn_oid_val = unsafe { ocaml::Value::new(ocaml::sys::val_int(fn_oid.to_u32() as isize)) };
    let prosrc_val = unsafe { ocaml::Value::string(&prosrc) };

    let result_val = unsafe {
        match crate::error::call_exn(invoke_fn, &[fn_oid_val, prosrc_val, names_arr_val, arr_val]) {
            Ok(v) => v,
            Err(err_msg) => crate::error::raise_ocaml_error(err_msg),
        }
    };

    // Convert OCaml result value to PostgreSQL Datum
    let (datum, isnull) = unsafe {
        match crate::typeio::ocaml_value_to_pg_datum(result_val.raw().0, prorettype) {
            Ok(res) => res,
            Err(err_msg) => crate::error::raise_ocaml_error(err_msg),
        }
    };

    let transferred_datum = if isnull || prorettype == pg_sys::VOIDOID {
        datum
    } else {
        unsafe {
            let mut typlen = 0;
            let mut typbyval = false;
            pg_sys::get_typlenbyval(prorettype, &mut typlen, &mut typbyval);
            pg_sys::SPI_datumTransfer(datum, typbyval, typlen as i32)
        }
    };

    unsafe {
        (*fcinfo).isnull = isnull;
    }

    transferred_datum
}
