use pgrx::prelude::*;

mod call_handler;
mod error;
mod fixme;
mod inline_handler;
mod spi;
mod subtransaction;
mod typeio;
mod validator;

::pgrx::pg_module_magic!(name, version);

extension_sql_file!(
    "sql/plocamlu.sql",
    name = "plocamlu",
    requires = [validator::plocaml_validator]
);

const BOOTSTRAP_CODE: &str = include_str!("../ml/bootstrap.ml");

#[pg_guard]
pub extern "C-unwind" fn _PG_init() {
    ocaml::runtime::init_persistent();
    if let Some(init_fn) = unsafe { ocaml::Value::named("plocaml_init_toplevel") } {
        let code_val = unsafe { ocaml::Value::string(BOOTSTRAP_CODE) };
        unsafe {
            if let Err(err_msg) = crate::error::call_exn(init_fn, &[code_val]) {
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

    #[pg_test]
    fn test_spi_execute_select() {
        Spi::run(
            r#"DO $$
            let res = PL.execute "SELECT 42 AS num, 'hello' AS msg, true AS flag" in
            if res.nrows <> 1 then failwith "wrong nrows";
            match res.rows.(0) with
            | [("num", PL.Int 42); ("msg", PL.String "hello"); ("flag", PL.Bool true)] -> ()
            | _ -> failwith "unexpected row contents"
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test]
    fn test_spi_execute_dml_and_subtransaction() {
        Spi::run(
            r#"DO $$
            let _ = PL.execute "CREATE TEMP TABLE test_subxact (id int, name text)" in
            let _ = PL.execute "INSERT INTO test_subxact VALUES (1, 'initial')" in
            (try
               PL.subtransaction (fun () ->
                 let _ = PL.execute "INSERT INTO test_subxact VALUES (2, 'temporary')" in
                 failwith "abort inner")
             with Failure _ -> ());
            let res = PL.execute "SELECT id, name FROM test_subxact ORDER BY id" in
            if res.nrows <> 1 then failwith "wrong nrows after rollback";
            match res.rows.(0) with
            | [("id", PL.Int 1); ("name", PL.String "initial")] -> ()
            | _ -> failwith "unexpected row contents after rollback"
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test]
    fn test_spi_execute_error_caught() {
        Spi::run(
            r#"DO $$
            try
              let _ = PL.execute "SELECT * FROM non_existent_table_12345" in
              failwith "query should have failed"
            with Failure _ -> ()
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test]
    fn test_spi_prepare_and_execute_plan() {
        Spi::run(
            r#"DO $$
            let plan = PL.prepare "SELECT $1::int * 2 AS doubled, $2::text || ' world' AS greeting" [|"int"; "text"|] in
            let res = PL.execute_plan plan [|PL.Int 21; PL.String "hello"|] in
            if res.nrows <> 1 then failwith "wrong nrows";
            match res.rows.(0) with
            | [("doubled", PL.Int 42); ("greeting", PL.String "hello world")] -> ()
            | _ -> failwith "unexpected row contents from plan"
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test]
    fn test_spi_prepare_dml_and_multiple_execs() {
        Spi::run(
            r#"DO $$
            let _ = PL.execute "CREATE TEMP TABLE test_plan (id int, val text)" in
            let plan_insert = PL.prepare "INSERT INTO test_plan VALUES ($1, $2)" [|"int"; "text"|] in
            let _ = PL.execute_plan plan_insert [|PL.Int 1; PL.String "first"|] in
            let _ = PL.execute_plan plan_insert [|PL.Int 2; PL.String "second"|] in
            let plan_select = PL.prepare "SELECT id, val FROM test_plan WHERE id = $1" [|"int"|] in
            let res = PL.execute_plan plan_select [|PL.Int 2|] in
            if res.nrows <> 1 then failwith "wrong nrows";
            match res.rows.(0) with
            | [("id", PL.Int 2); ("val", PL.String "second")] -> ()
            | _ -> failwith "unexpected row contents for id 2"
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test(error = "PL/OCaml execution failed")]
    fn test_spi_execute_plan_wrong_args_count() {
        Spi::run(
            r#"DO $$
            let plan = PL.prepare "SELECT $1::int" [|"int"|] in
            let _ = PL.execute_plan plan [|PL.Int 1; PL.Int 2|] in
            ()
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
