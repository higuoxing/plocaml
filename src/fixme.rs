#[macro_export]
macro_rules! pg_finfo_v1 {
    ($name:ident) => {
        #[no_mangle]
        pub extern "C-unwind" fn $name() -> *const pg_sys::Pg_finfo_record {
            static FINFO: pg_sys::Pg_finfo_record = pg_sys::Pg_finfo_record { api_version: 1 };
            &raw const FINFO
        }
    };
}
