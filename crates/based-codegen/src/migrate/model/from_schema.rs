//! Build the neutral [`Snapshot`] from a resolved [`CheckedSchema`]: tables, columns,
//! indexes, and FK constraints, all sorted so nothing map-ordered leaks in.

use super::*;
use based_ast::{DefaultVal, Literal, Primitive, SortDir, SortTerm};
use based_sema::{CheckedSchema, ForeignKeys, MemberKind, RModel, SoftDelete, SoftMode};

impl Snapshot {
    /// Build the neutral snapshot from a resolved schema, under the default `foreign_keys`
    /// convention (`none`). Convenience over [`Snapshot::from_schema_with`] for callers
    /// (tests, from-scratch DDL that carries only explicit `@fk`s) that don't thread the
    /// manifest convention.
    pub fn from_schema(schema: &CheckedSchema) -> Self {
        Self::from_schema_with(schema, ForeignKeys::None)
    }

    /// Build the neutral snapshot from a resolved schema under a given `foreign_keys`
    /// convention. Pure and deterministic: tables, columns, indexes, and FK constraints are
    /// all sorted by name so nothing map-ordered leaks in.
    pub fn from_schema_with(schema: &CheckedSchema, fks: ForeignKeys) -> Self {
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
        Self {
            scopes,
            tables,
            renames,
        }
    }
}

/// Is this column the universally-implicit default `id`? Such a column is elided
/// from the snapshot's column list and carried as an invariant; a model that declares a
/// non-default `id` (a different type, nullable, or unique) records it explicitly.
fn is_default_id(c: &ColumnSnap) -> bool {
    c.name == "id" && c.ty == "uuid" && !c.nullable && !c.unique && c.fk.is_none()
}

/// The model's members lowered to neutral columns: scalars (with enum/raw/generated
/// captured in their type), a forward relation as its FK column. Inverse edges own no
/// column. The default `id` is elided and re-synthesized on render.
fn column_snaps(schema: &CheckedSchema, model: &RModel) -> Vec<ColumnSnap> {
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
                generated,
            } => columns.push(ColumnSnap {
                name: column.clone(),
                // An enum column captures its variants as `enum(v1,v2,…)` so a variant
                // add/remove is a diffable type change; it maps to text SQL (+ CHECK).
                // An opaque column carries its canonical `raw(…)` spelling, so a change
                // to the literal type string is an ordinary column-type diff.
                ty: raw_type
                    .as_ref()
                    .map(based_ast::RawSpec::canonical)
                    .or_else(|| enum_neutral_type(schema, enum_name.as_deref()))
                    .unwrap_or_else(|| neutral_type(*ty, *many)),
                nullable: *optional,
                default: default.as_ref().map(|dv| {
                    render_default(dv, enum_name.as_deref().and_then(|n| schema.enum_(n)))
                }),
                unique: *unique,
                fk: None,
                // A generated column records its expression in a dialect-neutral, re-parseable
                // form (bare column names, `||` concat, DSL literals), so the migrate renderer
                // re-lowers it per target dialect — and add/drop/expression-change all diff.
                generated: generated.as_ref().map(|e| {
                    crate::sql::generated_expr(
                        Some(schema),
                        Some(model),
                        e,
                        crate::sql::Emit::Neutral,
                    )
                }),
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
                generated: None,
            }),
            MemberKind::Inverse { .. } => {}
        }
    }
    columns.retain(|c| !is_default_id(c));
    columns.sort_by(|a, b| a.name.cmp(&b.name));
    columns
}

fn table_snap(schema: &CheckedSchema, model: &RModel, fks: ForeignKeys) -> TableSnap {
    let columns = column_snaps(schema, model);

    let mut indexes = index_snaps(schema, model);
    indexes.sort_by(|a, b| a.name.cmp(&b.name));

    let mut foreign_keys = foreign_key_snaps(schema, model, fks);
    foreign_keys.sort();

    // Record non-default PK column(s) so the from-scratch `CREATE TABLE` names them: a
    // renamed `id`, a single-column `@key`, or a composite `@key`'s ordered columns. The
    // default single `id` stays empty (elided + re-synthesized); a keyless (`@no_id`) table
    // has none.
    let pk_cols = model.pk_columns();
    let pk = if pk_cols == ["id"] {
        Vec::new()
    } else {
        pk_cols
    };

    TableSnap {
        name: model.table.clone(),
        schema: model.schema.clone(),
        soft_delete: model.soft_delete.as_ref().map(soft_delete_snap),
        created: model.created.clone(),
        updated: model.updated.clone(),
        scope_alts: canonical_scope_alts(&model.scope_alts),
        sort: model.sort.iter().map(sort_term).collect(),
        no_id: model.no_id,
        pk,
        columns,
        indexes,
        foreign_keys,
    }
}

/// The resolved FK constraints on a model's forward relations under the convention. A
/// relation whose target is keyless contributes none — there is no primary key to
/// reference.
pub fn foreign_key_snaps(
    schema: &CheckedSchema,
    model: &RModel,
    fks: ForeignKeys,
) -> Vec<ForeignKeySnap> {
    let mut out = Vec::new();
    for mem in &model.members {
        let MemberKind::Forward { target, .. } = &mem.kind else {
            continue;
        };
        let Some(resolved) = model.resolved_fk(mem, fks) else {
            continue;
        };
        // The FK column(s) + the target key column(s) they reference, paired in key order —
        // one pair for a single-column-key target, several for a composite key.
        let pairs = schema.fk_columns(mem);
        if pairs.is_empty() {
            continue;
        }
        let ref_table = schema
            .model(target)
            .map_or_else(|| target.clone(), |t| t.table.clone());
        let ref_schema = schema.model(target).and_then(|t| t.schema.clone());
        out.push(ForeignKeySnap {
            columns: pairs.iter().map(|(c, _)| c.clone()).collect(),
            ref_table,
            ref_schema,
            ref_columns: pairs
                .iter()
                .map(|(_, p)| p.physical_col().to_string())
                .collect(),
            on_delete: resolved.on_delete.map(|a| a.snap().to_string()),
            on_update: resolved.on_update.map(|a| a.snap().to_string()),
        });
    }
    out
}

/// The physical primary-key column of a relation target (`id`, its `(column "…")`
/// override, or a `@key(field)` natural key). `None` when the target is missing or
/// keyless (`@no_id`).
pub fn target_pk_column(schema: &CheckedSchema, target: &str) -> Option<String> {
    schema.model(target)?.pk_column()
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
fn index_snaps(schema: &CheckedSchema, model: &RModel) -> Vec<IndexSnap> {
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
            let cols: Vec<String> = fields
                .iter()
                .flat_map(|c| physical_cols(schema, model, c))
                .collect();
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

/// The physical column(s) backing a field: a scalar/single-column FK's column, else the
/// ordered `<field>_<part>` columns of a composite FK (mirrors `sql::physical_cols`).
fn physical_cols(schema: &CheckedSchema, model: &RModel, field: &str) -> Vec<String> {
    match model.member(field) {
        Some(mem) if matches!(mem.kind, MemberKind::Forward { .. }) => {
            let fks = schema.fk_columns(mem);
            if fks.len() > 1 {
                return fks.into_iter().map(|(c, _)| c).collect();
            }
            vec![physical_col(model, field)]
        }
        _ => vec![physical_col(model, field)],
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
        Primitive::Time => "time",
        Primitive::Bytes => "bytes",
        Primitive::Json => "json",
        Primitive::Uuid | Primitive::Id => "uuid",
        // `ulid`/`serial` are recorded distinctly so a PK generation-strategy change
        // diffs (the renderer maps them back through `neutral_sql_type`).
        Primitive::Ulid => "ulid",
        Primitive::Serial => "serial",
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

/// A relation FK's neutral type: the target model's key *storage* type (default uuid). A
/// `serial` target contributes a plain `int` — the FK stores the integer value, it is not
/// itself an auto-increment/identity column (only the referenced PK is).
fn fk_type(schema: &CheckedSchema, target: &str) -> String {
    match schema
        .model(target)
        .and_then(RModel::pk_member)
        .map(|m| &m.kind)
    {
        Some(MemberKind::Scalar {
            ty: Primitive::Serial,
            ..
        }) => "int".to_string(),
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
