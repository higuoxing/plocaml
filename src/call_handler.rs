use crate::pg_finfo_v1;
use pgrx::prelude::*;

pg_finfo_v1!(pg_finfo_plocaml_call_handler);

#[no_mangle]
#[pg_guard]
pub extern "C-unwind" fn plocaml_call_handler(_fcinfo: pg_sys::FunctionCallInfo) -> pg_sys::Datum {
    todo!()
}
