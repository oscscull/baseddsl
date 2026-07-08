//! Build step: run the compiler front end over `schema/` and emit the two generated
//! artifacts the app includes — the typed Rust client and the Postgres DDL — into
//! `OUT_DIR`. This is `based gen client` + `based gen sql` invoked as a library, so the
//! generated code is regenerated on every build and can never drift from `schema/*.bsl`.

use std::path::PathBuf;

use based_codegen::{client::ClientTarget, sql, Dialect};
use based_runtime::Compiled;

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let schema = manifest.join("schema");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));

    // Rebuild the artifacts whenever a schema file changes.
    println!("cargo:rerun-if-changed={}", schema.display());

    // `Compiled::load` is the same front end `based check`/`based serve` run: discover
    // `**/*.bsl`, parse, and typecheck. It bails on any error, so a broken schema fails
    // the build with the diagnostics — exactly what a user wants.
    let compiled = Compiled::load(&schema)
        .unwrap_or_else(|e| panic!("schema under {} failed to check: {e:?}", schema.display()));

    // The generated file carries an inner `#![allow(dead_code)]`, which `include!` rejects
    // (inner attributes must annotate an enclosing file/module, not an included fragment).
    // Drop it here; `main.rs` puts the same allow as an outer attribute on `mod client`.
    let client =
        based_codegen::client::client(&compiled.schema, &compiled.decls, ClientTarget::Rust)
            .replace("#![allow(dead_code)]\n", "");
    std::fs::write(out.join("client.rs"), client).expect("write client.rs");

    let ddl = sql::ddl(&compiled.schema, Dialect::Postgres);
    std::fs::write(out.join("schema.sql"), ddl).expect("write schema.sql");
}
