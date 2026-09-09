//! The `Select` SQL-builder state: per-callable configuration and dialect identifier quoting.

use super::*;

/// Accumulates joins as paths are resolved, so the final FROM/JOIN block reflects
/// every column any clause reached across. Shared by the read side (this module)
/// and the write side (`mutations`), which reuses `predicate`/`value` so a
/// mutation `where` lowers identically to a query `where`.
pub(crate) struct Select<'a> {
    pub(crate) schema: &'a CheckedSchema,
    /// The compile target. Drives identifier quoting (`` `x` `` vs `"x"`) and a few
    /// operator/literal spellings; everything else is portable across dialects.
    pub(crate) dialect: Dialect,
    pub(crate) root_alias: String,
    pub(crate) joins: Vec<Join>,
    /// path-prefix key (e.g. "placed_by", "address.city") -> join alias.
    pub(crate) seen: HashMap<String, String>,
    /// Named filters by name, so a `FilterCall` (or a bare atom naming a filter) can
    /// inline its body against the call-site model — the codegen mirror of the sema check.
    pub(crate) filters: HashMap<&'a str, &'a NamedFilter>,
    /// Filters currently mid-expansion; guards a self-referential filter from looping
    /// (sema permits recursion, so we must terminate on our own, like sema does).
    pub(crate) filter_stack: Vec<&'a str>,
    /// Named shapes by name (`full` excluded — it is per-model and never referenced by
    /// name), so a `field -> Shape` nest can expand the referenced body in place.
    pub(crate) shapes: HashMap<&'a str, &'a Shape>,
    /// Shape references currently mid-expansion; sema rejects reference cycles
    /// (`E0134`), so this only keeps codegen terminating on an unchecked schema.
    pub(crate) shape_stack: Vec<&'a str>,
    /// The `create … as name` step bindings reachable in an enclosing `tx`, keyed by
    /// binding name, so a `$name.field` reference resolves to that step's produced row.
    /// Reaches any prior step, not just the immediately preceding one. Empty outside a `tx`.
    pub(crate) bindings: HashMap<&'a str, BackCtx<'a>>,
    /// Whether to inject a *joined* scoped model's `@scope` into its join `ON`.
    /// True by default; set false for an `unscoped` callable, which opts out of
    /// *all* scope handling — the joined tables included, not just the root. The
    /// root/write-target `@scope` is injected by the caller (`lower_query` /
    /// `lower_write`), which already honours `unscoped`; this flag governs only the
    /// join-`ON` injection the resolver performs as it materializes each join.
    pub(crate) inject_scope: bool,
    /// The per-touched-model scope the *current callable* injects, from
    /// `RQuery`/`RMutation.scope_inject`. Keyed by model name; each entry is the
    /// chosen alternative's `(column, ctx_field)` terms. The root `WHERE`, the joined
    /// `ON`, and the create auto-set all read the terms for their model from here, so a
    /// callable naming one alternative injects a different predicate than one naming
    /// another. Empty for an `unscoped` callable (sema returns no injection).
    pub(crate) scope_inject: &'a [ScopeInject],
    /// Monotonic counter minting a distinct root alias (`s<n>_<table>`) for each to-many
    /// nested-array subquery, so a self-referential edge's child table never collides
    /// with the outer row's alias. Threaded through nested subqueries so siblings stay
    /// unique.
    pub(crate) sub_counter: usize,
    /// Render column operands **bare** (unqualified) instead of `alias.col`. Set only for
    /// an upsert's `ON CONFLICT DO UPDATE` / `ON DUPLICATE KEY UPDATE` SET clause, where a
    /// bare column names the existing row across all three dialects (a qualified reference
    /// is rejected or ambiguous there). Off everywhere else.
    pub(crate) bare_cols: bool,
    /// Interpret a leading-`incoming` operand (`incoming.<col>`) as the **proposed/incoming**
    /// row of a bulk upsert (`create … from … on conflict update`): Postgres/SQLite lower it
    /// to `excluded.<col>`, MySQL/MariaDB to `VALUES(<col>)`. Set only for a bulk upsert's SET
    /// clause; off everywhere else (there `incoming` is an ordinary path — sema forbids it).
    pub(crate) incoming: bool,
    /// Names of the query's `?` optional filter params. A `where` comparison whose RHS is one
    /// of these is present-guarded (`:{name}__present`) so an absent arg widens the leaf to
    /// TRUE (queries.md), composing correctly through `and`/`or`.
    pub(crate) optional_params: HashSet<String>,
}

/// A reachable `create … as name` tx step binding: the model it created, so a
/// `$name.field` reference resolves to the `:bref_<name>__<column>` value the bound
/// create's row read-back captures for that field.
#[derive(Clone)]
pub(crate) struct BackCtx<'a> {
    /// The bound create's model name, so `$name.field` resolves `field` to a physical
    /// column of that model.
    pub(crate) model: &'a str,
}

impl<'a> Select<'a> {
    pub(crate) fn new(
        schema: &'a CheckedSchema,
        decls: &'a [Decl],
        root: &RModel,
        dialect: Dialect,
    ) -> Self {
        let filters = decls
            .iter()
            .filter_map(|d| match d {
                Decl::Filter(f) => Some((f.name.node.as_str(), f)),
                _ => None,
            })
            .collect();
        let shapes = decls
            .iter()
            .filter_map(|d| match d {
                Decl::Shape(s) if s.name.node != "full" => Some((s.name.node.as_str(), s)),
                _ => None,
            })
            .collect();
        Select {
            schema,
            dialect,
            root_alias: root.table.clone(),
            joins: Vec::new(),
            seen: HashMap::new(),
            filters,
            filter_stack: Vec::new(),
            shapes,
            shape_stack: Vec::new(),
            bindings: HashMap::new(),
            inject_scope: true,
            scope_inject: &[],
            sub_counter: 0,
            bare_cols: false,
            incoming: false,
            optional_params: HashSet::new(),
        }
    }

    /// Render column operands bare (unqualified) — for an upsert conflict-update SET.
    pub(crate) fn with_bare_cols(mut self, bare: bool) -> Self {
        self.bare_cols = bare;
        self
    }

    /// Interpret `incoming.<col>` as the proposed row of a bulk upsert — for a bulk
    /// `create … from … on conflict update` SET clause.
    pub(crate) fn with_incoming(mut self, incoming: bool) -> Self {
        self.incoming = incoming;
        self
    }

    /// Attach the reachable tx step bindings so a `$name.field` in this statement's
    /// assigns resolves to the bound `create`'s produced row.
    pub(crate) fn with_bindings(mut self, bindings: HashMap<&'a str, BackCtx<'a>>) -> Self {
        self.bindings = bindings;
        self
    }

    /// Quote one identifier for the target dialect (`` `x` `` / `"x"`).
    pub(crate) fn q(&self, ident: &str) -> String {
        self.dialect.quote(ident)
    }

    /// A table reference, `@schema`-qualified when the model lives outside the default
    /// namespace (`` `schema`.`table` ``), else bare. Used at every FROM/JOIN/DML base.
    pub(crate) fn qt(&self, model: &RModel) -> String {
        self.dialect
            .quote_table(model.schema.as_deref(), &model.table)
    }

    /// A `table`.`column` qualified reference, quoted for the dialect.
    pub(crate) fn qcol(&self, table: &str, column: &str) -> String {
        self.dialect.qcol(table, column)
    }
}
