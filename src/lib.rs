use pgrx::prelude::*;

mod call_handler;
mod cursor;
mod error;
mod fixme;
mod inline_handler;
mod log;
mod quote;
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

    #[pg_test]
    fn test_log_notice_and_warning_and_info() {
        Spi::run(
            r#"DO $$
            PL.Log.notice "This is a notice message";
            PL.Log.warning "This is a warning message";
            PL.Log.info "This is an info message";
            PL.Log.debug "This is a debug message";
            PL.Log.log "This is a log message";
            PL.Log.elog PL.Notice "This is an elog notice"
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test]
    fn test_log_report_with_optional_fields() {
        Spi::run(
            r#"DO $$
            PL.Log.report PL.Notice
              ~detail:"Detailed explanation"
              ~hint:"Try doing this instead"
              ~sqlstate:"01000"
              ~schema_name:"public"
              ~table_name:"my_table"
              ~column_name:"my_col"
              ~datatype_name:"text"
              ~constraint_name:"my_check"
              "Notice with all diagnostic fields"
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test(error = "PL/OCaml execution failed")]
    fn test_log_error_uncaught_fails() {
        Spi::run(
            r#"DO $$
            PL.Log.error ~detail:"Error details" ~hint:"Error hint" "Fatal custom error"
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test]
    fn test_log_error_catchable() {
        Spi::run(
            r#"DO $$
            try
              PL.Log.error "Caught error"
            with Failure _ -> ()
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test]
    fn test_quote_literal() {
        Spi::run(
            r#"DO $$
            let s1 = PL.Quote.literal "hello" in
            if s1 <> "'hello'" then failwith (Printf.sprintf "unexpected s1: %s" s1);

            let s2 = PL.Quote.literal "O'Reilly" in
            if s2 <> "'O''Reilly'" then failwith (Printf.sprintf "unexpected s2: %s" s2);

            let s3 = PL.Quote.literal "tab\tnewline\n" in
            let res = PL.execute (Printf.sprintf "SELECT %s AS val" s3) in
            match res.rows.(0) with
            | [("val", PL.String "tab\tnewline\n")] -> ()
            | _ -> failwith "unexpected query result for quoted literal"
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test]
    fn test_quote_nullable() {
        Spi::run(
            r#"DO $$
            let n1 = PL.Quote.nullable None in
            if n1 <> "NULL" then failwith (Printf.sprintf "unexpected n1: %s" n1);

            let n2 = PL.Quote.nullable (Some "hello") in
            if n2 <> "'hello'" then failwith (Printf.sprintf "unexpected n2: %s" n2);

            let n3 = PL.Quote.nullable (Some "O'Reilly") in
            if n3 <> "'O''Reilly'" then failwith (Printf.sprintf "unexpected n3: %s" n3);

            let res_null = PL.execute (Printf.sprintf "SELECT %s AS val" n1) in
            match res_null.rows.(0) with
            | [("val", PL.Null)] -> ()
            | _ -> failwith "expected NULL from quote_nullable None";

            let res_val = PL.execute (Printf.sprintf "SELECT %s AS val" n2) in
            match res_val.rows.(0) with
            | [("val", PL.String "hello")] -> ()
            | _ -> failwith "expected string from quote_nullable (Some ...)"
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test]
    fn test_quote_ident() {
        Spi::run(
            r#"DO $$
            let id1 = PL.Quote.ident "simple_col" in
            if id1 <> "simple_col" then failwith (Printf.sprintf "unexpected id1: %s" id1);

            let id2 = PL.Quote.ident "My Table" in
            if id2 <> "\"My Table\"" then failwith (Printf.sprintf "unexpected id2: %s" id2);

            let id3 = PL.Quote.ident "select" in
            if id3 <> "\"select\"" then failwith (Printf.sprintf "unexpected id3: %s" id3);

            let tname = PL.Quote.ident "Temp Quoted Table" in
            let cname = PL.Quote.ident "Quoted Col" in
            let _ = PL.execute (Printf.sprintf "CREATE TEMP TABLE %s (%s int)" tname cname) in
            let _ = PL.execute (Printf.sprintf "INSERT INTO %s (%s) VALUES (999)" tname cname) in
            let res = PL.execute (Printf.sprintf "SELECT %s FROM %s" cname tname) in
            match res.rows.(0) with
            | [("Quoted Col", PL.Int 999)] -> ()
            | _ -> failwith "unexpected row contents from quoted table/col query"
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test]
    fn test_spi_cursor_query_and_fetch() {
        Spi::run(
            r#"DO $$
            let cur = PL.cursor "SELECT generate_series(1, 5) AS n" in

            let r1 = PL.fetch cur 2 in
            if r1.nrows <> 2 then failwith (Printf.sprintf "expected 2 rows, got %d" r1.nrows);
            (match r1.rows.(0), r1.rows.(1) with
            | [("n", PL.Int 1)], [("n", PL.Int 2)] -> ()
            | _ -> failwith "unexpected contents for batch 1");

            let r2 = PL.fetch cur 2 in
            if r2.nrows <> 2 then failwith (Printf.sprintf "expected 2 rows, got %d" r2.nrows);
            (match r2.rows.(0), r2.rows.(1) with
            | [("n", PL.Int 3)], [("n", PL.Int 4)] -> ()
            | _ -> failwith "unexpected contents for batch 2");

            let r3 = PL.fetch cur 2 in
            if r3.nrows <> 1 then failwith (Printf.sprintf "expected 1 row, got %d" r3.nrows);
            (match r3.rows.(0) with
            | [("n", PL.Int 5)] -> ()
            | _ -> failwith "unexpected contents for batch 3");

            let r4 = PL.fetch cur 2 in
            if r4.nrows <> 0 then failwith (Printf.sprintf "expected 0 rows, got %d" r4.nrows);

            PL.close cur
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test]
    fn test_spi_cursor_plan_and_fetch() {
        Spi::run(
            r#"DO $$
            let _ = PL.execute "CREATE TEMP TABLE test_cursor_plan_tbl (id int, val text)" in
            let _ = PL.execute "INSERT INTO test_cursor_plan_tbl VALUES (10, 'ten'), (20, 'twenty'), (30, 'thirty'), (40, 'forty')" in

            let plan = PL.prepare "SELECT id, val FROM test_cursor_plan_tbl WHERE id >= $1 ORDER BY id" [|"int"|] in
            let cur = PL.cursor_plan plan [|PL.Int 20|] in

            let r1 = PL.fetch cur 2 in
            if r1.nrows <> 2 then failwith "expected 2 rows";
            (match r1.rows.(0), r1.rows.(1) with
            | [("id", PL.Int 20); ("val", PL.String "twenty")], [("id", PL.Int 30); ("val", PL.String "thirty")] -> ()
            | _ -> failwith "unexpected contents for plan batch 1");

            let r2 = PL.fetch cur 2 in
            if r2.nrows <> 1 then failwith "expected 1 row";
            (match r2.rows.(0) with
            | [("id", PL.Int 40); ("val", PL.String "forty")] -> ()
            | _ -> failwith "unexpected contents for plan batch 2");

            PL.close cur
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test(error = "PL/OCaml execution failed")]
    fn test_spi_cursor_fetch_after_close_fails() {
        Spi::run(
            r#"DO $$
            let cur = PL.cursor "SELECT 1 AS n" in
            PL.close cur;
            let _ = PL.fetch cur 1 in
            ()
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test]
    fn test_spi_cursor_close_idempotent() {
        Spi::run(
            r#"DO $$
            let cur = PL.cursor "SELECT 1 AS n" in
            PL.close cur;
            PL.close cur;
            PL.close cur
            $$ LANGUAGE plocamlu;"#,
        )
        .expect("DO block failed");
    }

    #[pg_test]
    fn test_call_handler_scalar_int() {
        Spi::run(
            r#"
            CREATE FUNCTION test_add_fn(a int, b int) RETURNS int LANGUAGE plocamlu AS $$
                match a, b with
                | PL.Int x, PL.Int y -> x + y
                | _ -> 0
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        let sum = Spi::get_one::<i32>("SELECT test_add_fn(18, 24);")
            .expect("SELECT failed")
            .expect("result is null");
        assert_eq!(sum, 42);
    }

    #[pg_test]
    fn test_call_handler_scalar_text() {
        Spi::run(
            r#"
            CREATE FUNCTION test_concat_fn(s1 text, s2 text) RETURNS text LANGUAGE plocamlu AS $$
                match s1, s2 with
                | PL.String a, PL.String b -> a ^ " " ^ b
                | _ -> ""
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        let res = Spi::get_one::<String>("SELECT test_concat_fn('hello', 'world');")
            .expect("SELECT failed")
            .expect("result is null");
        assert_eq!(res, "hello world");
    }

    #[pg_test]
    fn test_call_handler_scalar_bool() {
        Spi::run(
            r#"
            CREATE FUNCTION test_is_even_fn(n int) RETURNS bool LANGUAGE plocamlu AS $$
                match n with
                | PL.Int x -> x mod 2 = 0
                | _ -> false
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        let is_even_4 = Spi::get_one::<bool>("SELECT test_is_even_fn(4);")
            .expect("SELECT failed")
            .expect("result is null");
        assert!(is_even_4);

        let is_even_5 = Spi::get_one::<bool>("SELECT test_is_even_fn(5);")
            .expect("SELECT failed")
            .expect("result is null");
        assert!(!is_even_5);
    }

    #[pg_test]
    fn test_call_handler_scalar_float8() {
        Spi::run(
            r#"
            CREATE FUNCTION test_multiply_floats(a float8, b float8) RETURNS float8 LANGUAGE plocamlu AS $$
                match a, b with
                | PL.Float x, PL.Float y -> x *. y
                | _ -> 0.0
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        let product = Spi::get_one::<f64>("SELECT test_multiply_floats(2.5, 4.0);")
            .expect("SELECT failed")
            .expect("result is null");
        assert!((product - 10.0).abs() < 1e-9);
    }

    #[pg_test]
    fn test_call_handler_returning_datum() {
        Spi::run(
            r#"
            CREATE FUNCTION test_ret_datum(n int) RETURNS int LANGUAGE plocamlu AS $$
                match n with
                | PL.Int x -> PL.Int (x * 10)
                | _ -> PL.Null
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        let res = Spi::get_one::<i32>("SELECT test_ret_datum(7);")
            .expect("SELECT failed")
            .expect("result is null");
        assert_eq!(res, 70);
    }

    #[pg_test]
    fn test_call_handler_no_args() {
        Spi::run(
            r#"
            CREATE FUNCTION test_const_answer() RETURNS int LANGUAGE plocamlu AS $$
                42
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        let res = Spi::get_one::<i32>("SELECT test_const_answer();")
            .expect("SELECT failed")
            .expect("result is null");
        assert_eq!(res, 42);
    }

    #[pg_test]
    fn test_call_handler_unnamed_args() {
        Spi::run(
            r#"
            CREATE FUNCTION test_unnamed_args(int, int) RETURNS int LANGUAGE plocamlu AS $$
                match arg1, arg2 with
                | PL.Int x, PL.Int y -> x * y
                | _ -> 0
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        let res = Spi::get_one::<i32>("SELECT test_unnamed_args(6, 7);")
            .expect("SELECT failed")
            .expect("result is null");
        assert_eq!(res, 42);
    }

    #[pg_test]
    fn test_call_handler_spi_query() {
        Spi::run(
            r#"
            CREATE FUNCTION test_fn_query(n int) RETURNS int LANGUAGE plocamlu AS $$
                let res = PL.execute (Printf.sprintf "SELECT %d * 3 AS res" (match n with PL.Int x -> x | _ -> 0)) in
                match res.rows.(0) with
                | [("res", PL.Int v)] -> v
                | _ -> 0
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        let res = Spi::get_one::<i32>("SELECT test_fn_query(5);")
            .expect("SELECT failed")
            .expect("result is null");
        assert_eq!(res, 15);
    }

    #[pg_test]
    fn test_call_handler_spi_plan() {
        Spi::run(
            r#"
            CREATE FUNCTION test_fn_plan(factor int) RETURNS int LANGUAGE plocamlu AS $$
                let plan = PL.prepare "SELECT $1::int * 4 AS val" [|"int4"|] in
                let res = PL.execute_plan plan [|factor|] in
                match res.rows.(0) with
                | [("val", PL.Int n)] -> n
                | _ -> 0
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        let res = Spi::get_one::<i32>("SELECT test_fn_plan(10);")
            .expect("SELECT failed")
            .expect("result is null");
        assert_eq!(res, 40);
    }

    #[pg_test]
    fn test_call_handler_procedure() {
        Spi::run(
            r#"
            CREATE TABLE test_proc_tbl (id int);

            CREATE PROCEDURE test_insert_proc(v int) LANGUAGE plocamlu AS $$
                match v with
                | PL.Int n ->
                    let q = Printf.sprintf "INSERT INTO test_proc_tbl VALUES (%d)" n in
                    ignore (PL.execute q)
                | _ -> ()
            $$;

            CALL test_insert_proc(99);
            "#,
        )
        .expect("procedure test failed");

        let count = Spi::get_one::<i64>("SELECT count(*) FROM test_proc_tbl WHERE id = 99;")
            .expect("SELECT failed")
            .expect("count is null");
        assert_eq!(count, 1);
    }

    #[pg_test]
    fn test_call_handler_null_input_and_output() {
        Spi::run(
            r#"
            CREATE FUNCTION test_null_fn(s text) RETURNS text LANGUAGE plocamlu AS $$
                match s with
                | PL.Null -> PL.String "was null"
                | PL.String "make_null" -> PL.Null
                | PL.String str -> PL.String ("not null: " ^ str)
                | _ -> PL.Null
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        let res_from_null = Spi::get_one::<String>("SELECT test_null_fn(NULL);")
            .expect("SELECT failed")
            .expect("result is null");
        assert_eq!(res_from_null, "was null");

        let res_to_null =
            Spi::get_one::<String>("SELECT test_null_fn('make_null');").expect("SELECT failed");
        assert_eq!(res_to_null, None);

        let res_regular = Spi::get_one::<String>("SELECT test_null_fn('hello');")
            .expect("SELECT failed")
            .expect("result is null");
        assert_eq!(res_regular, "not null: hello");
    }

    #[pg_test]
    fn test_call_handler_multiple_invocations() {
        Spi::run(
            r#"
            CREATE FUNCTION test_fib(n int) RETURNS int LANGUAGE plocamlu AS $$
                let rec fib = function
                  | 0 -> 0
                  | 1 -> 1
                  | n -> fib (n - 1) + fib (n - 2)
                in
                match n with
                | PL.Int x -> fib x
                | _ -> 0
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        let res = Spi::get_one::<i32>("SELECT test_fib(10);")
            .expect("SELECT failed")
            .expect("result is null");
        assert_eq!(res, 55);

        let res_table =
            Spi::get_one::<i32>("SELECT sum(test_fib(i))::int FROM generate_series(0, 6) AS i;")
                .expect("SELECT failed")
                .expect("result is null");
        // fib(0..6) = 0 + 1 + 1 + 2 + 3 + 5 + 8 = 20
        assert_eq!(res_table, 20);
    }

    #[pg_test(error = "PL/OCaml execution failed")]
    fn test_call_handler_error_fails() {
        Spi::run(
            r#"
            CREATE FUNCTION test_failing_fn() RETURNS int LANGUAGE plocamlu AS $$
                failwith "intentional function error"
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        Spi::get_one::<i32>("SELECT test_failing_fn();").expect("SELECT failed");
    }

    #[pg_test]
    fn test_call_handler_sd_storage() {
        Spi::run(
            r#"
            CREATE FUNCTION test_sd_counter() RETURNS int LANGUAGE plocamlu AS $$
                let cur = match Hashtbl.find_opt sd "count" with
                  | Some v -> (Obj.obj v : int) + 1
                  | None -> 1
                in
                Hashtbl.replace sd "count" (Obj.repr cur);
                cur
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        let c1 = Spi::get_one::<i32>("SELECT test_sd_counter();")
            .expect("SELECT failed")
            .expect("null result");
        assert_eq!(c1, 1);

        let c2 = Spi::get_one::<i32>("SELECT test_sd_counter();")
            .expect("SELECT failed")
            .expect("null result");
        assert_eq!(c2, 2);

        let c3 = Spi::get_one::<i32>("SELECT test_sd_counter();")
            .expect("SELECT failed")
            .expect("null result");
        assert_eq!(c3, 3);
    }

    #[pg_test]
    fn test_call_handler_gd_storage() {
        Spi::run(
            r#"
            CREATE FUNCTION test_gd_setter(k text, v text) RETURNS void LANGUAGE plocamlu AS $$
                match k, v with
                | PL.String key, PL.String value ->
                    Hashtbl.replace PL.gd key (Obj.repr value)
                | _ -> ()
            $$;

            CREATE FUNCTION test_gd_getter(k text) RETURNS text LANGUAGE plocamlu AS $$
                match k with
                | PL.String key ->
                    (match Hashtbl.find_opt PL.gd key with
                     | Some v -> (Obj.obj v : string)
                     | None -> "not found")
                | _ -> "bad key"
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        Spi::run("SELECT test_gd_setter('greeting', 'hello from GD');").expect("setter failed");
        let val = Spi::get_one::<String>("SELECT test_gd_getter('greeting');")
            .expect("getter failed")
            .expect("null result");
        assert_eq!(val, "hello from GD");
    }

    #[pg_test]
    fn test_call_handler_create_or_replace_cache_invalidation() {
        Spi::run(
            r#"
            CREATE FUNCTION test_cached_repl(x int) RETURNS int LANGUAGE plocamlu AS $$
                match x with PL.Int n -> n * 2 | _ -> 0
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        let v1 = Spi::get_one::<i32>("SELECT test_cached_repl(10);")
            .expect("SELECT failed")
            .expect("null result");
        assert_eq!(v1, 20);

        // Replace the function definition with new behavior
        Spi::run(
            r#"
            CREATE OR REPLACE FUNCTION test_cached_repl(x int) RETURNS int LANGUAGE plocamlu AS $$
                match x with PL.Int n -> n * 10 | _ -> 0
            $$;
            "#,
        )
        .expect("CREATE OR REPLACE FUNCTION failed");

        let v2 = Spi::get_one::<i32>("SELECT test_cached_repl(10);")
            .expect("SELECT failed")
            .expect("null result");
        assert_eq!(v2, 100);
    }

    #[pg_test]
    fn test_call_handler_reentrant_recursion() {
        Spi::run(
            r#"
            CREATE FUNCTION test_reentrant_b(factor int, num int) RETURNS int LANGUAGE plocamlu AS $$
                match factor, num with
                | PL.Int f, PL.Int v -> f * v
                | _ -> 0
            $$;

            CREATE FUNCTION test_reentrant_a(x int) RETURNS int LANGUAGE plocamlu AS $$
                match x with
                | PL.Int n ->
                    let q = Printf.sprintf "SELECT test_reentrant_b(%d, %d) AS res" (n + 1) (n + 2) in
                    let res = PL.execute q in
                    (match res.rows.(0) with
                     | [("res", PL.Int r)] -> r
                     | _ -> 0)
                | _ -> 0
            $$;
            "#,
        )
        .expect("CREATE FUNCTION failed");

        // (3 + 1) * (3 + 2) = 4 * 5 = 20
        let res = Spi::get_one::<i32>("SELECT test_reentrant_a(3);")
            .expect("SELECT failed")
            .expect("null result");
        assert_eq!(res, 20);
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

    #[test]
    fn test_client_log_and_notice() {
        pgrx_tests::run_test("test_extension_loads", None, vec![])
            .expect("failed to run test setup");

        let (mut base_client, _) = pgrx_tests::client().expect("failed to connect client");
        let row = base_client
            .query_one(
                "SELECT inet_server_port() AS port, current_user AS usr, current_database() AS db",
                &[],
            )
            .expect("query port failed");
        let port: i32 = row.get("port");
        let user: String = row.get("usr");
        let db: String = row.get("db");
        drop(base_client);

        let notices = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let notices_clone = notices.clone();

        let mut client = postgres::Config::new()
            .host("127.0.0.1")
            .port(port as u16)
            .user(&user)
            .dbname(&db)
            .notice_callback(move |err| {
                notices_clone.lock().unwrap().push(err);
            })
            .connect(postgres::NoTls)
            .expect("failed to connect with notice_callback");

        client
            .simple_query("SET client_min_messages TO 'NOTICE';")
            .expect("set client_min_messages failed");

        client
            .simple_query(
                r#"
                DO $$
                PL.Log.notice "Client notice from PL/OCaml";
                PL.Log.warning "Client warning from PL/OCaml";
                PL.Log.info "Client info from PL/OCaml"
                $$ LANGUAGE plocamlu;
                "#,
            )
            .expect("DO block with log statements failed");

        let captured = notices.lock().unwrap();
        let notice_obj = captured
            .iter()
            .find(|n| n.message() == "Client notice from PL/OCaml")
            .expect("missing notice");
        assert_eq!(notice_obj.severity(), "NOTICE");

        let warning_obj = captured
            .iter()
            .find(|n| n.message() == "Client warning from PL/OCaml")
            .expect("missing warning");
        assert_eq!(warning_obj.severity(), "WARNING");

        let info_obj = captured
            .iter()
            .find(|n| n.message() == "Client info from PL/OCaml")
            .expect("missing info");
        assert_eq!(info_obj.severity(), "INFO");
    }

    #[test]
    fn test_client_log_diagnostics() {
        pgrx_tests::run_test("test_extension_loads", None, vec![])
            .expect("failed to run test setup");

        let (mut base_client, _) = pgrx_tests::client().expect("failed to connect client");
        let row = base_client
            .query_one(
                "SELECT inet_server_port() AS port, current_user AS usr, current_database() AS db",
                &[],
            )
            .expect("query port failed");
        let port: i32 = row.get("port");
        let user: String = row.get("usr");
        let db: String = row.get("db");
        drop(base_client);

        let notices = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let notices_clone = notices.clone();

        let mut client = postgres::Config::new()
            .host("127.0.0.1")
            .port(port as u16)
            .user(&user)
            .dbname(&db)
            .notice_callback(move |err| {
                notices_clone.lock().unwrap().push(err);
            })
            .connect(postgres::NoTls)
            .expect("failed to connect with notice_callback");

        client
            .simple_query("SET client_min_messages TO 'NOTICE';")
            .expect("set client_min_messages failed");

        client
            .simple_query(
                r#"
                DO $$
                PL.Log.report PL.Notice
                  ~detail:"Detailed explanation"
                  ~hint:"Try doing this instead"
                  ~sqlstate:"01000"
                  ~schema_name:"public"
                  ~table_name:"my_table"
                  ~column_name:"my_col"
                  ~datatype_name:"text"
                  ~constraint_name:"my_check"
                  "Notice with diagnostic fields"
                $$ LANGUAGE plocamlu;
                "#,
            )
            .expect("DO block with diagnostic log report failed");

        let captured = notices.lock().unwrap();
        let notice = captured
            .iter()
            .find(|n| n.message() == "Notice with diagnostic fields")
            .expect("missing notice with diagnostic fields");

        assert_eq!(notice.severity(), "NOTICE");
        assert_eq!(notice.detail(), Some("Detailed explanation"));
        assert_eq!(notice.hint(), Some("Try doing this instead"));
        assert_eq!(notice.code().code(), "01000");
        assert_eq!(notice.schema(), Some("public"));
        assert_eq!(notice.table(), Some("my_table"));
        assert_eq!(notice.column(), Some("my_col"));
        assert_eq!(notice.datatype(), Some("text"));
        assert_eq!(notice.constraint(), Some("my_check"));
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
