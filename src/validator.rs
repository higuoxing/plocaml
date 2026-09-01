use pgrx::prelude::*;

#[pg_extern]
fn plocaml_validator(_oid: pg_sys::Oid) {
    // Basic validator placeholder; full body validation can be added later.
}
