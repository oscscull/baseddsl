//! The neutral snapshot data types (the diff baseline's in-memory shape) and their
//! small accessor impls. Built in [`super::from_schema`], serialized in [`super::render`],
//! and parsed in [`super::parse`].

/// The canonical, dialect-neutral snapshot of a resolved schema: the diff baseline.
/// Derived from a [`CheckedSchema`] ([`Snapshot::from_schema`]) or parsed back from
/// `schema.snap` text ([`Snapshot::parse`]); a [`diff`] compares two of these.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    /// Named scope declarations, sorted by name and rendered above
    /// the tables. A scope emits no DDL — it is an injected row-visibility filter in
    /// generated code — but it is recorded here so a change to the contract (added,
    /// dropped, renamed, or a term retyped) is captured in a reviewable migration and
    /// caught by the offline drift check.
    pub scopes: Vec<ScopeDeclSnap>,
    /// Tables, sorted by name — the stable order that makes a git diff readable.
    pub tables: Vec<TableSnap>,
    /// Declared renames (`@was`), captured so the diff emits a clean `rename` step
    /// instead of a data-losing drop+add and so `apply`/`render`/`verify` re-derive that
    /// rename from the stored snapshots (snapshot-authoritative). A
    /// rename hint lives only in the migration where the rename happened; it does not
    /// participate in the "is the current schema captured?" check (that uses [`diff`], so
    /// a spent `@was` — one whose old name is already gone — produces no step). Sorted.
    pub renames: Vec<Rename>,
}

/// One declared rename (`@was`), the diff-time bridge between an old and new physical
/// name. Persisted in `schema.snap` so the rename survives to `apply`/`render` without a
/// database round-trip.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rename {
    /// A model `@was("old_table")`: table `from` → `to`.
    Table { from: String, to: String },
    /// A field `@was("old_col")`: column `from` → `to` on `table` (the current table name).
    Column {
        table: String,
        from: String,
        to: String,
    },
}

/// A `scope Name (col: Type = $ctx.field, …)` decl, captured neutrally: the column, the
/// declared type (a model name or a neutral primitive), and the `$ctx` field each term
/// binds. The one place the scope column's — and `$ctx.field`'s — type lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeDeclSnap {
    pub name: String,
    /// Terms in declaration order.
    pub terms: Vec<ScopeTermSnap>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeTermSnap {
    /// The scope column (the field a governed model must carry).
    pub column: String,
    /// The declared type — a model name (a relation) or a neutral primitive spelling.
    pub ty: String,
    /// The `$ctx.<field>` the column binds to.
    pub ctx_field: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSnap {
    pub name: String,
    /// `@schema("…")` — the SQL schema (Postgres) / database (MySQL/MariaDB) the table lives
    /// in, or `None` for the default namespace. Recorded so a from-scratch `CREATE TABLE` is
    /// namespaced and a model *moving* schema diffs into an `alter schema` step.
    pub schema: Option<String>,
    /// `@soft_delete` column + its neutral mode (`timestamp`/`bool`), if any.
    pub soft_delete: Option<(String, String)>,
    /// `@created` engine-managed column (set on insert), if any.
    pub created: Option<String>,
    /// `@updated` engine-managed column (set on insert + every update), if any.
    pub updated: Option<String>,
    /// The model's `@scope` alternatives, each a set of scope names (DNF). One
    /// entry per `@scope` decorator — `@scope A, B` is one alternative `["A", "B"]`, two
    /// stacked `@scope` decorators are two alternatives. Canonicalized (names sorted
    /// within an alternative, alternatives sorted) so the diff is stable. Empty = unscoped.
    pub scope_alts: Vec<Vec<String>>,
    /// `@sort` terms as `(column, dir)` where dir is `asc`/`desc`, in declaration order.
    pub sort: Vec<(String, String)>,
    /// `@no_id` — a keyless legacy table (no `id` primary key). The diff renders no
    /// `PRIMARY KEY` for it.
    pub no_id: bool,
    /// The primary-key column(s) when they are not the default single `id`: a renamed `id`,
    /// a single-column `@key(field)`, or the ordered columns of a composite `@key(f1, f2, …)`.
    /// Empty = the default `id` (elided from the column list and re-synthesized) or a keyless
    /// (`@no_id`) table.
    pub pk: Vec<String>,
    /// Columns, sorted by name.
    pub columns: Vec<ColumnSnap>,
    /// Declared indexes, sorted by name.
    pub indexes: Vec<IndexSnap>,
    /// Resolved foreign-key constraints (the toml `foreign_keys` convention ⊕ per-relation
    /// `@fk`/`@no_fk`), one per constrained FK column, sorted by column. Recorded so
    /// adding / removing / changing an FK diffs into a migration step. Empty when the
    /// convention is `none` and nothing writes `@fk`.
    pub foreign_keys: Vec<ForeignKeySnap>,
}

/// One resolved foreign-key constraint: the local FK column(s), the referenced table + its
/// primary-key column(s), and the optional referential actions. One column each for a
/// single-column-key target; several (paired positionally, in key order) for a composite
/// key. Diffed by value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ForeignKeySnap {
    pub columns: Vec<String>,
    pub ref_table: String,
    /// The referenced table's `@schema("…")` namespace, if it lives outside the default
    /// one — so a cross-schema `REFERENCES` names `schema.table`. `None` = default namespace.
    pub ref_schema: Option<String>,
    pub ref_columns: Vec<String>,
    /// `cascade`/`restrict`/`set_null`/`no_action`, or `None` for the DB-default action.
    pub on_delete: Option<String>,
    pub on_update: Option<String>,
}

impl ForeignKeySnap {
    /// A stable label for a constraint/error message: the sole column, or a
    /// `(c1, c2)` tuple for a composite FK.
    pub fn label(&self) -> String {
        match self.columns.as_slice() {
            [c] => c.clone(),
            cols => format!("({})", cols.join(", ")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSnap {
    pub name: String,
    /// Neutral type family (`int`/`text`/`uuid`/`timestamp`/`date`/`bool`/`json`), a
    /// `[]` suffix for a to-many scalar.
    pub ty: String,
    pub nullable: bool,
    /// A `(default …)` value rendered as a neutral literal, if declared.
    pub default: Option<String>,
    pub unique: bool,
    /// The related model when this column is a forward relation's FK (`fk=<Model>`).
    pub fk: Option<String>,
    /// A **generated column**'s expression in neutral form (`generated=(price - discount)`),
    /// or `None` for an ordinary stored column. Recorded so adding, dropping, or changing a
    /// generated column's expression shows up as a diff.
    pub generated: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSnap {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    /// `using <method>` — the declared access method, or `None` for the dialect default.
    pub method: Option<String>,
    /// An opaque `@index raw("…")` body, in its canonical `raw(…)` spelling. When set,
    /// `columns` is empty and the diff compares this string.
    pub raw: Option<String>,
}

impl Snapshot {
    pub fn scope(&self, name: &str) -> Option<&ScopeDeclSnap> {
        self.scopes.iter().find(|s| s.name == name)
    }

    pub fn table(&self, name: &str) -> Option<&TableSnap> {
        self.tables.iter().find(|t| t.name == name)
    }
}

impl TableSnap {
    pub fn column(&self, name: &str) -> Option<&ColumnSnap> {
        self.columns.iter().find(|c| c.name == name)
    }
    pub fn index(&self, name: &str) -> Option<&IndexSnap> {
        self.indexes.iter().find(|i| i.name == name)
    }
}
