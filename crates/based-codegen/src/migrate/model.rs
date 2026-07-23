//! Neutral snapshot model: the diff baseline and its `schema.snap` text form.
//!
//! The dialect-neutral [`Snapshot`] type (tables, columns, indexes, scope decls),
//! built from a resolved [`CheckedSchema`] ([`Snapshot::from_schema`]), rendered to the
//! canonical `schema.snap` text ([`Snapshot::render`]), and parsed back
//! ([`Snapshot::parse`]) so a stored baseline round-trips for the drift check. Names no
//! dialect — SQL lives in [`super::sql`].

use based_ast::{DefaultVal, Literal, Primitive, SortDir, SortTerm};
use based_sema::{CheckedSchema, ForeignKeys, MemberKind, RModel, SoftDelete, SoftMode};
use std::fmt::Write as _;

// ---------- neutral snapshot model ----------------------------------------

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

/// One resolved foreign-key constraint: the local FK column, the referenced table + its
/// primary-key column, and the optional referential actions. Diffed by value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ForeignKeySnap {
    pub column: String,
    pub ref_table: String,
    pub ref_column: String,
    /// `cascade`/`restrict`/`set_null`/`no_action`, or `None` for the DB-default action.
    pub on_delete: Option<String>,
    pub on_update: Option<String>,
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
    /// Build the neutral snapshot from a resolved schema, under the default `foreign_keys`
    /// convention (`none`). Convenience over [`Snapshot::from_schema_with`] for callers
    /// (tests, from-scratch DDL that carries only explicit `@fk`s) that don't thread the
    /// manifest convention.
    pub fn from_schema(schema: &CheckedSchema) -> Snapshot {
        Snapshot::from_schema_with(schema, ForeignKeys::None)
    }

    /// Build the neutral snapshot from a resolved schema under a given `foreign_keys`
    /// convention. Pure and deterministic: tables, columns, indexes, and FK constraints are
    /// all sorted by name so nothing map-ordered leaks in.
    pub fn from_schema_with(schema: &CheckedSchema, fks: ForeignKeys) -> Snapshot {
        let mut tables: Vec<TableSnap> = schema
            .models
            .iter()
            .map(|m| table_snap(schema, m, fks))
            .collect();
        tables.sort_by(|a, b| a.name.cmp(&b.name));
        let mut scopes: Vec<ScopeDeclSnap> = schema.scopes.iter().map(scope_decl_snap).collect();
        scopes.sort_by(|a, b| a.name.cmp(&b.name));
        let mut renames = collect_renames(schema);
        renames.sort();
        Snapshot {
            scopes,
            tables,
            renames,
        }
    }

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

/// Is this column the universally-implicit default `id`? Such a column is elided
/// from the snapshot's column list and carried as an invariant; a model that declares a
/// non-default `id` (a different type, nullable, or unique) records it explicitly.
fn is_default_id(c: &ColumnSnap) -> bool {
    c.name == "id" && c.ty == "uuid" && !c.nullable && !c.unique && c.fk.is_none()
}

fn table_snap(schema: &CheckedSchema, model: &RModel, fks: ForeignKeys) -> TableSnap {
    let mut columns: Vec<ColumnSnap> = Vec::new();
    for mem in &model.members {
        match &mem.kind {
            MemberKind::Scalar {
                ty,
                optional,
                many,
                column,
                unique,
                default,
                enum_name,
                raw_type,
            } => columns.push(ColumnSnap {
                name: column.clone(),
                // An enum column captures its variants as `enum(v1,v2,…)` so a variant
                // add/remove is a diffable type change; it maps to text SQL (+ CHECK).
                // An opaque column carries its canonical `raw(…)` spelling, so a change
                // to the literal type string is an ordinary column-type diff.
                ty: raw_type
                    .as_ref()
                    .map(|r| r.canonical())
                    .or_else(|| enum_neutral_type(schema, enum_name.as_deref()))
                    .unwrap_or_else(|| neutral_type(*ty, *many)),
                nullable: *optional,
                default: default.as_ref().map(|dv| {
                    render_default(dv, enum_name.as_deref().and_then(|n| schema.enum_(n)))
                }),
                unique: *unique,
                fk: None,
            }),
            MemberKind::Forward {
                target,
                optional,
                fk_col,
                ..
            } => columns.push(ColumnSnap {
                // A relation is its FK column: its physical type is the target's
                // key type (default uuid), and it carries the related model so a
                // retyped/dropped relation reads as an add/drop/alter of `<field>_id`.
                name: fk_col.clone(),
                ty: fk_type(schema, target),
                nullable: *optional,
                default: None,
                unique: false,
                fk: Some(target.clone()),
            }),
            // Inverse edges own no column — they emit no DDL, so no snapshot row.
            MemberKind::Inverse { .. } => {}
        }
    }
    columns.retain(|c| !is_default_id(c));
    columns.sort_by(|a, b| a.name.cmp(&b.name));

    let mut indexes = index_snaps(model);
    indexes.sort_by(|a, b| a.name.cmp(&b.name));

    let mut foreign_keys = foreign_key_snaps(schema, model, fks);
    foreign_keys.sort();

    TableSnap {
        name: model.table.clone(),
        soft_delete: model.soft_delete.as_ref().map(soft_delete_snap),
        created: model.created.clone(),
        updated: model.updated.clone(),
        scope_alts: canonical_scope_alts(&model.scope_alts),
        sort: model.sort.iter().map(sort_term).collect(),
        no_id: model.no_id,
        columns,
        indexes,
        foreign_keys,
    }
}

/// The resolved FK constraints on a model's forward relations under the convention. A
/// relation whose target is keyless (`@no_id`, already `E0265`) contributes none — there is
/// no primary key to reference.
pub fn foreign_key_snaps(
    schema: &CheckedSchema,
    model: &RModel,
    fks: ForeignKeys,
) -> Vec<ForeignKeySnap> {
    let mut out = Vec::new();
    for mem in &model.members {
        let MemberKind::Forward { target, fk_col, .. } = &mem.kind else {
            continue;
        };
        let Some(resolved) = model.resolved_fk(mem, fks) else {
            continue;
        };
        let Some(ref_column) = target_pk_column(schema, target) else {
            continue;
        };
        let ref_table = schema
            .model(target)
            .map(|t| t.table.clone())
            .unwrap_or_else(|| target.clone());
        out.push(ForeignKeySnap {
            column: fk_col.clone(),
            ref_table,
            ref_column,
            on_delete: resolved.on_delete.map(|a| a.snap().to_string()),
            on_update: resolved.on_update.map(|a| a.snap().to_string()),
        });
    }
    out
}

/// The physical primary-key column of a relation target (`id`, or its `(column "…")`
/// override). `None` when the target is missing or keyless (`@no_id`).
pub fn target_pk_column(schema: &CheckedSchema, target: &str) -> Option<String> {
    let t = schema.model(target)?;
    if t.no_id {
        return None;
    }
    Some(
        t.member("id")
            .map(|m| m.physical_col().to_string())
            .unwrap_or_else(|| "id".to_string()),
    )
}

/// The declared renames (`@was`) across the schema: model-level `@was` → a table rename,
/// field-level `@was` → a column rename on that model's (current) table. The old name
/// lives only in a prior snapshot; the diff matches it there.
fn collect_renames(schema: &CheckedSchema) -> Vec<Rename> {
    let mut out = Vec::new();
    for m in &schema.models {
        if let Some(old) = &m.was {
            out.push(Rename::Table {
                from: old.clone(),
                to: m.table.clone(),
            });
        }
        for mem in &m.members {
            if let Some(old) = &mem.was {
                out.push(Rename::Column {
                    table: m.table.clone(),
                    from: old.clone(),
                    to: mem.physical_col().to_string(),
                });
            }
        }
    }
    out
}

/// A resolved scope decl → its neutral snapshot form. The term type is a model name for a
/// relation-typed scope column, or the neutral primitive spelling for a scalar one.
fn scope_decl_snap(scope: &based_sema::RScope) -> ScopeDeclSnap {
    ScopeDeclSnap {
        name: scope.name.clone(),
        terms: scope
            .terms
            .iter()
            .map(|t| ScopeTermSnap {
                column: t.column.clone(),
                ty: scope_ty(&t.ty),
                ctx_field: t.ctx_field.clone(),
            })
            .collect(),
    }
}

/// A scope term's declared type as neutral text: a relation is its model name; a scalar is
/// the neutral primitive spelling (`Id` folds to `uuid`, matching the column type map).
fn scope_ty(ty: &based_sema::CtxField) -> String {
    match ty {
        based_sema::CtxField::Relation(m) => m.clone(),
        based_sema::CtxField::Scalar(p) => neutral_type(*p, false),
    }
}

/// Canonicalize a model's `@scope` DNF alternatives for a stable, diff-friendly snapshot:
/// sort names within each alternative, then sort and dedup the alternatives themselves.
fn canonical_scope_alts(alts: &[Vec<String>]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = alts
        .iter()
        .map(|alt| {
            let mut a = alt.clone();
            a.sort();
            a
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Declared `@index`es resolved to physical columns and their stable `idx_`/`uq_`
/// name. Mirrors `sql::ddl`'s naming so the snapshot's index identity matches the
/// generated DDL exactly — a non-unique index on a soft-delete model prepends the
/// tombstone column (predicate-leading).
fn index_snaps(model: &RModel) -> Vec<IndexSnap> {
    model
        .indexes
        .iter()
        .map(|idx| {
            if let Some(spec) = &idx.raw {
                return IndexSnap {
                    name: crate::sql::raw_index_name(&model.table, spec),
                    columns: Vec::new(),
                    unique: false,
                    method: None,
                    raw: Some(spec.canonical()),
                };
            }
            let mut fields = idx.columns.clone();
            if !idx.unique {
                if let Some(sd) = &model.soft_delete {
                    if fields.first() != Some(&sd.field) {
                        fields.insert(0, sd.field.clone());
                    }
                }
            }
            let cols: Vec<String> = fields.iter().map(|c| physical_col(model, c)).collect();
            IndexSnap {
                name: index_name(if idx.unique { "uq" } else { "idx" }, &model.table, &cols),
                columns: cols,
                unique: idx.unique,
                method: idx.method.clone(),
                raw: None,
            }
        })
        .collect()
}

fn soft_delete_snap(sd: &SoftDelete) -> (String, String) {
    let mode = match sd.mode {
        SoftMode::Timestamp => "timestamp",
        SoftMode::Bool => "bool",
    };
    (sd.field.clone(), mode.to_string())
}

fn sort_term(t: &SortTerm) -> (String, String) {
    let col = t
        .path
        .segments
        .iter()
        .map(|s| s.node.clone())
        .collect::<Vec<_>>()
        .join(".");
    let dir = match t.dir {
        SortDir::Asc => "asc",
        SortDir::Desc => "desc",
    };
    (col, dir.to_string())
}

/// Physical column for a field: a scalar's column, or a forward relation's FK.
fn physical_col(model: &RModel, field: &str) -> String {
    match model.member(field).map(|m| &m.kind) {
        Some(MemberKind::Scalar { column, .. }) => column.clone(),
        Some(MemberKind::Forward { fk_col, .. }) => fk_col.clone(),
        _ => field.to_string(),
    }
}

/// Stable, readable index name: `<prefix>_<table>_<col1>_<col2>` (mirrors `sql::ddl`).
pub(crate) fn index_name(prefix: &str, table: &str, columns: &[String]) -> String {
    let mut name = format!("{prefix}_{table}");
    for c in columns {
        name.push('_');
        name.push_str(c);
    }
    name
}

/// Neutral type family for a primitive (`Id` folds to `uuid`). A to-many scalar
/// gets a `[]` suffix — it has no columnar form and rides as a JSON array in DDL, but
/// the snapshot records the neutral intent so a change to it still diffs.
fn neutral_type(ty: Primitive, many: bool) -> String {
    let base = match ty {
        Primitive::Text => "text",
        Primitive::Int => "int",
        Primitive::Bool => "bool",
        Primitive::Timestamp => "timestamp",
        Primitive::Date => "date",
        Primitive::Json => "json",
        Primitive::Uuid | Primitive::Id => "uuid",
        Primitive::Float => "float",
        // `decimal(p,s)` in the snapshot so a precision/scale change diffs as an
        // `alter column` (the renderer parses it back through `neutral_sql_type`).
        Primitive::Decimal { precision, scale } => {
            let base = format!("decimal({precision},{scale})");
            return if many { format!("{base}[]") } else { base };
        }
    };
    if many {
        format!("{base}[]")
    } else {
        base.to_string()
    }
}

/// An enum column's neutral snapshot type — a single `schema.snap` token capturing its
/// kind + wire values so a variant add/remove OR a string↔int kind change is a diffable
/// column type change: `enum(v1,v2,…)` for a string enum (renderer → text + CHECK),
/// `enum:int(0,1,…)` for an int enum (renderer → integer + CHECK). `None` for a non-enum
/// column.
fn enum_neutral_type(schema: &CheckedSchema, enum_name: Option<&str>) -> Option<String> {
    use based_sema::{EnumKind, EnumValue};
    let en = schema.enum_(enum_name?)?;
    Some(match en.kind {
        EnumKind::Str => {
            let vals: Vec<&str> = en
                .variants
                .iter()
                .map(|v| match &v.value {
                    EnumValue::Str(s) => s.as_str(),
                    EnumValue::Int(_) => v.name.as_str(),
                })
                .collect();
            format!("enum({})", vals.join(","))
        }
        EnumKind::Int => {
            let vals: Vec<String> = en
                .variants
                .iter()
                .map(|v| match &v.value {
                    EnumValue::Int(n) => n.to_string(),
                    EnumValue::Str(_) => "0".to_string(),
                })
                .collect();
            format!("enum:int({})", vals.join(","))
        }
    })
}

/// A relation FK's neutral type: the target model's key type (default uuid).
fn fk_type(schema: &CheckedSchema, target: &str) -> String {
    match schema
        .model(target)
        .and_then(|m| m.member("id"))
        .map(|m| &m.kind)
    {
        Some(MemberKind::Scalar { ty, .. }) => neutral_type(*ty, false),
        _ => "uuid".to_string(),
    }
}

/// Render a `(default …)` value as a neutral literal for the snapshot. Dialect-neutral
/// by construction: `now()` is the neutral `now()`, never `CURRENT_TIMESTAMP`.
fn render_default(dv: &DefaultVal, en: Option<&based_sema::REnum>) -> String {
    use based_sema::EnumValue;
    match dv {
        DefaultVal::Lit(Literal::Str(s)) => format!("\"{}\"", s.replace('"', "\\\"")),
        DefaultVal::Lit(Literal::Int(i)) => i.to_string(),
        DefaultVal::Lit(Literal::Decimal(s)) => s.clone(),
        DefaultVal::Lit(Literal::Bool(b)) => b.to_string(),
        DefaultVal::Lit(Literal::Null) => "null".to_string(),
        DefaultVal::Func(f) => format!("{}()", f.name.node),
        // An enum default renders as its wire value — a quoted string for a string enum,
        // a bare integer for an int enum — matching the DB column default.
        DefaultVal::Variant(v) => match en.and_then(|e| e.wire_of(&v.node)) {
            Some(EnumValue::Int(n)) => n.to_string(),
            Some(EnumValue::Str(s)) => format!("\"{}\"", s.replace('"', "\\\"")),
            None => format!("\"{}\"", v.node.replace('"', "\\\"")),
        },
    }
}

// ---------- serialization -------------------------------------------------

/// Serialize a resolved schema to the canonical `schema.snap` text. Convenience over
/// [`Snapshot::from_schema`] + [`Snapshot::render`].
pub fn snapshot(schema: &CheckedSchema) -> String {
    Snapshot::from_schema(schema).render()
}

impl Snapshot {
    /// Render this snapshot to the canonical `schema.snap` text. Deterministic — the
    /// same snapshot renders byte-identically every time (no wall-clock, no map order).
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("# schema.snap — generated by `based migrate gen`; do not edit by hand.\n");
        out.push_str("snapshot v1 dialect=neutral\n");
        for s in &self.scopes {
            out.push_str(&render_scope_decl(s));
            out.push('\n');
        }
        for r in &self.renames {
            out.push_str(&render_rename(r));
            out.push('\n');
        }
        for t in &self.tables {
            out.push('\n');
            render_table(&mut out, t);
        }
        out
    }
}

/// One `scope Name (col: Type = $ctx.field, …)` decl line.
pub(crate) fn render_scope_decl(s: &ScopeDeclSnap) -> String {
    let terms = s
        .terms
        .iter()
        .map(|t| format!("{}: {} = $ctx.{}", t.column, t.ty, t.ctx_field))
        .collect::<Vec<_>>()
        .join(", ");
    format!("scope {} ({terms})", s.name)
}

/// One `rename table <old> -> <new>` / `rename column <table>.<old> -> <new>` line —
/// the `@was` directive persisted so the diff re-derives the rename offline.
fn render_rename(r: &Rename) -> String {
    match r {
        Rename::Table { from, to } => format!("rename table {from} -> {to}"),
        Rename::Column { table, from, to } => format!("rename column {table}.{from} -> {to}"),
    }
}

fn render_table(out: &mut String, t: &TableSnap) {
    let mut header = format!("table {}", t.name);
    if let Some((col, mode)) = &t.soft_delete {
        let _ = write!(header, " soft_delete={col}:{mode}");
    }
    if let Some(col) = &t.created {
        let _ = write!(header, " created={col}");
    }
    if let Some(col) = &t.updated {
        let _ = write!(header, " updated={col}");
    }
    // One `scope=(A, B)` group per `@scope` alternative (DNF): the commas inside a group
    // are the AND-conjunction, separate groups are the OR-alternatives.
    for alt in &t.scope_alts {
        let _ = write!(header, " scope=({})", alt.join(", "));
    }
    if !t.sort.is_empty() {
        let terms = t
            .sort
            .iter()
            .map(|(c, d)| format!("{c} {d}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(header, " sort=({terms})");
    }
    if t.no_id {
        header.push_str(" no_id");
    }
    header.push('\n');
    out.push_str(&header);

    for c in &t.columns {
        let mut line = format!(
            "  column {} {} {}",
            c.name,
            c.ty,
            if c.nullable { "null" } else { "not_null" }
        );
        if let Some(d) = &c.default {
            let _ = write!(line, " default={d}");
        }
        if c.unique {
            line.push_str(" unique");
        }
        if let Some(fk) = &c.fk {
            let _ = write!(line, " fk={fk}");
        }
        line.push('\n');
        out.push_str(&line);
    }
    for i in &t.indexes {
        out.push_str(&format!("  index {}\n", index_spec_text(i)));
    }
    for f in &t.foreign_keys {
        out.push_str(&format!("  {}\n", fk_spec_text(f)));
    }
}

/// The `fk <col> -> <ref_table>.<ref_col> [on_delete=<a>] [on_update=<a>]` line shared by
/// the `schema.snap` FK line and the `up.mig` foreign-key step.
pub fn fk_spec_text(f: &ForeignKeySnap) -> String {
    let mut s = format!("fk {} -> {}.{}", f.column, f.ref_table, f.ref_column);
    if let Some(a) = &f.on_delete {
        let _ = write!(s, " on_delete={a}");
    }
    if let Some(a) = &f.on_update {
        let _ = write!(s, " on_update={a}");
    }
    s
}

/// The `<name> (<cols>) [unique] [using <m>]` / `<name> raw(…)` spec shared by the
/// `schema.snap` index line and the `up.mig` index step, so both read identically.
pub(crate) fn index_spec_text(i: &IndexSnap) -> String {
    if let Some(raw) = &i.raw {
        return format!("{} {raw}", i.name);
    }
    let mut line = format!("{} ({})", i.name, i.columns.join(", "));
    if i.unique {
        line.push_str(" unique");
    }
    if let Some(m) = &i.method {
        line.push_str(&format!(" using {m}"));
    }
    line
}

// ---------- parsing (round-trip) ------------------------------------------

/// A `schema.snap` that could not be parsed — the file is corrupt or hand-edited into
/// an invalid state. Carries the 1-based line and a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "schema.snap line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

impl Snapshot {
    /// Parse a `schema.snap` back into the neutral model — the inverse of [`render`], so
    /// the stored baseline can be diffed against the current schema. Whitespace-tolerant
    /// on the leading indent; comments (`#…`) and the header line are skipped.
    ///
    /// [`render`]: Snapshot::render
    pub fn parse(text: &str) -> Result<Snapshot, ParseError> {
        let mut scopes: Vec<ScopeDeclSnap> = Vec::new();
        let mut tables: Vec<TableSnap> = Vec::new();
        let mut renames: Vec<Rename> = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line_no = i + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("snapshot ") {
                continue;
            }
            if let Some(rest) = line.strip_prefix("scope ") {
                scopes.push(parse_scope_decl(rest, line_no)?);
            } else if let Some(rest) = line.strip_prefix("rename ") {
                renames.push(parse_rename(rest, line_no)?);
            } else if let Some(rest) = line.strip_prefix("table ") {
                tables.push(parse_table_header(rest, line_no)?);
            } else if let Some(rest) = line.strip_prefix("column ") {
                let t = tables.last_mut().ok_or_else(|| ParseError {
                    line: line_no,
                    message: "column before any table".to_string(),
                })?;
                t.columns.push(parse_column(rest, line_no)?);
            } else if let Some(rest) = line.strip_prefix("index ") {
                let t = tables.last_mut().ok_or_else(|| ParseError {
                    line: line_no,
                    message: "index before any table".to_string(),
                })?;
                t.indexes.push(parse_index(rest, line_no)?);
            } else if let Some(rest) = line.strip_prefix("fk ") {
                let t = tables.last_mut().ok_or_else(|| ParseError {
                    line: line_no,
                    message: "fk before any table".to_string(),
                })?;
                t.foreign_keys.push(parse_fk(rest, line_no)?);
            } else {
                return Err(ParseError {
                    line: line_no,
                    message: format!("unrecognized line: {line}"),
                });
            }
        }
        Ok(Snapshot {
            scopes,
            tables,
            renames,
        })
    }
}

/// Parse a `table <old> -> <new>` / `column <table>.<old> -> <new>` rename line (the
/// `rename ` prefix already stripped).
fn parse_rename(rest: &str, line: usize) -> Result<Rename, ParseError> {
    let malformed = || ParseError {
        line,
        message: format!("malformed rename: {rest}"),
    };
    if let Some(spec) = rest.strip_prefix("table ") {
        let (from, to) = spec.split_once("->").ok_or_else(malformed)?;
        Ok(Rename::Table {
            from: from.trim().to_string(),
            to: to.trim().to_string(),
        })
    } else if let Some(spec) = rest.strip_prefix("column ") {
        let (lhs, to) = spec.split_once("->").ok_or_else(malformed)?;
        let (table, from) = lhs.trim().split_once('.').ok_or_else(malformed)?;
        Ok(Rename::Column {
            table: table.trim().to_string(),
            from: from.trim().to_string(),
            to: to.trim().to_string(),
        })
    } else {
        Err(malformed())
    }
}

/// Parse a top-level `scope Name (col: Type = $ctx.field, …)` decl line.
fn parse_scope_decl(rest: &str, line: usize) -> Result<ScopeDeclSnap, ParseError> {
    let open = rest.find('(').ok_or_else(|| ParseError {
        line,
        message: format!("scope decl has no term list: {rest}"),
    })?;
    let name = rest[..open].trim().to_string();
    if name.is_empty() {
        return Err(ParseError {
            line,
            message: "scope decl has no name".to_string(),
        });
    }
    let close = rest.rfind(')').ok_or_else(|| ParseError {
        line,
        message: format!("scope decl term list is not closed: {rest}"),
    })?;
    let inner = rest[open + 1..close].trim();
    let mut terms = Vec::new();
    if !inner.is_empty() {
        for t in inner.split(',') {
            let (col_ty, ctx) = t.split_once(" = $ctx.").ok_or_else(|| ParseError {
                line,
                message: format!("malformed scope term: {t}"),
            })?;
            let (col, ty) = col_ty.split_once(':').ok_or_else(|| ParseError {
                line,
                message: format!("scope term has no type: {t}"),
            })?;
            terms.push(ScopeTermSnap {
                column: col.trim().to_string(),
                ty: ty.trim().to_string(),
                ctx_field: ctx.trim().to_string(),
            });
        }
    }
    Ok(ScopeDeclSnap { name, terms })
}

/// Split off a `key=(a, b, c)` group after `key=`, returning the inner terms and the
/// remaining tail. Returns `None` when `head` does not start with `key=(`.
fn take_group<'a>(head: &'a str, key: &str) -> Option<(Vec<String>, &'a str)> {
    let after = head.strip_prefix(key)?.strip_prefix("=(")?;
    let close = after.find(')')?;
    let inner = &after[..close];
    let tail = after[close + 1..].trim_start();
    let terms = if inner.trim().is_empty() {
        Vec::new()
    } else {
        inner.split(',').map(|s| s.trim().to_string()).collect()
    };
    Some((terms, tail))
}

fn parse_table_header(rest: &str, line: usize) -> Result<TableSnap, ParseError> {
    let mut it = rest.splitn(2, char::is_whitespace);
    let name = it.next().unwrap_or("").to_string();
    if name.is_empty() {
        return Err(ParseError {
            line,
            message: "table has no name".to_string(),
        });
    }
    let mut head = it.next().unwrap_or("").trim_start();

    let mut soft_delete = None;
    let mut created = None;
    let mut updated = None;
    let mut scope_alts = Vec::new();
    let mut sort = Vec::new();
    let mut no_id = false;

    while !head.is_empty() {
        if head == "no_id" || head.starts_with("no_id ") {
            no_id = true;
            head = head["no_id".len()..].trim_start();
        } else if let Some(after) = head.strip_prefix("soft_delete=") {
            let mut sp = after.splitn(2, char::is_whitespace);
            let spec = sp.next().unwrap_or("");
            let (col, mode) = spec.split_once(':').ok_or_else(|| ParseError {
                line,
                message: format!("malformed soft_delete: {spec}"),
            })?;
            soft_delete = Some((col.to_string(), mode.to_string()));
            head = sp.next().unwrap_or("").trim_start();
        } else if let Some(after) = head.strip_prefix("created=") {
            let mut sp = after.splitn(2, char::is_whitespace);
            created = Some(sp.next().unwrap_or("").to_string());
            head = sp.next().unwrap_or("").trim_start();
        } else if let Some(after) = head.strip_prefix("updated=") {
            let mut sp = after.splitn(2, char::is_whitespace);
            updated = Some(sp.next().unwrap_or("").to_string());
            head = sp.next().unwrap_or("").trim_start();
        } else if let Some((names, tail)) = take_group(head, "scope") {
            // Each `scope=(A, B)` group is one `@scope` alternative (a name set).
            scope_alts.push(names);
            head = tail;
        } else if let Some((terms, tail)) = take_group(head, "sort") {
            for t in terms {
                let (col, dir) = t
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| ParseError {
                        line,
                        message: format!("malformed sort term: {t}"),
                    })?;
                sort.push((col.trim().to_string(), dir.trim().to_string()));
            }
            head = tail;
        } else {
            return Err(ParseError {
                line,
                message: format!("unrecognized table attribute: {head}"),
            });
        }
    }

    Ok(TableSnap {
        name,
        soft_delete,
        created,
        updated,
        scope_alts,
        sort,
        no_id,
        columns: Vec::new(),
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
    })
}

/// Parse a `<col> -> <ref_table>.<ref_col> [on_delete=<a>] [on_update=<a>]` FK line (the
/// `fk ` prefix already stripped).
fn parse_fk(rest: &str, line: usize) -> Result<ForeignKeySnap, ParseError> {
    let malformed = || ParseError {
        line,
        message: format!("malformed fk: {rest}"),
    };
    let (col, tail) = rest.split_once("->").ok_or_else(malformed)?;
    let mut toks = tail.split_whitespace();
    let reference = toks.next().ok_or_else(malformed)?;
    let (ref_table, ref_column) = reference.split_once('.').ok_or_else(malformed)?;
    let mut on_delete = None;
    let mut on_update = None;
    for tok in toks {
        if let Some(a) = tok.strip_prefix("on_delete=") {
            on_delete = Some(a.to_string());
        } else if let Some(a) = tok.strip_prefix("on_update=") {
            on_update = Some(a.to_string());
        } else {
            return Err(malformed());
        }
    }
    Ok(ForeignKeySnap {
        column: col.trim().to_string(),
        ref_table: ref_table.trim().to_string(),
        ref_column: ref_column.trim().to_string(),
        on_delete,
        on_update,
    })
}

/// Byte length of the balanced `raw(…)` token at the head of `s`, counting parens
/// outside string literals. `None` when it never closes (a corrupt snapshot).
fn balanced_end(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in s.char_indices() {
        match c {
            _ if esc => esc = false,
            '\\' if in_str => esc = true,
            '"' => in_str = !in_str,
            '(' if !in_str => depth += 1,
            ')' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_column(rest: &str, line: usize) -> Result<ColumnSnap, ParseError> {
    // A quoted `default="…"` is the one attribute that may hold spaces; pull it out of
    // the raw text first so the rest tokenizes on whitespace cleanly.
    let mut default = None;
    let mut remainder = rest.to_string();
    if let Some(idx) = rest.find("default=\"") {
        let after = &rest[idx + "default=".len()..]; // starts at the opening quote
        if let Some(close) = after[1..].find('"') {
            let end = close + 2; // include both quotes
            default = Some(after[..end].to_string());
            remainder = format!("{}{}", &rest[..idx], &after[end..]);
        }
    }

    // An opaque `raw(…)` type may hold spaces and commas; take it as one balanced token
    // before whitespace-splitting the rest.
    let mut raw_ty = None;
    if let Some(idx) = remainder.find("raw(") {
        if let Some(end) = balanced_end(&remainder[idx..]) {
            raw_ty = Some(remainder[idx..idx + end].to_string());
            remainder = format!("{}{}", &remainder[..idx], &remainder[idx + end..]);
        }
    }

    let mut toks = remainder.split_whitespace();
    let name = toks.next().ok_or_else(|| ParseError {
        line,
        message: "column has no name".to_string(),
    })?;
    let ty = match &raw_ty {
        Some(t) => t.as_str(),
        None => toks.next().ok_or_else(|| ParseError {
            line,
            message: "column has no type".to_string(),
        })?,
    };
    let nullability = toks.next().ok_or_else(|| ParseError {
        line,
        message: "column has no nullability".to_string(),
    })?;
    let nullable = match nullability {
        "null" => true,
        "not_null" => false,
        other => {
            return Err(ParseError {
                line,
                message: format!("expected null|not_null, got {other}"),
            })
        }
    };

    let mut unique = false;
    let mut fk = None;
    for tok in toks {
        if let Some(d) = tok.strip_prefix("default=") {
            default = Some(d.to_string());
        } else if tok == "unique" {
            unique = true;
        } else if let Some(t) = tok.strip_prefix("fk=") {
            fk = Some(t.to_string());
        } else {
            return Err(ParseError {
                line,
                message: format!("unrecognized column attribute: {tok}"),
            });
        }
    }

    Ok(ColumnSnap {
        name: name.to_string(),
        ty: ty.to_string(),
        nullable,
        default,
        unique,
        fk,
    })
}

fn parse_index(rest: &str, line: usize) -> Result<IndexSnap, ParseError> {
    // An opaque index is `<name> raw(…)`: everything after the name is the literal body.
    if let Some((name, body)) = rest.trim().split_once(char::is_whitespace) {
        let body = body.trim();
        if body.starts_with("raw(") {
            return Ok(IndexSnap {
                name: name.trim().to_string(),
                columns: Vec::new(),
                unique: false,
                method: None,
                raw: Some(body.to_string()),
            });
        }
    }
    let (name, after) = rest.split_once('(').ok_or_else(|| ParseError {
        line,
        message: format!("index missing column list: {rest}"),
    })?;
    let close = after.find(')').ok_or_else(|| ParseError {
        line,
        message: format!("index column list not closed: {rest}"),
    })?;
    let cols_inner = &after[..close];
    let columns: Vec<String> = if cols_inner.trim().is_empty() {
        Vec::new()
    } else {
        cols_inner
            .split(',')
            .map(|s| s.trim().to_string())
            .collect()
    };
    let flags = after[close + 1..].trim();
    let mut toks = flags.split_whitespace().peekable();
    let mut unique = false;
    let mut method = None;
    while let Some(tok) = toks.next() {
        match tok {
            "unique" => unique = true,
            "using" => method = toks.next().map(str::to_string),
            // Unknown flags are ignored, not rejected — a snapshot written by a newer or
            // older engine (e.g. a pre-D103 `inferred` marker) must still parse so an
            // existing ledger keeps applying.
            _ => {}
        }
    }

    Ok(IndexSnap {
        name: name.trim().to_string(),
        columns,
        unique,
        method,
        raw: None,
    })
}
