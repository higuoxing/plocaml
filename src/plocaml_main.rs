use pgrx::prelude::*;

mod fixme;
mod plocaml_call_handler;
mod plocaml_error;
mod plocaml_inline_handler;
mod plocaml_subtransaction;
mod plocaml_validator;

::pgrx::pg_module_magic!(name, version);

extension_sql_file!(
    "sql/plocamlu.sql",
    name = "plocamlu",
    requires = [plocaml_validator]
);

const BOOTSTRAP_CODE: &str = include_str!("../ml/bootstrap.ml");

#[pg_guard]
pub extern "C-unwind" fn _PG_init() {
    ocaml::runtime::init_persistent();
    if let Some(init_fn) = unsafe { ocaml::Value::named("plocaml_init_toplevel") } {
        let code_val = unsafe { ocaml::Value::string(BOOTSTRAP_CODE) };
        unsafe {
            if let Err(err_msg) = crate::plocaml_error::call_exn(init_fn, &[code_val]) {
                pgrx::error!("failed to initialize PL/OCaml toplevel: {err_msg}");
            }
        }
    }
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

    #[pg_test]
    fn test_inline_handler() {
        Spi::run("DO $$ let x = 1 + 2 in ();; () $$ LANGUAGE plocamlu;").expect("DO block failed");
    }

    #[pg_test(error = "PL/OCaml execution failed")]
    fn test_ocaml_failwith_bridging() {
        Spi::run("DO $$ failwith \"some error\" $$ LANGUAGE plocamlu;").expect("DO block failed");
    }

    #[pg_test]
    fn test_subtransaction_commit() {
        Spi::run("DO $$ PL.subtransaction (fun () -> ()) $$ LANGUAGE plocamlu;")
            .expect("DO block failed");
    }

    #[pg_test]
    fn test_subtransaction_rollback_and_catch() {
        Spi::run(
            "DO $$ try PL.subtransaction (fun () -> failwith \"inner error\") with Failure _ -> () $$ LANGUAGE plocamlu;",
        )
        .expect("DO block failed");
    }

    #[pg_test(error = "PL/OCaml execution failed")]
    fn test_subtransaction_uncaught() {
        Spi::run(
            "DO $$ PL.subtransaction (fun () -> failwith \"uncaught error\") $$ LANGUAGE plocamlu;",
        )
        .expect("DO block failed");
    }

    #[pg_test]
    fn test_subtransaction_nested() {
        Spi::run(
            r#"DO $$
            PL.subtransaction (fun () ->
                try
                    PL.subtransaction (fun () -> failwith "nested error")
                with Failure _ -> ()
            )
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
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
