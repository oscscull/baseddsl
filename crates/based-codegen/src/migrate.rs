//! Migration snapshot + diff engine (Track E2, `spec/syntax/migrations.md`).
//!
//! Two pure, dialect-neutral, deterministic functions over a [`CheckedSchema`]:
//!
//! - [`snapshot`] serializes the resolved schema to the canonical `schema.snap` text
//!   — stable-ordered (tables by name; columns and indexes by name within a table),
//!   dialect-neutral (`int`/`text`/`uuid`, never `BIGINT`/`TEXT`), and git-diffable.
//!   It is a pure function of the schema: no wall-clock, no map iteration order.
//! - [`diff`] compares a *prior* snapshot to the *current* schema and returns the
//!   neutral [`Step`] list (`up.mig`). `0001_init` diffs against the empty schema, so
//!   its steps are the full create set — exactly what `based gen sql` builds from
//!   scratch. Renames are never auto-guessed: a changed name is a drop + add pair (the
//!   `@was`-driven rename step is E5). Data-losing steps are *marked* destructive so
//!   apply (E4) can gate on the acknowledgement — this engine only marks, never applies.
//!
//! - [`render_sql`] (E3) renders the neutral [`Step`] list to per-dialect
//!   `CREATE`/`ALTER`/`DROP` SQL over the existing `Dialect` seam — reusing the DDL
//!   type map (`sql::sql_type`) so a migration's SQL can never drift from `based gen
//!   sql` (principle 4). `0001_init`'s create steps render to the same schema `based
//!   gen sql` builds from scratch.
//!
//! The snapshot text and step list stay decoupled from SQL: everything above
//! [`render_sql`] names no dialect.
//!
//! ## `schema.snap` grammar (finalizing migrations.md's TODO)
//! ```text
//! snapshot v1 dialect=neutral
//! scope <Name> (<col>: <Type> = $ctx.<field>, …)
//!
//! table <name> [soft_delete=<col>:<mode>] [scope=(<Name>, …)]* [sort=(<col> <dir>, …)]
//!   column <name> <type> null|not_null [default=<lit>] [unique] [fk=<Model>]
//!   index  <name> (<col>, …) [unique] [inferred]
//! ```
//! Named scopes (auth.md Handle 2) serialize once as top-level `scope` decls (sorted by
//! name, before the tables — the one place a scope column's/`$ctx` field's type lives) and
//! are referenced on each governed table by name: one `scope=(…)` group per `@scope`
//! alternative (the DNF — commas inside a group are the AND, separate groups the OR). A
//! scope emits no DDL, so this is header/decl metadata that round-trips for the drift check.
//! Every table opens with a `table` line and closes at the next `table`/EOF; its
//! `column`/`index` lines are indented two spaces. The `id` column is elided when it
//! is the default (`uuid`, not-null, not-unique) — a universally implicit invariant
//! (D2); a model that declares a non-default `id` records it explicitly.

use crate::Dialect;
use based_ast::{DefaultVal, Literal, Primitive, SortDir, SortTerm};
use based_sema::{CheckedSchema, MemberKind, RModel, SoftDelete, SoftMode};
use std::fmt::Write as _;

// ---------- neutral snapshot model ----------------------------------------

/// The canonical, dialect-neutral snapshot of a resolved schema: the diff baseline.
/// Derived from a [`CheckedSchema`] ([`Snapshot::from_schema`]) or parsed back from
/// `schema.snap` text ([`Snapshot::parse`]); a [`diff`] compares two of these.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    /// Named scope declarations (auth.md Handle 2), sorted by name and rendered above
    /// the tables. A scope emits no DDL — it is an injected row-visibility filter in
    /// generated code — but it is recorded here so a change to the contract (added,
    /// dropped, renamed, or a term retyped) is captured in a reviewable migration and
    /// caught by the offline drift check.
    pub scopes: Vec<ScopeDeclSnap>,
    /// Tables, sorted by name — the stable order that makes a git diff readable.
    pub tables: Vec<TableSnap>,
}

/// A `scope Name (col: Type = $ctx.field, …)` decl, captured neutrally: the column, the
/// declared type (a model name or a neutral primitive), and the `$ctx` field each term
/// binds. The one place the scope column's — and `$ctx.field`'s — type lives (auth.md).
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
    /// `@created` engine-managed column (set on insert, D2), if any.
    pub created: Option<String>,
    /// `@updated` engine-managed column (set on insert + every update, D2), if any.
    pub updated: Option<String>,
    /// The model's `@scope` alternatives, each a set of scope names (auth.md DNF). One
    /// entry per `@scope` decorator — `@scope A, B` is one alternative `["A", "B"]`, two
    /// stacked `@scope` decorators are two alternatives. Canonicalized (names sorted
    /// within an alternative, alternatives sorted) so the diff is stable. Empty = unscoped.
    pub scope_alts: Vec<Vec<String>>,
    /// `@sort` terms as `(column, dir)` where dir is `asc`/`desc`, in declaration order.
    pub sort: Vec<(String, String)>,
    /// Columns, sorted by name.
    pub columns: Vec<ColumnSnap>,
    /// Indexes (declared + inferred), sorted by name.
    pub indexes: Vec<IndexSnap>,
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
    /// An engine-inferred join-key baseline index (D15) vs. a declared `@index`.
    pub inferred: bool,
}

impl Snapshot {
    /// Build the neutral snapshot from a resolved schema. Pure and deterministic:
    /// tables, columns, and indexes are all sorted by name so nothing map-ordered
    /// leaks in.
    pub fn from_schema(schema: &CheckedSchema) -> Snapshot {
        let mut tables: Vec<TableSnap> = schema
            .models
            .iter()
            .map(|m| table_snap(schema, m))
            .collect();
        tables.sort_by(|a, b| a.name.cmp(&b.name));
        let mut scopes: Vec<ScopeDeclSnap> = schema.scopes.iter().map(scope_decl_snap).collect();
        scopes.sort_by(|a, b| a.name.cmp(&b.name));
        Snapshot { scopes, tables }
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

/// Is this column the universally-implicit default `id` (D2)? Such a column is elided
/// from the snapshot's column list and carried as an invariant; a model that declares a
/// non-default `id` (a different type, nullable, or unique) records it explicitly.
fn is_default_id(c: &ColumnSnap) -> bool {
    c.name == "id" && c.ty == "uuid" && !c.nullable && !c.unique && c.fk.is_none()
}

fn table_snap(schema: &CheckedSchema, model: &RModel) -> TableSnap {
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
            } => columns.push(ColumnSnap {
                name: column.clone(),
                ty: neutral_type(*ty, *many),
                nullable: *optional,
                default: default.as_ref().map(render_default),
                unique: *unique,
                fk: None,
            }),
            MemberKind::Forward {
                target,
                optional,
                fk_col,
                ..
            } => columns.push(ColumnSnap {
                // A relation is its FK column (D3): its physical type is the target's
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

    TableSnap {
        name: model.table.clone(),
        soft_delete: model.soft_delete.as_ref().map(soft_delete_snap),
        created: model.created.clone(),
        updated: model.updated.clone(),
        scope_alts: canonical_scope_alts(&model.scope_alts),
        sort: model.sort.iter().map(sort_term).collect(),
        columns,
        indexes,
    }
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

/// Declared `@index`es + the inferred join-key baseline (D15), each resolved to
/// physical columns and its stable `idx_`/`uq_`/`inf_` name. Mirrors `sql::ddl`'s
/// naming so the snapshot's index identity matches the generated DDL exactly — the
/// soft-delete column is prepended to an inferred index (predicate-leading, D15).
fn index_snaps(model: &RModel) -> Vec<IndexSnap> {
    let mut out = Vec::new();
    for idx in &model.indexes {
        let cols: Vec<String> = idx.columns.iter().map(|c| physical_col(model, c)).collect();
        out.push(IndexSnap {
            name: index_name(if idx.unique { "uq" } else { "idx" }, &model.table, &cols),
            columns: cols,
            unique: idx.unique,
            inferred: false,
        });
    }
    for idx in &model.inferred_indexes {
        let mut fields = idx.columns.clone();
        if let Some(sd) = &model.soft_delete {
            fields.insert(0, sd.field.clone());
        }
        let cols: Vec<String> = fields.iter().map(|c| physical_col(model, c)).collect();
        out.push(IndexSnap {
            name: index_name("inf", &model.table, &cols),
            columns: cols,
            unique: false,
            inferred: true,
        });
    }
    out
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
fn index_name(prefix: &str, table: &str, columns: &[String]) -> String {
    let mut name = format!("{prefix}_{table}");
    for c in columns {
        name.push('_');
        name.push_str(c);
    }
    name
}

/// Neutral type family for a primitive (`Id` folds to `uuid`, D1). A to-many scalar
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
    };
    if many {
        format!("{base}[]")
    } else {
        base.to_string()
    }
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
fn render_default(dv: &DefaultVal) -> String {
    match dv {
        DefaultVal::Lit(Literal::Str(s)) => format!("\"{}\"", s.replace('"', "\\\"")),
        DefaultVal::Lit(Literal::Int(i)) => i.to_string(),
        DefaultVal::Lit(Literal::Float(f)) => f.to_string(),
        DefaultVal::Lit(Literal::Bool(b)) => b.to_string(),
        DefaultVal::Lit(Literal::Null) => "null".to_string(),
        DefaultVal::Func(f) => format!("{}()", f.name.node),
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
        for t in &self.tables {
            out.push('\n');
            render_table(&mut out, t);
        }
        out
    }
}

/// One `scope Name (col: Type = $ctx.field, …)` decl line.
fn render_scope_decl(s: &ScopeDeclSnap) -> String {
    let terms = s
        .terms
        .iter()
        .map(|t| format!("{}: {} = $ctx.{}", t.column, t.ty, t.ctx_field))
        .collect::<Vec<_>>()
        .join(", ");
    format!("scope {} ({terms})", s.name)
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
    // are the AND-conjunction, separate groups are the OR-alternatives (auth.md).
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
        let cols = i.columns.join(", ");
        let mut line = format!("  index {} ({cols})", i.name);
        if i.unique {
            line.push_str(" unique");
        }
        if i.inferred {
            line.push_str(" inferred");
        }
        line.push('\n');
        out.push_str(&line);
    }
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
        for (i, raw) in text.lines().enumerate() {
            let line_no = i + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("snapshot ") {
                continue;
            }
            if let Some(rest) = line.strip_prefix("scope ") {
                scopes.push(parse_scope_decl(rest, line_no)?);
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
            } else {
                return Err(ParseError {
                    line: line_no,
                    message: format!("unrecognized line: {line}"),
                });
            }
        }
        Ok(Snapshot { scopes, tables })
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

    while !head.is_empty() {
        if let Some(after) = head.strip_prefix("soft_delete=") {
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
        columns: Vec::new(),
        indexes: Vec::new(),
    })
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

    let mut toks = remainder.split_whitespace();
    let name = toks.next().ok_or_else(|| ParseError {
        line,
        message: "column has no name".to_string(),
    })?;
    let ty = toks.next().ok_or_else(|| ParseError {
        line,
        message: "column has no type".to_string(),
    })?;
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
    let unique = flags.split_whitespace().any(|f| f == "unique");
    let inferred = flags.split_whitespace().any(|f| f == "inferred");

    Ok(IndexSnap {
        name: name.trim().to_string(),
        columns,
        unique,
        inferred,
    })
}

// ---------- neutral step vocabulary (`up.mig`) ----------------------------

/// One neutral migration step (migrations.md's `up.mig` vocabulary). Dialect-neutral:
/// E3 renders each to per-dialect DDL over the `Dialect` seam. A [`Step`] carrying
/// `destructive: true` (drops, type-narrowing, a new `not_null` without a default, a new
/// unique over existing data) is *marked* so apply (E4) can gate it on an acknowledgement
/// — this engine marks, never applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Full `CREATE TABLE` — `0001_init` is entirely these.
    CreateTable(TableSnap),
    /// `drop table <name>` — DESTRUCTIVE.
    DropTable(String),
    /// `add column <table>.<col> …`.
    AddColumn { table: String, column: ColumnSnap },
    /// `drop column <table>.<col>` — DESTRUCTIVE.
    DropColumn { table: String, column: String },
    /// `alter column <table>.<col> …` — one or more changes.
    AlterColumn {
        table: String,
        column: String,
        changes: Vec<ColumnChange>,
        /// The resulting column state. Carried because MariaDB alters a column via a
        /// full `MODIFY COLUMN <full definition>` — it cannot express a piecemeal
        /// null/type change — so the renderer (E3) needs the whole target column, not
        /// just the deltas. Postgres/SQLite render from the `changes` alone.
        after: ColumnSnap,
    },
    /// `add index <name> (<cols>)`.
    AddIndex { table: String, index: IndexSnap },
    /// `drop index <name>`.
    DropIndex { table: String, name: String },
    /// `add unique <name> (<cols>)` — DESTRUCTIVE over existing data.
    AddUnique { table: String, index: IndexSnap },
    /// `drop unique <name>`.
    DropUnique { table: String, name: String },
    /// A scope-contract change (auth.md Handle 2): a scope decl added/dropped/retyped, or a
    /// model joining/leaving a scope. A scope emits **no DDL** (it is an injected filter in
    /// generated code, not a DB object), so this renders as a neutral note and produces no
    /// SQL — it exists so the change lands in a reviewable migration and advances the
    /// snapshot, keeping the offline drift check honest.
    ScopeChange(ScopeChange),
}

/// The kinds of scope-contract change a diff surfaces (auth.md / DNF). None emit SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeChange {
    /// A new `scope Name (…)` decl.
    Add(ScopeDeclSnap),
    /// A `scope Name` decl removed.
    Drop(String),
    /// A surviving `scope Name` decl whose terms changed; carries the new state.
    Alter(ScopeDeclSnap),
    /// A surviving table's `@scope` alternative set changed (joined/left/re-shaped a scope).
    Table {
        table: String,
        alts: Vec<Vec<String>>,
    },
}

/// One field-level change inside an [`Step::AlterColumn`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnChange {
    /// `type <new>` — DESTRUCTIVE when narrowing (see [`is_narrowing`]).
    Type { from: String, to: String },
    /// `null` — always safe (relaxing a constraint).
    SetNull,
    /// `not_null` — DESTRUCTIVE without a default (existing NULLs violate it).
    SetNotNull { has_default: bool },
    /// `default=<lit>` — safe.
    SetDefault(String),
    /// `drop default` — safe.
    DropDefault,
}

impl Step {
    /// Does this step risk losing or rejecting data (migrations.md's destructive
    /// policy)? Apply (E4) gates a destructive step on `--allow-destructive` /
    /// `unsafe("reason")`; this engine only reports the marker.
    pub fn destructive(&self) -> bool {
        match self {
            Step::DropTable(_) | Step::DropColumn { .. } | Step::AddUnique { .. } => true,
            Step::AlterColumn { changes, .. } => changes.iter().any(|c| match c {
                ColumnChange::Type { from, to } => is_narrowing(from, to),
                ColumnChange::SetNotNull { has_default } => !has_default,
                _ => false,
            }),
            _ => false,
        }
    }
}

/// Is a type change `from -> to` narrowing (potentially truncating/failing)? Widening
/// is safe (`int -> text`); a same-family no-op is not a change at all. Anything that is
/// not a recognized safe widening is treated as narrowing (conservative — principle 1).
fn is_narrowing(from: &str, to: &str) -> bool {
    if from == to {
        return false;
    }
    // The only unambiguously safe widenings: any scalar -> text (text holds any value),
    // and int -> its wider selves (we have one `int`, so this is really the text case).
    !matches!(to, "text")
}

// ---------- diff ----------------------------------------------------------

/// Diff a *prior* snapshot against the *current* schema, producing the neutral `up.mig`
/// step list. An empty prior snapshot (`0001_init`) yields a full create set — exactly
/// what `based gen sql` builds from scratch. Renames are never auto-guessed: a changed
/// name is a drop + add pair (the `@was`-driven rename is E5).
pub fn diff(prev: &Snapshot, schema: &CheckedSchema) -> Vec<Step> {
    diff_snapshots(prev, &Snapshot::from_schema(schema))
}

/// Diff two neutral snapshots (the pure core — used by [`diff`] and by verify/drift). The
/// step order is: drops last within a table isn't required, but table creates/drops are
/// grouped and columns/indexes are emitted in a stable name order so the `up.mig` is
/// deterministic and reviewable.
pub fn diff_snapshots(prev: &Snapshot, now: &Snapshot) -> Vec<Step> {
    let mut steps = Vec::new();

    // Tables added (present now, absent before) → full CREATE. Sorted by name.
    for t in &now.tables {
        if prev.table(&t.name).is_none() {
            steps.push(Step::CreateTable(t.clone()));
        }
    }

    // Tables surviving → column + index deltas. Sorted by name (both snapshots are).
    for now_t in &now.tables {
        if let Some(prev_t) = prev.table(&now_t.name) {
            diff_table(prev_t, now_t, &mut steps);
        }
    }

    // Tables dropped (present before, absent now) → DROP. Sorted by name.
    for t in &prev.tables {
        if now.table(&t.name).is_none() {
            steps.push(Step::DropTable(t.name.clone()));
        }
    }

    diff_scopes(prev, now, &mut steps);

    steps
}

/// Diff the scope contract (top-level decls + membership on surviving tables). Emits
/// no-DDL [`Step::ScopeChange`] steps so a scope-only change still produces a reviewable
/// migration. Skipped for a from-scratch `0001_init` (empty prior): the initial scopes ride
/// in `schema.snap` and each table's `scope_alts` rides its `CreateTable`, so init stays
/// create-only (its `up.mig` matches `based gen sql` from scratch, which emits no scope SQL).
fn diff_scopes(prev: &Snapshot, now: &Snapshot, steps: &mut Vec<Step>) {
    if prev.tables.is_empty() && prev.scopes.is_empty() {
        return;
    }
    // Scope decls added / retyped (present now).
    for s in &now.scopes {
        match prev.scope(&s.name) {
            None => steps.push(Step::ScopeChange(ScopeChange::Add(s.clone()))),
            Some(old) if old != s => steps.push(Step::ScopeChange(ScopeChange::Alter(s.clone()))),
            Some(_) => {}
        }
    }
    // Scope decls dropped (present before, absent now).
    for s in &prev.scopes {
        if now.scope(&s.name).is_none() {
            steps.push(Step::ScopeChange(ScopeChange::Drop(s.name.clone())));
        }
    }
    // Surviving tables whose `@scope` membership changed (joined/left/re-shaped).
    for now_t in &now.tables {
        if let Some(prev_t) = prev.table(&now_t.name) {
            if prev_t.scope_alts != now_t.scope_alts {
                steps.push(Step::ScopeChange(ScopeChange::Table {
                    table: now_t.name.clone(),
                    alts: now_t.scope_alts.clone(),
                }));
            }
        }
    }
}

fn diff_table(prev: &TableSnap, now: &TableSnap, steps: &mut Vec<Step>) {
    // Columns added.
    for c in &now.columns {
        if prev.column(&c.name).is_none() {
            steps.push(Step::AddColumn {
                table: now.name.clone(),
                column: c.clone(),
            });
        }
    }
    // Columns altered (present in both, changed).
    for c in &now.columns {
        if let Some(old) = prev.column(&c.name) {
            let changes = column_changes(old, c);
            if !changes.is_empty() {
                steps.push(Step::AlterColumn {
                    table: now.name.clone(),
                    column: c.name.clone(),
                    changes,
                    after: c.clone(),
                });
            }
        }
    }
    // Columns dropped.
    for c in &prev.columns {
        if now.column(&c.name).is_none() {
            steps.push(Step::DropColumn {
                table: now.name.clone(),
                column: c.name.clone(),
            });
        }
    }

    // Indexes added. A unique index is its own `add unique` step (destructive over
    // existing data); a plain index is `add index` (safe).
    for i in &now.indexes {
        if prev.index(&i.name).map(|p| p == i) != Some(true) && prev.index(&i.name).is_none() {
            if i.unique {
                steps.push(Step::AddUnique {
                    table: now.name.clone(),
                    index: i.clone(),
                });
            } else {
                steps.push(Step::AddIndex {
                    table: now.name.clone(),
                    index: i.clone(),
                });
            }
        }
    }
    // Indexes changed (same name, different columns/unique) → drop + re-add. Renaming
    // an index isn't auto-guessed either; a definition change is a drop then an add.
    for i in &now.indexes {
        if let Some(old) = prev.index(&i.name) {
            if old != i {
                drop_index_step(old, now, steps);
                if i.unique {
                    steps.push(Step::AddUnique {
                        table: now.name.clone(),
                        index: i.clone(),
                    });
                } else {
                    steps.push(Step::AddIndex {
                        table: now.name.clone(),
                        index: i.clone(),
                    });
                }
            }
        }
    }
    // Indexes dropped.
    for i in &prev.indexes {
        if now.index(&i.name).is_none() {
            drop_index_step(i, now, steps);
        }
    }
}

fn drop_index_step(idx: &IndexSnap, table: &TableSnap, steps: &mut Vec<Step>) {
    if idx.unique {
        steps.push(Step::DropUnique {
            table: table.name.clone(),
            name: idx.name.clone(),
        });
    } else {
        steps.push(Step::DropIndex {
            table: table.name.clone(),
            name: idx.name.clone(),
        });
    }
}

/// The field-level changes turning column `old` into `now`. Empty when identical.
fn column_changes(old: &ColumnSnap, now: &ColumnSnap) -> Vec<ColumnChange> {
    let mut changes = Vec::new();
    if old.ty != now.ty {
        changes.push(ColumnChange::Type {
            from: old.ty.clone(),
            to: now.ty.clone(),
        });
    }
    if old.nullable != now.nullable {
        if now.nullable {
            changes.push(ColumnChange::SetNull);
        } else {
            changes.push(ColumnChange::SetNotNull {
                has_default: now.default.is_some(),
            });
        }
    }
    if old.default != now.default {
        match &now.default {
            Some(d) => changes.push(ColumnChange::SetDefault(d.clone())),
            None => changes.push(ColumnChange::DropDefault),
        }
    }
    changes
}

// ---------- `up.mig` rendering --------------------------------------------

/// Render a neutral step list to the canonical `up.mig` text. Deterministic; each step
/// is one (or a few) line(s) in migrations.md's vocabulary. A destructive step carries a
/// trailing `# DESTRUCTIVE` marker so a reviewer sees the gate before applying.
pub fn render_up(steps: &[Step]) -> String {
    let mut out = String::new();
    out.push_str("# up.mig — generated by `based migrate gen`; edit if needed, then apply.\n");
    for s in steps {
        render_step(&mut out, s);
    }
    out
}

fn render_step(out: &mut String, step: &Step) {
    let destructive = step.destructive();
    match step {
        Step::CreateTable(t) => render_create_table(out, t),
        Step::DropTable(name) => {
            let _ = writeln!(out, "drop table {name}  # DESTRUCTIVE");
        }
        Step::AddColumn { table, column } => {
            let _ = writeln!(out, "add column {table}.{}", column_spec(column));
        }
        Step::DropColumn { table, column } => {
            let _ = writeln!(out, "drop column {table}.{column}  # DESTRUCTIVE");
        }
        Step::AlterColumn {
            table,
            column,
            changes,
            ..
        } => {
            let parts = changes
                .iter()
                .map(render_change)
                .collect::<Vec<_>>()
                .join(" ");
            let tail = if destructive { "  # DESTRUCTIVE" } else { "" };
            let _ = writeln!(out, "alter column {table}.{column} {parts}{tail}");
        }
        Step::AddIndex { index, .. } => {
            let _ = writeln!(
                out,
                "add index {} ({})",
                index.name,
                index.columns.join(", ")
            );
        }
        Step::DropIndex { name, .. } => {
            let _ = writeln!(out, "drop index {name}");
        }
        Step::AddUnique { index, .. } => {
            let _ = writeln!(
                out,
                "add unique {} ({})  # DESTRUCTIVE",
                index.name,
                index.columns.join(", ")
            );
        }
        Step::DropUnique { name, .. } => {
            let _ = writeln!(out, "drop unique {name}");
        }
        Step::ScopeChange(sc) => {
            let _ = writeln!(out, "{}  # code-level; no DDL", scope_change_line(sc));
        }
    }
}

/// The neutral `up.mig` line for a scope-contract change (no SQL is emitted for it).
fn scope_change_line(sc: &ScopeChange) -> String {
    match sc {
        ScopeChange::Add(s) => render_scope_decl(s),
        ScopeChange::Alter(s) => format!("alter {}", render_scope_decl(s)),
        ScopeChange::Drop(name) => format!("drop scope {name}"),
        ScopeChange::Table { table, alts } => {
            let rhs = if alts.is_empty() {
                "unscoped".to_string()
            } else {
                alts.iter()
                    .map(|a| format!("({})", a.join(", ")))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            format!("scope table {table} = {rhs}")
        }
    }
}

fn render_create_table(out: &mut String, t: &TableSnap) {
    let _ = writeln!(out, "create table {} {{", t.name);
    for c in &t.columns {
        let _ = writeln!(out, "  column {}", column_spec(c));
    }
    for i in &t.indexes {
        let kind = if i.unique { "unique" } else { "index" };
        let _ = writeln!(out, "  {kind} {} ({})", i.name, i.columns.join(", "));
    }
    out.push_str("}\n");
}

/// The `<col> <type> null|not_null [default=…] [unique] [fk=…]` spec shared by
/// `create table` bodies and `add column` steps.
fn column_spec(c: &ColumnSnap) -> String {
    let mut s = format!(
        "{} {} {}",
        c.name,
        c.ty,
        if c.nullable { "null" } else { "not_null" }
    );
    if let Some(d) = &c.default {
        let _ = write!(s, " default={d}");
    }
    if c.unique {
        s.push_str(" unique");
    }
    if let Some(fk) = &c.fk {
        let _ = write!(s, " fk={fk}");
    }
    s
}

fn render_change(c: &ColumnChange) -> String {
    match c {
        ColumnChange::Type { to, .. } => format!("type {to}"),
        ColumnChange::SetNull => "null".to_string(),
        ColumnChange::SetNotNull { .. } => "not_null".to_string(),
        ColumnChange::SetDefault(d) => format!("default={d}"),
        ColumnChange::DropDefault => "drop default".to_string(),
    }
}

// ---------- per-dialect SQL rendering (E3) --------------------------------

/// Render a neutral step list to executable per-dialect SQL over the `Dialect` seam
/// (migrations.md, E3). This is the "review the SQL" surface (`based migrate render`):
/// `0001_init`'s create steps render to the same DDL `based gen sql` builds from scratch
/// (the neutral type map goes through `sql::sql_type`, so the two can't drift, P4).
/// A destructive step is preceded by a loud `-- DESTRUCTIVE` comment (principle 1).
///
/// Deliberate dialect divergences: MariaDB alters a column with a full `MODIFY COLUMN`
/// (it has no piecemeal `SET NOT NULL`); Postgres emits one `ALTER COLUMN` per change;
/// SQLite has no in-place `ALTER COLUMN` at all, so such a step renders as a loud comment
/// pointing at a hand-authored `raw(sqlite)` table-rebuild (the neutral vocabulary's edge,
/// principle 6). `DROP INDEX` also differs (MySQL/MariaDB need `ON <table>`).
pub fn render_sql(steps: &[Step], dialect: Dialect) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "-- Rendered by `based migrate render` (dialect: {}). Review before apply.",
        dialect.name()
    );
    for step in steps {
        out.push('\n');
        // A scope change alters generated code, not the database — render it as a note.
        if let Step::ScopeChange(sc) = step {
            let _ = writeln!(
                out,
                "-- scope contract change (no DDL): {}",
                scope_change_line(sc)
            );
            continue;
        }
        if step.destructive() {
            out.push_str(
                "-- DESTRUCTIVE: needs --allow-destructive or an unsafe(\"reason\") ack to apply.\n",
            );
        }
        match step_statements(step, dialect) {
            // Each bare statement is written `;`-terminated for the reviewer/psql/mysql.
            Ok(stmts) => {
                for s in stmts {
                    let _ = writeln!(out, "{s};");
                }
            }
            // A step with no in-place rendering for this dialect (SQLite `ALTER COLUMN`):
            // a loud, greppable comment, never broken SQL (principle 6).
            Err(msg) => {
                let _ = writeln!(out, "-- {msg}");
            }
        }
    }
    out
}

/// The executable statements for a step list, for `based migrate apply` — bare (no
/// trailing `;`, no comments), so a driver can run each through `Db::execute`. `Err(msg)`
/// = a step the dialect can't render in place (a SQLite `ALTER COLUMN` — the author must
/// supply a `raw(sqlite)` rebuild); apply surfaces it loudly rather than emit broken SQL
/// (principle 6). This is the execution twin of [`render_sql`]'s review text; both go
/// through [`step_statements`], so the SQL applied is exactly the SQL reviewed.
pub fn sql_statements(steps: &[Step], dialect: Dialect) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for step in steps {
        out.extend(step_statements(step, dialect)?);
    }
    Ok(out)
}

/// Bare executable statement(s) for one neutral step (no trailing `;`, no comment). A
/// `CreateTable` yields several on SQLite/Postgres (the table + trailing `CREATE INDEX`es);
/// most steps yield one.
fn step_statements(step: &Step, dialect: Dialect) -> Result<Vec<String>, String> {
    Ok(match step {
        Step::CreateTable(t) => create_table_statements(t, dialect),
        Step::DropTable(name) => vec![format!("DROP TABLE {}", dialect.quote(name))],
        Step::AddColumn { table, column } => vec![format!(
            "ALTER TABLE {} ADD COLUMN {}",
            dialect.quote(table),
            column_ddl(column, dialect),
        )],
        Step::DropColumn { table, column } => vec![format!(
            "ALTER TABLE {} DROP COLUMN {}",
            dialect.quote(table),
            dialect.quote(column),
        )],
        Step::AlterColumn {
            table,
            column,
            changes,
            after,
        } => alter_column_statements(table, column, changes, after, dialect)?,
        Step::AddIndex { table, index } | Step::AddUnique { table, index } => {
            vec![create_index_sql(dialect, table, index)]
        }
        Step::DropIndex { table, name } | Step::DropUnique { table, name } => {
            vec![drop_index_sql(dialect, table, name)]
        }
        // A scope change is code-level (an injected filter), not DDL — no SQL to run.
        Step::ScopeChange(_) => vec![],
    })
}

/// A stable content hash of an `up.mig`'s canonical bytes — the `_based_migrations`
/// ledger's tamper guard (migrations.md). Canonicalization drops comment (`#…`) and blank
/// lines and trims each remaining line, so a cosmetic whitespace/comment edit doesn't trip
/// the guard but any change to a step does. FNV-1a-64 (the same family the runtime uses for
/// request fingerprints, D31), rendered as 16 lowercase hex digits — collision resistance
/// is not security-critical here (it guards against an accidental post-apply edit, not an
/// adversary), so a fast non-cryptographic hash is the right tool.
pub fn content_hash(up_text: &str) -> String {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for line in up_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        mix(line.as_bytes());
        mix(b"\n");
    }
    format!("{h:016x}")
}

/// The statement(s) for a full `CREATE TABLE` from a neutral snapshot table. Mirrors
/// `sql::create_table`: the implicit `id` PK (D2) is re-synthesized (it is elided from the
/// snapshot) unless the model declared its own; `(unique)` columns become `CONSTRAINT …
/// UNIQUE`; indexes are inline `KEY`/`UNIQUE KEY` on MariaDB (one statement) and trailing
/// standalone `CREATE INDEX` statements elsewhere.
fn create_table_statements(t: &TableSnap, dialect: Dialect) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    // Implicit `id` primary key (D2): synthesized as the default uuid when the snapshot
    // elided it; a declared non-default `id` rides in the column list instead.
    if t.column("id").is_none() {
        lines.push(format!(
            "{} {} NOT NULL",
            dialect.quote("id"),
            crate::sql::sql_type(Primitive::Uuid, false, dialect),
        ));
    }
    for c in &t.columns {
        lines.push(column_ddl(c, dialect));
    }
    lines.push(format!("PRIMARY KEY ({})", dialect.quote("id")));

    // Column-level `(unique)` constraints (a declared `@index (unique)` is an IndexSnap
    // instead — handled below — so there is no double-emit).
    for c in &t.columns {
        if c.unique {
            lines.push(format!(
                "CONSTRAINT {} UNIQUE ({})",
                dialect.quote(&index_name("uq", &t.name, std::slice::from_ref(&c.name))),
                dialect.quote(&c.name),
            ));
        }
    }

    // MariaDB inlines indexes as table clauses; SQLite/Postgres trail them as statements.
    if dialect == Dialect::MariaDb {
        for i in &t.indexes {
            let cols = quote_cols(dialect, &i.columns);
            let kind = if i.unique { "UNIQUE KEY" } else { "KEY" };
            lines.push(format!("{kind} {} ({cols})", dialect.quote(&i.name)));
        }
    }

    let body = lines
        .iter()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join(",\n");
    let mut stmts = vec![format!(
        "CREATE TABLE {} (\n{body}\n)",
        dialect.quote(&t.name)
    )];
    if dialect != Dialect::MariaDb {
        for i in &t.indexes {
            stmts.push(create_index_sql(dialect, &t.name, i));
        }
    }
    stmts
}

fn alter_column_statements(
    table: &str,
    column: &str,
    changes: &[ColumnChange],
    after: &ColumnSnap,
    dialect: Dialect,
) -> Result<Vec<String>, String> {
    Ok(match dialect {
        // Postgres: one `ALTER COLUMN` sub-statement per change (it has them all).
        Dialect::Postgres => changes
            .iter()
            .map(|ch| {
                let clause = match ch {
                    ColumnChange::Type { to, .. } => {
                        format!("TYPE {}", neutral_sql_type(to, dialect))
                    }
                    ColumnChange::SetNull => "DROP NOT NULL".to_string(),
                    ColumnChange::SetNotNull { .. } => "SET NOT NULL".to_string(),
                    ColumnChange::SetDefault(d) => {
                        format!("SET DEFAULT {}", render_neutral_default(d, dialect))
                    }
                    ColumnChange::DropDefault => "DROP DEFAULT".to_string(),
                };
                format!(
                    "ALTER TABLE {} ALTER COLUMN {} {clause}",
                    dialect.quote(table),
                    dialect.quote(column),
                )
            })
            .collect(),
        // MariaDB: a type/null change needs a full `MODIFY COLUMN` (no piecemeal form);
        // a default-only change uses `ALTER COLUMN … SET/DROP DEFAULT`.
        Dialect::MariaDb => {
            let structural = changes.iter().any(|c| {
                matches!(
                    c,
                    ColumnChange::Type { .. }
                        | ColumnChange::SetNull
                        | ColumnChange::SetNotNull { .. }
                )
            });
            if structural {
                vec![format!(
                    "ALTER TABLE {} MODIFY COLUMN {}",
                    dialect.quote(table),
                    column_ddl(after, dialect),
                )]
            } else {
                changes
                    .iter()
                    .filter_map(|ch| match ch {
                        ColumnChange::SetDefault(d) => Some(format!(
                            "ALTER TABLE {} ALTER COLUMN {} SET DEFAULT {}",
                            dialect.quote(table),
                            dialect.quote(column),
                            render_neutral_default(d, dialect),
                        )),
                        ColumnChange::DropDefault => Some(format!(
                            "ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT",
                            dialect.quote(table),
                            dialect.quote(column),
                        )),
                        _ => None,
                    })
                    .collect()
            }
        }
        // SQLite has no in-place ALTER COLUMN — a type/null/default change requires the
        // 12-step table rebuild, which the neutral vocabulary can't safely auto-generate.
        // Surface a loud, greppable message pointing at a hand-authored raw(sqlite) step
        // (principle 6 — the escape hatch is never silent) rather than broken SQL.
        Dialect::Sqlite => {
            return Err(format!(
                "SQLite cannot ALTER COLUMN {table}.{column} in place; author a raw(sqlite) table-rebuild migration."
            ))
        }
    })
}

/// A standalone `CREATE [UNIQUE] INDEX` (all dialects share this form for an add). Bare
/// (no trailing `;`); `render_sql` terminates it, `apply` executes it as-is.
fn create_index_sql(dialect: Dialect, table: &str, index: &IndexSnap) -> String {
    let kind = if index.unique {
        "CREATE UNIQUE INDEX"
    } else {
        "CREATE INDEX"
    };
    format!(
        "{kind} {} ON {} ({})",
        dialect.quote(&index.name),
        dialect.quote(table),
        quote_cols(dialect, &index.columns),
    )
}

/// `DROP INDEX` — MySQL/MariaDB require the `ON <table>` qualifier; SQLite/Postgres
/// drop by index name alone. Bare (no trailing `;`).
fn drop_index_sql(dialect: Dialect, table: &str, name: &str) -> String {
    match dialect {
        Dialect::MariaDb => format!(
            "DROP INDEX {} ON {}",
            dialect.quote(name),
            dialect.quote(table)
        ),
        Dialect::Sqlite | Dialect::Postgres => format!("DROP INDEX {}", dialect.quote(name)),
    }
}

/// A column definition `<name> <type> NULL|NOT NULL [DEFAULT <lit>]`, shared by
/// `CREATE TABLE` bodies, `ADD COLUMN`, and MariaDB's `MODIFY COLUMN`. Matches
/// `sql::column_line` so an `add column` reads identically to a `create table` column.
fn column_ddl(c: &ColumnSnap, dialect: Dialect) -> String {
    let mut s = format!(
        "{} {} {}",
        dialect.quote(&c.name),
        neutral_sql_type(&c.ty, dialect),
        if c.nullable { "NULL" } else { "NOT NULL" },
    );
    if let Some(d) = &c.default {
        let _ = write!(s, " DEFAULT {}", render_neutral_default(d, dialect));
    }
    s
}

/// Map a neutral snapshot type (`int`/`text`/`uuid`/…, `[]` for a to-many scalar) to the
/// dialect's SQL type — through `sql::sql_type`, the *same* map `based gen sql` uses (P4).
fn neutral_sql_type(neutral: &str, dialect: Dialect) -> String {
    let (base, many) = match neutral.strip_suffix("[]") {
        Some(b) => (b, true),
        None => (neutral, false),
    };
    let prim = match base {
        "text" => Primitive::Text,
        "int" => Primitive::Int,
        "bool" => Primitive::Bool,
        "timestamp" => Primitive::Timestamp,
        "date" => Primitive::Date,
        "json" => Primitive::Json,
        "uuid" => Primitive::Uuid,
        // A corrupt/hand-edited snapshot type; parse/verify guards this upstream.
        _ => Primitive::Text,
    };
    crate::sql::sql_type(prim, many, dialect).to_string()
}

/// Render a neutral snapshot default (`render_default`'s output — a quoted string,
/// number, `true`/`false`, `null`, or `now()`) to a dialect SQL literal/expression.
/// The inverse of `render_default`, over the same value forms.
fn render_neutral_default(d: &str, dialect: Dialect) -> String {
    // A quoted string default → a SQL string literal (unescape `\"`, then `'`-quote).
    if let Some(inner) = d.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        let unescaped = inner.replace("\\\"", "\"");
        return format!("'{}'", unescaped.replace('\'', "''"));
    }
    match d {
        "true" => dialect.bool_lit(true).to_string(),
        "false" => dialect.bool_lit(false).to_string(),
        "null" => "NULL".to_string(),
        // `now()` is the only value-position function (ir::KNOWN_FUNCS).
        _ if d.ends_with("()") => "CURRENT_TIMESTAMP".to_string(),
        // A numeric literal rides through verbatim.
        _ => d.to_string(),
    }
}

/// Quote a physical column list for the dialect, comma-joined.
fn quote_cols(dialect: Dialect, cols: &[String]) -> String {
    cols.iter()
        .map(|c| dialect.quote(c))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str, nullable: bool) -> ColumnSnap {
        ColumnSnap {
            name: name.to_string(),
            ty: ty.to_string(),
            nullable,
            default: None,
            unique: false,
            fk: None,
        }
    }

    fn table(name: &str, columns: Vec<ColumnSnap>) -> TableSnap {
        TableSnap {
            name: name.to_string(),
            soft_delete: None,
            created: None,
            updated: None,
            scope_alts: Vec::new(),
            sort: Vec::new(),
            columns,
            indexes: Vec::new(),
        }
    }

    fn scope_decl(name: &str, terms: &[(&str, &str, &str)]) -> ScopeDeclSnap {
        ScopeDeclSnap {
            name: name.to_string(),
            terms: terms
                .iter()
                .map(|(c, t, f)| ScopeTermSnap {
                    column: c.to_string(),
                    ty: t.to_string(),
                    ctx_field: f.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn render_then_parse_round_trips_every_attribute() {
        let snap = Snapshot {
            scopes: vec![scope_decl("Tenant", &[("org", "Org", "org")])],
            tables: vec![TableSnap {
                name: "order".to_string(),
                soft_delete: Some(("deleted_at".to_string(), "timestamp".to_string())),
                created: Some("created_at".to_string()),
                updated: Some("updated_at".to_string()),
                scope_alts: vec![vec!["Tenant".to_string()]],
                sort: vec![("placed_at".to_string(), "desc".to_string())],
                columns: vec![
                    ColumnSnap {
                        name: "status".to_string(),
                        ty: "text".to_string(),
                        nullable: false,
                        default: Some("\"pending\"".to_string()),
                        unique: false,
                        fk: None,
                    },
                    ColumnSnap {
                        name: "org_id".to_string(),
                        ty: "uuid".to_string(),
                        nullable: false,
                        default: None,
                        unique: false,
                        fk: Some("Org".to_string()),
                    },
                ],
                indexes: vec![IndexSnap {
                    name: "idx_order_status".to_string(),
                    columns: vec!["status".to_string()],
                    unique: false,
                    inferred: true,
                }],
            }],
        };
        let text = snap.render();
        let parsed = Snapshot::parse(&text).expect("parse round-trip");
        assert_eq!(snap, parsed, "\n{text}");
    }

    #[test]
    fn parse_tolerates_a_quoted_default_with_spaces() {
        let mut c = col("label", "text", false);
        c.default = Some("\"in progress\"".to_string());
        let snap = Snapshot {
            scopes: Vec::new(),
            tables: vec![table("job", vec![c])],
        };
        let parsed = Snapshot::parse(&snap.render()).expect("parse");
        assert_eq!(snap, parsed);
    }

    #[test]
    fn init_diff_from_empty_is_a_full_create_set() {
        let now = Snapshot {
            scopes: Vec::new(),
            tables: vec![
                table("a", vec![col("x", "int", false)]),
                table("b", vec![col("y", "text", true)]),
            ],
        };
        let steps = diff_snapshots(&Snapshot::default(), &now);
        assert_eq!(steps.len(), 2);
        assert!(matches!(&steps[0], Step::CreateTable(t) if t.name == "a"));
        assert!(matches!(&steps[1], Step::CreateTable(t) if t.name == "b"));
    }

    #[test]
    fn add_column_and_add_index_between_versions() {
        let prev = Snapshot {
            scopes: Vec::new(),
            tables: vec![table("product", vec![col("name", "text", false)])],
        };
        let mut now_t = table(
            "product",
            vec![col("name", "text", false), col("barcode", "text", true)],
        );
        now_t.indexes.push(IndexSnap {
            name: "idx_product_barcode".to_string(),
            columns: vec!["barcode".to_string()],
            unique: false,
            inferred: false,
        });
        let now = Snapshot {
            scopes: Vec::new(),
            tables: vec![now_t],
        };
        let steps = diff_snapshots(&prev, &now);
        assert_eq!(steps.len(), 2);
        assert!(matches!(&steps[0], Step::AddColumn { column, .. } if column.name == "barcode"));
        assert!(
            matches!(&steps[1], Step::AddIndex { index, .. } if index.name == "idx_product_barcode")
        );
        // Neither a nullable add nor a plain index is destructive.
        assert!(steps.iter().all(|s| !s.destructive()));
    }

    #[test]
    fn dropping_a_column_and_a_table_is_destructive() {
        let prev = Snapshot {
            scopes: Vec::new(),
            tables: vec![
                table("keep", vec![col("a", "int", false), col("b", "int", false)]),
                table("gone", vec![col("c", "int", false)]),
            ],
        };
        let now = Snapshot {
            scopes: Vec::new(),
            tables: vec![table("keep", vec![col("a", "int", false)])],
        };
        let steps = diff_snapshots(&prev, &now);
        // drop column keep.b + drop table gone.
        let drops: Vec<_> = steps.iter().filter(|s| s.destructive()).collect();
        assert_eq!(drops.len(), 2);
        assert!(steps
            .iter()
            .any(|s| matches!(s, Step::DropColumn { column, .. } if column == "b")));
        assert!(steps
            .iter()
            .any(|s| matches!(s, Step::DropTable(n) if n == "gone")));
    }

    #[test]
    fn narrowing_type_and_new_not_null_without_default_are_destructive() {
        let prev = Snapshot {
            scopes: Vec::new(),
            tables: vec![table("t", vec![col("v", "text", true)])],
        };
        // text -> int (narrowing) AND null -> not_null with no default.
        let now = Snapshot {
            scopes: Vec::new(),
            tables: vec![table("t", vec![col("v", "int", false)])],
        };
        let steps = diff_snapshots(&prev, &now);
        assert_eq!(steps.len(), 1);
        assert!(steps[0].destructive(), "{:?}", steps[0]);

        // The inverse — widening int -> text and relaxing not_null -> null — is safe.
        let prev2 = Snapshot {
            scopes: Vec::new(),
            tables: vec![table("t", vec![col("v", "int", false)])],
        };
        let now2 = Snapshot {
            scopes: Vec::new(),
            tables: vec![table("t", vec![col("v", "text", true)])],
        };
        let steps2 = diff_snapshots(&prev2, &now2);
        assert_eq!(steps2.len(), 1);
        assert!(!steps2[0].destructive(), "{:?}", steps2[0]);
    }

    #[test]
    fn adding_a_unique_index_is_destructive_over_existing_data() {
        let prev = Snapshot {
            scopes: Vec::new(),
            tables: vec![table("t", vec![col("email", "text", false)])],
        };
        let mut now_t = table("t", vec![col("email", "text", false)]);
        now_t.indexes.push(IndexSnap {
            name: "uq_t_email".to_string(),
            columns: vec!["email".to_string()],
            unique: true,
            inferred: false,
        });
        let now = Snapshot {
            scopes: Vec::new(),
            tables: vec![now_t],
        };
        let steps = diff_snapshots(&prev, &now);
        assert_eq!(steps.len(), 1);
        assert!(matches!(&steps[0], Step::AddUnique { .. }));
        assert!(steps[0].destructive());
    }

    #[test]
    fn no_changes_yields_no_steps() {
        let snap = Snapshot {
            scopes: Vec::new(),
            tables: vec![table("t", vec![col("a", "int", false)])],
        };
        assert!(diff_snapshots(&snap, &snap).is_empty());
    }

    /// A multi-alternative (OR) scope round-trips through render/parse and its addition,
    /// term change, and a model joining a second alternative each surface as diff steps.
    #[test]
    fn multi_alternative_scope_serializes_parses_and_diffs() {
        // `Post` is scoped either by page OR by author — two stacked `@scope` decorators.
        let mut post = table("post", vec![col("body", "text", false)]);
        post.columns.push(col("page_id", "uuid", false));
        post.columns.push(col("author_id", "uuid", false));
        post.scope_alts = vec![vec!["Author".to_string()], vec!["Page".to_string()]];
        let now = Snapshot {
            scopes: vec![
                scope_decl("Author", &[("author", "User", "user")]),
                scope_decl("Page", &[("page", "Page", "page")]),
            ],
            tables: vec![post.clone()],
        };

        // Round-trip: both the OR alternatives and the two top-level decls survive.
        let text = now.render();
        assert!(
            text.contains("scope Author (author: User = $ctx.user)"),
            "\n{text}"
        );
        assert!(
            text.contains("scope Page (page: Page = $ctx.page)"),
            "\n{text}"
        );
        assert!(text.contains("scope=(Author) scope=(Page)"), "\n{text}");
        assert_eq!(Snapshot::parse(&text).expect("round-trip"), now, "\n{text}");

        // From a prior with only the `Page` scope + a `Post` scoped by page alone: adding
        // the `Author` scope decl and Post joining it both surface (no DDL, but stepped).
        let mut prev_post = table("post", vec![col("body", "text", false)]);
        prev_post.columns.push(col("page_id", "uuid", false));
        prev_post.columns.push(col("author_id", "uuid", false));
        prev_post.scope_alts = vec![vec!["Page".to_string()]];
        let prev = Snapshot {
            scopes: vec![scope_decl("Page", &[("page", "Page", "page")])],
            tables: vec![prev_post],
        };
        let steps = diff_snapshots(&prev, &now);
        assert!(
            steps
                .iter()
                .any(|s| matches!(s, Step::ScopeChange(ScopeChange::Add(d)) if d.name == "Author")),
            "{steps:?}"
        );
        assert!(
            steps.iter().any(|s| matches!(s, Step::ScopeChange(ScopeChange::Table { table, .. }) if table == "post")),
            "{steps:?}"
        );
        // Scope changes are code-level, never destructive, and emit no SQL.
        assert!(steps.iter().all(|s| !s.destructive()));
        assert!(sql_statements(&steps, MariaDb).unwrap().is_empty());

        // Dropping a scope and retyping a term both surface too.
        let mut now2 = now.clone();
        now2.scopes[0].terms[0].ctx_field = "actor".to_string(); // Author term retyped
        now2.scopes.remove(1); // Page dropped
        now2.tables[0].scope_alts = vec![vec!["Author".to_string()]];
        let steps2 = diff_snapshots(&now, &now2);
        assert!(steps2
            .iter()
            .any(|s| matches!(s, Step::ScopeChange(ScopeChange::Alter(d)) if d.name == "Author")));
        assert!(steps2
            .iter()
            .any(|s| matches!(s, Step::ScopeChange(ScopeChange::Drop(n)) if n == "Page")));
    }

    /// A from-scratch `0001_init` stays create-only even with scopes: the scope contract
    /// rides `schema.snap` + each `CreateTable`'s `scope_alts`, so no `ScopeChange` steps.
    #[test]
    fn init_diff_omits_scope_change_steps() {
        let mut t = table("post", vec![col("body", "text", false)]);
        t.scope_alts = vec![vec!["Page".to_string()]];
        let now = Snapshot {
            scopes: vec![scope_decl("Page", &[("page", "Page", "page")])],
            tables: vec![t],
        };
        let steps = diff_snapshots(&Snapshot::default(), &now);
        assert!(
            steps.iter().all(|s| matches!(s, Step::CreateTable(_))),
            "{steps:?}"
        );
    }

    // ---- E3: per-dialect SQL rendering ----------------------------------

    use crate::Dialect::{MariaDb, Postgres, Sqlite};

    #[test]
    fn create_table_renders_id_pk_and_types_per_dialect() {
        // A nullable column, a not-null column, and a unique column exercise the type
        // map, nullability, and the `(unique)` constraint across all three dialects.
        let mut email = col("email", "text", false);
        email.unique = true;
        let t = table("account", vec![email, col("age", "int", true)]);
        let steps = vec![Step::CreateTable(t)];

        let maria = render_sql(&steps, MariaDb);
        assert!(maria.contains("CREATE TABLE `account` ("), "\n{maria}");
        assert!(maria.contains("`id` UUID NOT NULL"), "\n{maria}");
        assert!(maria.contains("`email` VARCHAR(255) NOT NULL"), "\n{maria}");
        assert!(maria.contains("`age` BIGINT NULL"), "\n{maria}");
        assert!(maria.contains("PRIMARY KEY (`id`)"), "\n{maria}");
        assert!(
            maria.contains("CONSTRAINT `uq_account_email` UNIQUE (`email`)"),
            "\n{maria}"
        );

        let pg = render_sql(&steps, Postgres);
        assert!(pg.contains("CREATE TABLE \"account\" ("), "\n{pg}");
        assert!(pg.contains("\"id\" UUID NOT NULL"), "\n{pg}");
        assert!(pg.contains("\"email\" TEXT NOT NULL"), "\n{pg}");
        assert!(pg.contains("\"age\" BIGINT NULL"), "\n{pg}");

        let sqlite = render_sql(&steps, Sqlite);
        assert!(sqlite.contains("`id` TEXT NOT NULL"), "\n{sqlite}");
        assert!(sqlite.contains("`age` INTEGER NULL"), "\n{sqlite}");
    }

    #[test]
    fn create_table_indexes_inline_on_mariadb_standalone_elsewhere() {
        let mut t = table("item", vec![col("sku", "text", false)]);
        t.indexes.push(IndexSnap {
            name: "idx_item_sku".to_string(),
            columns: vec!["sku".to_string()],
            unique: false,
            inferred: false,
        });
        let steps = vec![Step::CreateTable(t)];

        // MariaDB carries the index inline as a `KEY` clause inside the CREATE TABLE.
        let maria = render_sql(&steps, MariaDb);
        assert!(maria.contains("KEY `idx_item_sku` (`sku`)"), "\n{maria}");
        assert!(!maria.contains("CREATE INDEX"), "\n{maria}");

        // Postgres/SQLite trail it as a separate CREATE INDEX statement.
        let pg = render_sql(&steps, Postgres);
        assert!(
            pg.contains("CREATE INDEX \"idx_item_sku\" ON \"item\" (\"sku\");"),
            "\n{pg}"
        );
    }

    #[test]
    fn add_column_and_string_default_render() {
        let mut c = col("status", "text", false);
        c.default = Some("\"pending\"".to_string());
        let steps = vec![Step::AddColumn {
            table: "order".to_string(),
            column: c,
        }];
        let maria = render_sql(&steps, MariaDb);
        assert!(
            maria.contains(
                "ALTER TABLE `order` ADD COLUMN `status` VARCHAR(255) NOT NULL DEFAULT 'pending';"
            ),
            "\n{maria}"
        );
    }

    #[test]
    fn drop_column_and_drop_table_carry_destructive_markers() {
        let steps = vec![
            Step::DropColumn {
                table: "product".to_string(),
                column: "legacy".to_string(),
            },
            Step::DropTable("gone".to_string()),
        ];
        let out = render_sql(&steps, Postgres);
        assert!(
            out.contains("-- DESTRUCTIVE"),
            "destructive marker missing\n{out}"
        );
        assert!(
            out.contains("ALTER TABLE \"product\" DROP COLUMN \"legacy\";"),
            "\n{out}"
        );
        assert!(out.contains("DROP TABLE \"gone\";"), "\n{out}");
    }

    #[test]
    fn alter_column_diverges_per_dialect() {
        // null -> not_null AND type text -> int on the same column.
        let after = col("v", "int", false);
        let changes = vec![
            ColumnChange::Type {
                from: "text".to_string(),
                to: "int".to_string(),
            },
            ColumnChange::SetNotNull { has_default: false },
        ];
        let steps = vec![Step::AlterColumn {
            table: "t".to_string(),
            column: "v".to_string(),
            changes,
            after,
        }];

        // Postgres: one ALTER COLUMN sub-statement per change.
        let pg = render_sql(&steps, Postgres);
        assert!(
            pg.contains("ALTER TABLE \"t\" ALTER COLUMN \"v\" TYPE BIGINT;"),
            "\n{pg}"
        );
        assert!(
            pg.contains("ALTER TABLE \"t\" ALTER COLUMN \"v\" SET NOT NULL;"),
            "\n{pg}"
        );

        // MariaDB: a single full MODIFY COLUMN restating the resulting definition.
        let maria = render_sql(&steps, MariaDb);
        assert!(
            maria.contains("ALTER TABLE `t` MODIFY COLUMN `v` BIGINT NOT NULL;"),
            "\n{maria}"
        );

        // SQLite: a loud comment (no in-place ALTER COLUMN) — never broken SQL.
        let sqlite = render_sql(&steps, Sqlite);
        assert!(
            sqlite.contains("-- SQLite cannot ALTER COLUMN t.v in place"),
            "\n{sqlite}"
        );

        // Both narrowing and a new not-null-without-default are destructive.
        assert!(steps[0].destructive());
        assert!(render_sql(&steps, Postgres).contains("-- DESTRUCTIVE"));
    }

    #[test]
    fn mariadb_default_only_alter_avoids_modify() {
        let after = col("v", "int", false);
        let steps = vec![Step::AlterColumn {
            table: "t".to_string(),
            column: "v".to_string(),
            changes: vec![ColumnChange::SetDefault("0".to_string())],
            after,
        }];
        let maria = render_sql(&steps, MariaDb);
        assert!(
            maria.contains("ALTER TABLE `t` ALTER COLUMN `v` SET DEFAULT 0;"),
            "\n{maria}"
        );
        assert!(!maria.contains("MODIFY COLUMN"), "\n{maria}");
    }

    #[test]
    fn index_add_and_drop_render_per_dialect() {
        let uq = IndexSnap {
            name: "uq_u_email".to_string(),
            columns: vec!["email".to_string()],
            unique: true,
            inferred: false,
        };
        let add = vec![Step::AddUnique {
            table: "u".to_string(),
            index: uq,
        }];
        let out = render_sql(&add, Postgres);
        assert!(out.contains("-- DESTRUCTIVE"), "\n{out}"); // unique over existing data
        assert!(
            out.contains("CREATE UNIQUE INDEX \"uq_u_email\" ON \"u\" (\"email\");"),
            "\n{out}"
        );

        let drop = vec![Step::DropIndex {
            table: "u".to_string(),
            name: "idx_u_name".to_string(),
        }];
        // MySQL/MariaDB need the `ON <table>` qualifier; Postgres/SQLite drop by name.
        assert!(render_sql(&drop, MariaDb).contains("DROP INDEX `idx_u_name` ON `u`;"));
        assert!(render_sql(&drop, Postgres).contains("DROP INDEX \"idx_u_name\";"));
    }

    // ---- E4: executable statements + content hash -----------------------

    #[test]
    fn sql_statements_are_bare_and_one_per_statement() {
        // A create + an add-column → bare statements (no `;`, no comments), exactly what
        // `apply` runs one at a time through `Db::execute`.
        let mut t = table("thing", vec![col("name", "text", false)]);
        t.indexes.push(IndexSnap {
            name: "idx_thing_name".to_string(),
            columns: vec!["name".to_string()],
            unique: false,
            inferred: false,
        });
        let steps = vec![
            Step::CreateTable(t),
            Step::AddColumn {
                table: "thing".to_string(),
                column: col("size", "int", true),
            },
        ];
        let stmts = sql_statements(&steps, Postgres).unwrap();
        // create table, its trailing create index, then the add column.
        assert_eq!(stmts.len(), 3, "{stmts:#?}");
        assert!(
            stmts[0].starts_with("CREATE TABLE \"thing\" ("),
            "{}",
            stmts[0]
        );
        assert!(
            stmts.iter().all(|s| !s.ends_with(';')),
            "no trailing `;`: {stmts:#?}"
        );
        assert!(
            stmts.iter().all(|s| !s.contains("--")),
            "no comments: {stmts:#?}"
        );
        assert!(
            stmts[1].contains("CREATE INDEX \"idx_thing_name\" ON \"thing\" (\"name\")"),
            "{}",
            stmts[1]
        );
        assert!(
            stmts[2].contains("ALTER TABLE \"thing\" ADD COLUMN \"size\""),
            "{}",
            stmts[2]
        );
    }

    #[test]
    fn sql_statements_errs_on_sqlite_alter_column() {
        // SQLite can't ALTER COLUMN in place — `apply` must fail loudly, not emit broken SQL.
        let steps = vec![Step::AlterColumn {
            table: "t".to_string(),
            column: "v".to_string(),
            changes: vec![ColumnChange::SetNotNull { has_default: false }],
            after: col("v", "int", false),
        }];
        let err = sql_statements(&steps, Sqlite).unwrap_err();
        assert!(err.contains("SQLite cannot ALTER COLUMN t.v"), "{err}");
    }

    #[test]
    fn content_hash_ignores_comments_and_whitespace_but_not_steps() {
        let a = "# generated header\nadd column product.barcode text null\n";
        // Same step, different comment + blank lines + indentation → identical hash.
        let b = "\n  add column product.barcode text null  \n# a different comment\n";
        assert_eq!(content_hash(a), content_hash(b));
        // A real change to the step → a different hash (the tamper guard fires).
        let c = "add column product.barcode text not_null\n";
        assert_ne!(content_hash(a), content_hash(c));
        // 16 lowercase hex digits.
        assert_eq!(content_hash(a).len(), 16);
        assert!(content_hash(a).bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
