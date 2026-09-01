use pgrx::pg_sys;

// FIXME: pgrx does not provide a public macro equivalent to PostgreSQL's `PG_FUNCTION_INFO_V1`.
// While `#[pg_extern]` automatically generates the `Pg_finfo_record` for standard exported functions,
// raw `extern "C-unwind"` handlers (such as language call, inline, or validator handlers taking
// `FunctionCallInfo`) require defining the `pg_finfo_<func>` symbol manually.
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

// FIXME: pgrx does not expose `SPI_connect_ext` or a public RAII guard for SPI connections.
// Its built-in `SpiClient` is `pub(super)` and only accessible via `Spi::connect` / `Spi::connect_mut`,
// which unconditionally invokes `SPI_connect()` without allowing `SPI_connect_ext(SPI_OPT_NONATOMIC)`.
// PL call and inline handlers require `SPI_connect_ext` to support non-atomic execution contexts.
pub struct SpiGuard;

impl Drop for SpiGuard {
    fn drop(&mut self) {
        unsafe {
            pg_sys::SPI_finish();
        }
    }
}
