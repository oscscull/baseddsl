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
//! The snapshot text is decoupled from SQL: E3 renders the neutral [`Step`]s to
//! per-dialect DDL over the existing `Dialect` seam. This module names no dialect.
//!
//! ## `schema.snap` grammar (finalizing migrations.md's TODO)
//! ```text
//! snapshot v1 dialect=neutral
//!
//! table <name> [soft_delete=<col>:<mode>] [scope=(<col> = $ctx.<field>, …)] [sort=(<col> <dir>, …)]
//!   column <name> <type> null|not_null [default=<lit>] [unique] [fk=<Model>]
//!   index  <name> (<col>, …) [unique] [inferred]
//! ```
//! Every table opens with a `table` line and closes at the next `table`/EOF; its
//! `column`/`index` lines are indented two spaces. The `id` column is elided when it
//! is the default (`uuid`, not-null, not-unique) — a universally implicit invariant
//! (D2); a model that declares a non-default `id` records it explicitly.

use based_ast::{DefaultVal, Literal, Primitive, SortDir, SortTerm};
use based_sema::{CheckedSchema, MemberKind, RModel, SoftDelete, SoftMode};
use std::fmt::Write as _;

// ---------- neutral snapshot model ----------------------------------------

/// The canonical, dialect-neutral snapshot of a resolved schema: the diff baseline.
/// Derived from a [`CheckedSchema`] ([`Snapshot::from_schema`]) or parsed back from
/// `schema.snap` text ([`Snapshot::parse`]); a [`diff`] compares two of these.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    /// Tables, sorted by name — the stable order that makes a git diff readable.
    pub tables: Vec<TableSnap>,
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
    /// `@scope` terms as `(column, ctx_field)` pairs (D32), in declaration order.
    pub scope: Vec<(String, String)>,
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
        Snapshot { tables }
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
        scope: model.scope_terms(),
        sort: model.sort.iter().map(sort_term).collect(),
        columns,
        indexes,
    }
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
        for t in &self.tables {
            out.push('\n');
            render_table(&mut out, t);
        }
        out
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
    if !t.scope.is_empty() {
        let terms = t
            .scope
            .iter()
            .map(|(c, f)| format!("{c} = $ctx.{f}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(header, " scope=({terms})");
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
        let mut tables: Vec<TableSnap> = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line_no = i + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("snapshot ") {
                continue;
            }
            if let Some(rest) = line.strip_prefix("table ") {
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
        Ok(Snapshot { tables })
    }
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
    let mut scope = Vec::new();
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
        } else if let Some((terms, tail)) = take_group(head, "scope") {
            for t in terms {
                let (col, ctx) = t.split_once(" = $ctx.").ok_or_else(|| ParseError {
                    line,
                    message: format!("malformed scope term: {t}"),
                })?;
                scope.push((col.trim().to_string(), ctx.trim().to_string()));
            }
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
        scope,
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
    },
    /// `add index <name> (<cols>)`.
    AddIndex { table: String, index: IndexSnap },
    /// `drop index <name>`.
    DropIndex { table: String, name: String },
    /// `add unique <name> (<cols>)` — DESTRUCTIVE over existing data.
    AddUnique { table: String, index: IndexSnap },
    /// `drop unique <name>`.
    DropUnique { table: String, name: String },
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

    steps
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
            scope: Vec::new(),
            sort: Vec::new(),
            columns,
            indexes: Vec::new(),
        }
    }

    #[test]
    fn render_then_parse_round_trips_every_attribute() {
        let snap = Snapshot {
            tables: vec![TableSnap {
                name: "order".to_string(),
                soft_delete: Some(("deleted_at".to_string(), "timestamp".to_string())),
                created: Some("created_at".to_string()),
                updated: Some("updated_at".to_string()),
                scope: vec![("org".to_string(), "org".to_string())],
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
            tables: vec![table("job", vec![c])],
        };
        let parsed = Snapshot::parse(&snap.render()).expect("parse");
        assert_eq!(snap, parsed);
    }

    #[test]
    fn init_diff_from_empty_is_a_full_create_set() {
        let now = Snapshot {
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
            tables: vec![
                table("keep", vec![col("a", "int", false), col("b", "int", false)]),
                table("gone", vec![col("c", "int", false)]),
            ],
        };
        let now = Snapshot {
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
            tables: vec![table("t", vec![col("v", "text", true)])],
        };
        // text -> int (narrowing) AND null -> not_null with no default.
        let now = Snapshot {
            tables: vec![table("t", vec![col("v", "int", false)])],
        };
        let steps = diff_snapshots(&prev, &now);
        assert_eq!(steps.len(), 1);
        assert!(steps[0].destructive(), "{:?}", steps[0]);

        // The inverse — widening int -> text and relaxing not_null -> null — is safe.
        let prev2 = Snapshot {
            tables: vec![table("t", vec![col("v", "int", false)])],
        };
        let now2 = Snapshot {
            tables: vec![table("t", vec![col("v", "text", true)])],
        };
        let steps2 = diff_snapshots(&prev2, &now2);
        assert_eq!(steps2.len(), 1);
        assert!(!steps2[0].destructive(), "{:?}", steps2[0]);
    }

    #[test]
    fn adding_a_unique_index_is_destructive_over_existing_data() {
        let prev = Snapshot {
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
            tables: vec![table("t", vec![col("a", "int", false)])],
        };
        assert!(diff_snapshots(&snap, &snap).is_empty());
    }
}
