//! Loading a project into the runtime: the same front end as `based check`
//! (discover → parse → check), then codegen's query lowering, held in memory.
//!
//! The runtime never runs on a schema that does not check clean — a codegen
//! precondition (PLAN pipeline). `Compiled` is the served artifact: the resolved
//! schema, the AST (queries need their signatures for validation), and the lowered
//! SQL keyed by callable name.

use std::collections::HashMap;
use std::path::Path;

use based_ast::{Decl, FileId};
use based_codegen::sql::{lower_queries, LoweredQuery};
use based_diagnostics::{Diagnostic, Severity};
use based_parser::parse_file;
use based_sema::{check, CheckedSchema};

/// A project compiled and ready to serve: resolved schema + AST + lowered queries.
pub struct Compiled {
    pub schema: CheckedSchema,
    pub decls: Vec<Decl>,
    /// Lowered query SQL, keyed by callable name for O(1) request dispatch.
    pub queries: HashMap<String, LoweredQuery>,
}

/// How loading failed: either the front end reported errors (the schema is not
/// clean), or a file could not be read.
#[derive(Debug)]
pub enum LoadError {
    /// One or more parse/sema *errors* (warnings do not block). Rendered by the
    /// caller — the runtime holds the diagnostics, not the formatting.
    Check(Vec<Diagnostic>),
    Io(String),
}

impl Compiled {
    /// Discover `**/*.bsl` under `root`, parse + check them, and lower every query.
    /// Bails with [`LoadError::Check`] on any error diagnostic (a dirty schema must
    /// never reach codegen). Warnings are tolerated — they do not affect execution.
    pub fn load(root: &Path) -> Result<Compiled, LoadError> {
        let project = based_manifest::discover(root).map_err(LoadError::Check)?;

        let mut decls = Vec::new();
        let mut diags = Vec::new();
        for (i, f) in project.files.iter().enumerate() {
            let src = std::fs::read_to_string(&f.path)
                .map_err(|e| LoadError::Io(format!("{}: {e}", f.path.display())))?;
            match parse_file(&src, FileId(i as u32)) {
                Ok(sf) => decls.extend(sf.decls),
                Err(d) => diags.extend(d),
            }
        }
        if diags.iter().any(|d| d.severity == Severity::Error) {
            return Err(LoadError::Check(diags));
        }

        let (schema, sema_diags) = check(&decls);
        if sema_diags.iter().any(|d| d.severity == Severity::Error) {
            return Err(LoadError::Check(sema_diags));
        }

        Ok(Compiled::from_checked(schema, decls))
    }

    /// Build the served artifact from an already-checked schema + AST (the loader's
    /// tail; also the seam tests use to skip disk I/O).
    pub fn from_checked(schema: CheckedSchema, decls: Vec<Decl>) -> Compiled {
        let queries = lower_queries(&schema, &decls)
            .into_iter()
            .map(|q| (q.name.clone(), q))
            .collect();
        Compiled {
            schema,
            decls,
            queries,
        }
    }
}
