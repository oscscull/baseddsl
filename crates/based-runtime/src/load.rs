//! Loading a project into the runtime: the same front end as `based check`
//! (parse → check, from disk or embedded sources), then codegen's query lowering, held in memory.
//!
//! The runtime never runs on a schema that does not check clean — a codegen
//! precondition. `Compiled` is the served artifact: the resolved schema, the AST
//! (queries need their signatures for validation), and the lowered SQL keyed by
//! callable name.

use std::collections::HashMap;
use std::path::Path;

use based_ast::Decl;
use based_codegen::sql::{lower_mutations, lower_queries, LoweredMutation, LoweredQuery};
use based_codegen::Dialect;
use based_project::CheckedProject;
use based_sema::CheckedSchema;

/// Why a schema did not load: compiler diagnostics (rendered against their source
/// names) or an unreadable file. Warnings never block.
pub use based_project::Error as LoadError;

/// A project compiled and ready to serve: resolved schema + AST + lowered queries
/// and mutations (the read + write dispatch tables).
pub struct Compiled {
    pub schema: CheckedSchema,
    pub decls: Vec<Decl>,
    /// The compile target the SQL was lowered for. The runtime's named→positional
    /// scanner also branches on it (`?` vs `$n`), so it must equal the dialect the
    /// driver/`Backend` in use actually speaks (a deployment invariant).
    pub dialect: Dialect,
    /// Lowered query SQL, keyed by callable name for O(1) request dispatch.
    pub queries: HashMap<String, LoweredQuery>,
    /// Lowered mutation write statements, keyed by callable name (write dispatch).
    pub mutations: HashMap<String, LoweredMutation>,
    /// Test-fixture insert ops, compiled on first use; never part of dispatch.
    pub(crate) fixtures: crate::fixture::Fixtures,
}

impl Compiled {
    /// Discover `based.toml` and `**/*.bsl` under `root`, check them, and lower every
    /// callable. Fails on any error diagnostic; warnings are tolerated.
    pub fn load(root: &Path) -> Result<Self, LoadError> {
        based_project::load(root).map(Self::from_project)
    }

    /// Check `based.toml` text plus `(name, text)` sources compiled into the binary,
    /// exactly as [`Compiled::load`] checks files on disk. Diagnostics cite each `name`.
    pub fn from_sources(manifest: &str, files: &[(&str, &str)]) -> Result<Self, LoadError> {
        based_project::check_embedded(manifest, files).map(Self::from_project)
    }

    /// The shared tail: lower a checked project for its manifest's dialect.
    fn from_project(checked: CheckedProject) -> Self {
        let dialect = Dialect::parse(&checked.project.manifest.dialect);
        Self::from_checked(checked.schema, checked.declarations, dialect)
    }

    /// Build the served artifact from an already-checked schema + AST for a target
    /// `dialect` (the loader's tail; also the seam tests use to skip disk I/O). The
    /// SQL is lowered *and* later bound (`?` vs `$n`) for this dialect, so it must be
    /// the one the serving `Backend` speaks.
    pub fn from_checked(schema: CheckedSchema, decls: Vec<Decl>, dialect: Dialect) -> Self {
        let queries = lower_queries(&schema, &decls, dialect)
            .into_iter()
            .map(|q| (q.name.clone(), q))
            .collect();
        let mutations = lower_mutations(&schema, &decls, dialect)
            .into_iter()
            .map(|m| (m.name.clone(), m))
            .collect();
        Self {
            schema,
            decls,
            dialect,
            queries,
            mutations,
            fixtures: crate::fixture::Fixtures::default(),
        }
    }

    /// The `$ctx` field a request on this route shards on, or `None` when the callable's
    /// target model has no `@scope` (single-shard deployments route it to shard 0) or the
    /// callable is `unscoped` (a cross-scope read/write has no single owning shard).
    /// `is_mutation` picks the wire side. An unknown name is `None` — dispatch reports the
    /// 404, and an unroutable request never reaches a shard anyway.
    ///
    /// The shard key is the model's resolved scope owner field (read off the same `@scope`
    /// that filters rows), so the shard a row lives in and the shard its owner's requests
    /// route to share one source of truth — no hand-set, drift-prone config.
    /// Whether `name` is a declared `-> stream` query. The wire edge branches on it:
    /// a stream query's response is the NDJSON row stream, never the collected array.
    pub fn is_stream_query(&self, name: &str) -> bool {
        self.schema
            .queries
            .iter()
            .any(|q| q.name == name && q.stream)
    }

    /// The guard a mutation declares (`guard <name>`, auth.md Handle 3), or `None` for
    /// an unguarded mutation or an unknown/query name. Dispatch invokes the registered
    /// implementation before the write body.
    pub fn guard_of(&self, name: &str) -> Option<&str> {
        self.schema
            .mutations
            .iter()
            .find(|m| m.name == name)?
            .guard
            .as_deref()
    }

    /// Every `(mutation, guard)` pair the schema declares — the engine-build check
    /// walks this to refuse a schema whose guards are not all registered.
    pub fn declared_guards(&self) -> impl Iterator<Item = (&str, &str)> {
        self.schema
            .mutations
            .iter()
            .filter_map(|m| m.guard.as_deref().map(|g| (m.name.as_str(), g)))
    }

    pub fn shard_key_field(&self, is_mutation: bool, name: &str) -> Option<&str> {
        if is_mutation {
            self.schema
                .mutations
                .iter()
                .find(|m| m.name == name)?
                .shard_key
                .as_deref()
        } else {
            self.schema
                .queries
                .iter()
                .find(|q| q.name == name)?
                .shard_key
                .as_deref()
        }
    }
}
