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
    fn test_subtransaction_commit_and_rollback_inserted_values() {
        Spi::run(
            r#"DO $$
            let _ = PL.execute "CREATE TEMP TABLE test_subxact_counts (id int, val text)" in
            let _ = PL.execute "INSERT INTO test_subxact_counts VALUES (100, 'base')" in
            
            (* Subtx 1: successful commit *)
            PL.subtransaction (fun () ->
                let _ = PL.execute "INSERT INTO test_subxact_counts VALUES (200, 'sub1')" in
                ()
            );

            (* Subtx 2: rollback via exception *)
            (try
                PL.subtransaction (fun () ->
                    let _ = PL.execute "INSERT INTO test_subxact_counts VALUES (300, 'sub2_fail')" in
                    failwith "sub2 failed"
                )
            with Failure _ -> ());

            (* Subtx 3: successful commit *)
            PL.subtransaction (fun () ->
                let _ = PL.execute "INSERT INTO test_subxact_counts VALUES (400, 'sub3')" in
                ()
            );

            (* Subtx 4: rollback via exception *)
            (try
                PL.subtransaction (fun () ->
                    let _ = PL.execute "INSERT INTO test_subxact_counts VALUES (500, 'sub4_fail')" in
                    failwith "sub4 failed"
                )
            with Failure _ -> ());

            (* Subtx 5: successful commit *)
            PL.subtransaction (fun () ->
                let _ = PL.execute "INSERT INTO test_subxact_counts VALUES (600, 'sub5')" in
                ()
            );

            let res = PL.execute "SELECT id, val FROM test_subxact_counts ORDER BY id" in
            if res.nrows <> 4 then failwith (Printf.sprintf "expected 4 rows, got %d" res.nrows);
            let expected_ids = [100; 200; 400; 600] in
            let actual_ids = Array.to_list (Array.map (fun row ->
                match List.assoc "id" row with
                | PL.Int n -> n
                | _ -> failwith "invalid id type"
            ) res.rows) in
            if actual_ids <> expected_ids then failwith "inserted ids mismatch"
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test]
    fn test_subtransaction_nested_commit_and_rollback_inserted_values() {
        Spi::run(
            r#"DO $$
            let _ = PL.execute "CREATE TEMP TABLE test_nested_counts (id int)" in
            let _ = PL.execute "INSERT INTO test_nested_counts VALUES (1)" in

            (* Outer subtransaction A: completes normally *)
            PL.subtransaction (fun () ->
                let _ = PL.execute "INSERT INTO test_nested_counts VALUES (2)" in
                
                (* Inner A1: commits *)
                PL.subtransaction (fun () ->
                    let _ = PL.execute "INSERT INTO test_nested_counts VALUES (3)" in
                    ()
                );

                (* Inner A2: rolls back *)
                (try
                    PL.subtransaction (fun () ->
                        let _ = PL.execute "INSERT INTO test_nested_counts VALUES (4)" in
                        failwith "inner A2 failed"
                    )
                with Failure _ -> ());

                (* Inner A3: commits *)
                PL.subtransaction (fun () ->
                    let _ = PL.execute "INSERT INTO test_nested_counts VALUES (5)" in
                    ()
                )
            );

            (* Outer subtransaction B: rolls back entire tree (6, 7, 8) *)
            (try
                PL.subtransaction (fun () ->
                    let _ = PL.execute "INSERT INTO test_nested_counts VALUES (6)" in
                    
                    PL.subtransaction (fun () ->
                        let _ = PL.execute "INSERT INTO test_nested_counts VALUES (7)" in
                        ()
                    );

                    PL.subtransaction (fun () ->
                        let _ = PL.execute "INSERT INTO test_nested_counts VALUES (8)" in
                        ()
                    );

                    failwith "abort outer B"
                )
            with Failure _ -> ());

            let res = PL.execute "SELECT id FROM test_nested_counts ORDER BY id" in
            if res.nrows <> 4 then failwith (Printf.sprintf "expected 4 rows, got %d" res.nrows);
            let expected_ids = [1; 2; 3; 5] in
            let actual_ids = Array.to_list (Array.map (fun row ->
                match List.assoc "id" row with
                | PL.Int n -> n
                | _ -> failwith "invalid id type"
            ) res.rows) in
            if actual_ids <> expected_ids then failwith "nested inserted ids mismatch"
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

    #[pg_test(error = "PL/OCaml execution failed")]
    fn test_commit_in_atomic_context_fails() {
        Spi::run("DO $$ PL.commit () $$ LANGUAGE plocamlu;").expect("DO block failed");
    }

    #[pg_test(error = "PL/OCaml execution failed")]
    fn test_rollback_in_atomic_context_fails() {
        Spi::run("DO $$ PL.rollback () $$ LANGUAGE plocamlu;").expect("DO block failed");
    }

    #[pg_test]
    fn test_commit_in_atomic_context_catchable() {
        Spi::run(
            r#"DO $$
            try
              PL.commit ()
            with Failure _ -> ()
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test(error = "PL/OCaml execution failed")]
    fn test_commit_inside_subtransaction_fails() {
        Spi::run(
            r#"DO $$
            PL.subtransaction (fun () -> PL.commit ())
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }
}

#[cfg(test)]
mod host_tests {
    #[test]
    fn test_standalone_do_commit_and_rollback() {
        // Ensure the test framework is initialized and the extension is installed
        pgrx_tests::run_test("test_extension_loads", None, vec![])
            .expect("failed to run test setup");

        let (mut client, _) = pgrx_tests::client().expect("failed to connect client");
        client
            .simple_query("CREATE TABLE IF NOT EXISTS test_standalone_xact (id int);")
            .expect("create table failed");
        client
            .simple_query("TRUNCATE test_standalone_xact;")
            .expect("truncate failed");
        client
            .simple_query(
                r#"
                DO $$
                for i = 0 to 9 do
                  let _ = PL.execute (Printf.sprintf "INSERT INTO test_standalone_xact VALUES (%d)" i) in
                  if i mod 2 = 0 then
                    PL.commit ()
                  else
                    PL.rollback ()
                done
                $$ LANGUAGE plocamlu;
                "#,
            )
            .expect("standalone DO failed");

        let rows = client
            .query("SELECT id FROM test_standalone_xact ORDER BY id", &[])
            .expect("query failed");
        let ids: Vec<i32> = rows.iter().map(|r| r.get(0)).collect();
        assert_eq!(ids, vec![0, 2, 4, 6, 8]);
    }

    #[test]
    fn test_standalone_do_commit_and_rollback_blocks() {
        pgrx_tests::run_test("test_extension_loads", None, vec![])
            .expect("failed to run test setup");

        let (mut client, _) = pgrx_tests::client().expect("failed to connect client");
        client
            .simple_query("CREATE TABLE IF NOT EXISTS test_standalone_blocks (id int, tag text);")
            .expect("create table failed");
        client
            .simple_query("TRUNCATE test_standalone_blocks;")
            .expect("truncate failed");
        client
            .simple_query(
                r#"
                DO $$
                (* Block 1: insert 10, 20, 30 and commit *)
                let _ = PL.execute "INSERT INTO test_standalone_blocks VALUES (10, 'c1'), (20, 'c1'), (30, 'c1')" in
                PL.commit ();

                (* Block 2: insert 40, 50 and rollback *)
                let _ = PL.execute "INSERT INTO test_standalone_blocks VALUES (40, 'r1'), (50, 'r1')" in
                PL.rollback ();

                (* Block 3: insert 60, 70, 80 and commit *)
                let _ = PL.execute "INSERT INTO test_standalone_blocks VALUES (60, 'c2'), (70, 'c2'), (80, 'c2')" in
                PL.commit ();

                (* Block 4: insert 90 and rollback *)
                let _ = PL.execute "INSERT INTO test_standalone_blocks VALUES (90, 'r2')" in
                PL.rollback ();
                $$ LANGUAGE plocamlu;
                "#,
            )
            .expect("standalone DO failed");

        let rows = client
            .query(
                "SELECT id, tag FROM test_standalone_blocks ORDER BY id",
                &[],
            )
            .expect("query failed");
        assert_eq!(rows.len(), 6);
        let ids: Vec<i32> = rows.iter().map(|r| r.get(0)).collect();
        assert_eq!(ids, vec![10, 20, 30, 60, 70, 80]);
    }

    #[test]
    fn test_standalone_do_commit_rollback_with_prepared_plan() {
        pgrx_tests::run_test("test_extension_loads", None, vec![])
            .expect("failed to run test setup");

        let (mut client, _) = pgrx_tests::client().expect("failed to connect client");
        client
            .simple_query(
                "CREATE TABLE IF NOT EXISTS test_standalone_plan_xact (id int, label text);",
            )
            .expect("create table failed");
        client
            .simple_query("TRUNCATE test_standalone_plan_xact;")
            .expect("truncate failed");
        client
            .simple_query(
                r#"
                DO $$
                let plan = PL.prepare "INSERT INTO test_standalone_plan_xact VALUES ($1, $2)" [|"int"; "text"|] in
                for i = 1 to 6 do
                  let _ = PL.execute_plan plan [|PL.Int i; PL.String (Printf.sprintf "item_%d" i)|] in
                  if i mod 2 = 1 then
                    PL.commit ()
                  else
                    PL.rollback ()
                done
                $$ LANGUAGE plocamlu;
                "#,
            )
            .expect("standalone DO with plan failed");

        let rows = client
            .query(
                "SELECT id, label FROM test_standalone_plan_xact ORDER BY id",
                &[],
            )
            .expect("query failed");
        assert_eq!(rows.len(), 3);
        let ids: Vec<i32> = rows.iter().map(|r| r.get(0)).collect();
        let labels: Vec<String> = rows.iter().map(|r| r.get(1)).collect();
        assert_eq!(ids, vec![1, 3, 5]);
        assert_eq!(labels, vec!["item_1", "item_3", "item_5"]);
    }

    #[test]
    fn test_standalone_do_rollback_all() {
        pgrx_tests::run_test("test_extension_loads", None, vec![])
            .expect("failed to run test setup");

        let (mut client, _) = pgrx_tests::client().expect("failed to connect client");
        client
            .simple_query("CREATE TABLE IF NOT EXISTS test_standalone_rollback_all (id int);")
            .expect("create table failed");
        client
            .simple_query("TRUNCATE test_standalone_rollback_all;")
            .expect("truncate failed");
        client
            .simple_query(
                r#"
                DO $$
                for i = 1 to 5 do
                  let _ = PL.execute (Printf.sprintf "INSERT INTO test_standalone_rollback_all VALUES (%d)" i) in
                  ()
                done;
                PL.rollback ()
                $$ LANGUAGE plocamlu;
                "#,
            )
            .expect("standalone DO rollback all failed");

        let rows = client
            .query(
                "SELECT COUNT(*)::int FROM test_standalone_rollback_all",
                &[],
            )
            .expect("query failed");
        let count: i32 = rows[0].get(0);
        assert_eq!(count, 0);
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
