//! `based gen sql|client|openapi`: emit target artifacts from the checked schema —
//! SQL DDL, a typed client module, or an OpenAPI 3.1 contract.

use crate::error::{io_at, CliError};
use crate::project::load_checked;
use based_codegen::{client::ClientTarget, Dialect};
use std::path::Path;

pub fn cmd_gen_sql(root: &Path, out: Option<&Path>) -> Result<(), CliError> {
    let (project, schema, decls, _sources, _warnings) = load_checked(root)?;
    let dialect = Dialect::parse(&project.manifest.dialect);
    let fks = based_sema::ForeignKeys::parse(&project.manifest.schema.foreign_keys);
    // Schema DDL first, then the parameterized query templates.
    let mut sql = based_codegen::sql::ddl_with(&schema, dialect, fks);
    if !schema.queries.is_empty() {
        sql.push_str(
            "\n\n-- ============================== queries ==============================\n",
        );
        sql.push_str(&based_codegen::sql::dml::dml(&schema, &decls, dialect));
    }
    if !schema.mutations.is_empty() {
        sql.push_str(
            "\n\n-- ============================= mutations =============================\n",
        );
        sql.push_str(&based_codegen::sql::mutations::mutations(
            &schema, &decls, dialect,
        ));
    }
    match out {
        Some(path) => {
            std::fs::write(path, &sql).map_err(|e| io_at("writing", path, e))?;
            eprintln!("wrote {} ({} models)", path.display(), schema.models.len());
        }
        None => print!("{sql}"),
    }
    Ok(())
}

pub fn cmd_gen_client(root: &Path, out: Option<&Path>, embedded: bool) -> Result<(), CliError> {
    use based_codegen::client::ClientOptions;
    let (project, schema, decls, _sources, _warnings) = load_checked(root)?;
    let target = ClientTarget::parse(&project.manifest.client);
    // The compile-target dialect gates the one per-driver `adopt_*` bring-your-own
    // transaction constructor the embedded bridge emits (each names a concrete
    // `sqlx::Transaction<DB>`); a wire-only client never touches it.
    let dialect = embedded.then(|| Dialect::parse(&project.manifest.dialect));
    let opts = ClientOptions { embedded, dialect };
    // Format the emitted client so the written file matches `cargo fmt`: a re-`gen` is never
    // a whitespace diff, and the consumer's `cargo fmt` leaves the checked-in client alone.
    let code = based_codegen::client::format_rust(&based_codegen::client::client_with(
        &schema, &decls, target, opts,
    ));
    match out {
        Some(path) => {
            std::fs::write(path, &code).map_err(|e| io_at("writing", path, e))?;
            let n = schema.queries.len() + schema.mutations.len();
            eprintln!("wrote {} ({n} callable(s))", path.display());
        }
        None => print!("{code}"),
    }
    Ok(())
}

pub fn cmd_gen_openapi(root: &Path, out: Option<&Path>) -> Result<(), CliError> {
    let (_project, schema, decls, _sources, _warnings) = load_checked(root)?;
    let doc = based_codegen::openapi::openapi(&schema, &decls);
    match out {
        Some(path) => {
            std::fs::write(path, &doc).map_err(|e| io_at("writing", path, e))?;
            let n = schema.queries.len() + schema.mutations.len();
            eprintln!("wrote {} ({n} operation(s))", path.display());
        }
        None => print!("{doc}"),
    }
    Ok(())
}
