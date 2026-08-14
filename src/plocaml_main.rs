use pgrx::prelude::*;

mod fixme;
mod plocaml_call_handler;
mod plocaml_inline_handler;
mod plocaml_validator;

::pgrx::pg_module_magic!(name, version);

extension_sql_file!(
    "sql/plocamlu.sql",
    name = "plocamlu",
    requires = [plocaml_validator]
);

#[pg_guard]
pub extern "C-unwind" fn _PG_init() {
    ocaml::runtime::init_persistent();
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;

    #[pg_test]
    fn test_extension_loads() {
        // Verify the extension can be loaded and the language is registered
        let has_lang =
            Spi::get_one::<bool>("SELECT COUNT(*) > 0 FROM pg_language WHERE lanname = 'plocamlu'")
                .expect("SPI query failed");
        assert!(has_lang == Some(true), "plocamlu language not found");
    }
}

/// This module is required by `cargo pgrx test` invocations.
/// It must be visible at the root of your extension crate.
#[cfg(test)]
pub mod pg_test {
    pub fn setup(_options: Vec<&str>) {
        // perform one-off initialization when the pg_test framework starts
    }

    #[must_use]
    pub fn postgresql_conf_options() -> Vec<&'static str> {
        // return any postgresql.conf settings that are required for your tests
        vec![]
    }
}
