use std::process::Command;

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

    // Locate OCaml headers and libraries
    let ocaml_where = String::from_utf8(
        Command::new("ocamlc")
            .arg("-where")
            .output()
            .expect("failed to run ocamlc -where")
            .stdout,
    )
    .expect("invalid UTF-8 from ocamlc")
    .trim()
    .to_string();

    // dune generates bootstrap.c with embedded caml_startup + builtin primitives
    let ml_dir = format!("{}/ml", manifest_dir);
    let status = Command::new("dune")
        .current_dir(&ml_dir)
        .args(["build", "bootstrap.c"])
        .status()
        .expect("failed to run dune build");
    assert!(status.success(), "dune build bootstrap.c failed");

    // Compile the generated C to a PIC object file using PostgreSQL's toolchain
    // (PGXS-style). Our compiler uses global-dynamic TLS, compatible with shared
    // libraries — unlike ocamlc's default initial-exec.
    let pg_cc = String::from_utf8(
        Command::new("pg_config")
            .arg("--cc")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let pg_cflags = String::from_utf8(
        Command::new("pg_config")
            .arg("--cflags")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let pg_cppflags = String::from_utf8(
        Command::new("pg_config")
            .arg("--cppflags")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let c_file = format!("{}/_build/default/bootstrap.c", ml_dir);
    let o_file = format!("{}/bootstrap.o", out_dir);
    let status = Command::new(&pg_cc)
        .args(pg_cflags.split_whitespace())
        .args(pg_cppflags.split_whitespace())
        .args(["-fPIC", "-I", &ocaml_where, "-c", &c_file, "-o", &o_file])
        .status()
        .expect("failed to compile bootstrap.c");
    assert!(status.success(), "cc bootstrap.c failed");

    // Link bootstrap.o and the OCaml bytecode runtime into plocamlu.so
    println!("cargo:rustc-link-arg={}", o_file);
    println!("cargo:rustc-link-search=native={}", ocaml_where);
    println!("cargo:rustc-link-lib=camlrun_pic");
    println!("cargo:rustc-link-lib=pthread");
    println!("cargo:rustc-link-lib=m");
    println!("cargo:rustc-link-lib=dl");
    println!("cargo:rustc-link-lib=zstd");
}
