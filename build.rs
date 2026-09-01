use pgrx_pg_config::{PgConfig, Pgrx, SUPPORTED_VERSIONS};
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;

fn get_pg_config() -> PgConfig {
    if let Ok(pg_config) = PgConfig::from_env() {
        return pg_config;
    }

    let pgrx = Pgrx::from_config().expect("failed to load pgrx configuration");
    for pgver in SUPPORTED_VERSIONS() {
        if std::env::var(format!("CARGO_FEATURE_PG{}", pgver.major)).is_ok() {
            return pgrx
                .get(&format!("pg{}", pgver.major))
                .unwrap_or_else(|e| panic!("failed to get pg_config for pg{}: {}", pgver.major, e));
        }
    }

    pgrx.get("pg18").expect("failed to get default pg18 config")
}

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

    // Locate pg_config using pgrx
    let pg_config = get_pg_config();
    let pg_config_bin = pg_config
        .path()
        .unwrap_or_else(|| PathBuf::from("pg_config"));

    // Compile the generated C to a PIC object file using PostgreSQL's toolchain
    // (PGXS-style). Our compiler uses global-dynamic TLS, compatible with shared
    // libraries — unlike ocamlc's default initial-exec.
    let pg_cc = String::from_utf8(
        Command::new(&pg_config_bin)
            .arg("--cc")
            .output()
            .unwrap_or_else(|e| panic!("failed to run {} --cc: {}", pg_config_bin.display(), e))
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let pg_cflags = String::from_utf8(
        Command::new(&pg_config_bin)
            .arg("--cflags")
            .output()
            .unwrap_or_else(|e| panic!("failed to run {} --cflags: {}", pg_config_bin.display(), e))
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let pg_cppflags = String::from_utf8(
        Command::new(&pg_config_bin)
            .arg("--cppflags")
            .output()
            .unwrap_or_else(|e| {
                panic!("failed to run {} --cppflags: {}", pg_config_bin.display(), e)
            })
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

    // Read ocamlc -config for library dependencies
    let config_output = String::from_utf8(
        Command::new("ocamlc")
            .arg("-config")
            .output()
            .expect("failed to run ocamlc -config")
            .stdout,
    )
    .unwrap_or_default();

    // Link bootstrap.o and the OCaml bytecode runtime into plocamlu.so / plocamlu.dylib
    println!("cargo:rustc-link-arg={}", o_file);
    println!("cargo:rustc-link-search=native={}", ocaml_where);

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
        println!("cargo:rustc-link-search=native=/usr/local/lib");
    }

    println!("cargo:rustc-link-lib=camlrun_pic");

    let mut linked_libs = HashSet::new();
    for lib in ["pthread", "m"] {
        linked_libs.insert(lib.to_string());
        println!("cargo:rustc-link-lib={}", lib);
    }
    if target_os != "macos" {
        linked_libs.insert("dl".to_string());
        println!("cargo:rustc-link-lib=dl");
    }

    for line in config_output.lines() {
        if line.starts_with("bytecomp_c_libraries:")
            || line.starts_with("compression_c_libraries:")
        {
            let flags = line.splitn(2, ':').nth(1).unwrap_or("").trim();
            for flag in flags.split_whitespace() {
                if let Some(lib) = flag.strip_prefix("-l") {
                    if linked_libs.insert(lib.to_string()) {
                        println!("cargo:rustc-link-lib={}", lib);
                    }
                } else if let Some(search) = flag.strip_prefix("-L") {
                    println!("cargo:rustc-link-search=native={}", search);
                }
            }
        }
    }
}
