//! based-ast — the shared AST vocabulary.
//!
//! Mirrors the canonical grammar node-for-node. No logic lives here; the parser
//! builds these, sema/codegen read them. Spans ride the nodes that produce
//! diagnostics.

// ---------- Source positions ----------------------------------------------

/// Index into the compilation's file table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(pub u32);

/// Byte range within a single file. Half-open `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

/// A value plus where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

/// An identifier token. Casing is load-bearing — preserved verbatim.
pub type Ident = Spanned<String>;

// ---------- Files ----------------------------------------------------------
// One extension (`.bsl`); the grammar is uniform across files. Any declaration
// may appear in any file. Splitting schema vs access into separate files is a
// recommended convention, not enforced.

/// A parsed source file: an ordered list of top-level declarations.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaFile {
    pub decls: Vec<Decl>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decl {
    Model(Model),
    Shape(Shape),
    Scope(ScopeDecl),
    Enum(EnumDecl),
    Query(Query),
    Mutation(Mutation),
    Filter(NamedFilter),
}

// ---------- Enums ----------------------------------------------------------

/// `enum Name { pending, paid = "PAID", … }` — a closed set of named values, a
/// first-class scalar type. The name (UpperCamel) shares the type-name namespace with
/// models and shapes; the variants (lowercase snake) are its members. A field typed by
/// an enum name is a scalar column (not a relation). The kind (string vs numeric) is
/// inferred in sema from whether any variant carries an int value.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    pub name: Ident,
    pub variants: Vec<EnumVariant>,
    pub span: Span,
}

/// One `variant` of an enum: a bare identifier name, optionally with an explicit wire
/// value (`paid = "PAID"` or `low = 0`). The name is always the identifier (it yields the
/// Rust variant, go-to-def, and rename); `value` is the wire representation when written.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    pub name: Ident,
    /// The explicit `= STRING | INT` wire value, if written. Absent → a bare string
    /// variant whose wire value is its own name.
    pub value: Option<Spanned<VariantValue>>,
}

/// An explicit enum-variant wire value: a string (`= "PAID"`) or an integer (`= 0`).
#[derive(Debug, Clone, PartialEq)]
pub enum VariantValue {
    Str(String),
    Int(i64),
}

// ---------- Scopes ---------------------------------------------------------

/// `scope Name (col: Type = $ctx.field, …)` — a named row-visibility contract,
/// declared once and referenced by name on the model (`@scope Name`) and every
/// callable that touches it (`scoped Name`). The predicate is the restricted form
/// (a conjunction of `col = $ctx.field`); the term's `Type` is the one place the
/// scope column's — and thus `$ctx.field`'s — type is declared.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeDecl {
    pub name: Ident,
    pub terms: Vec<ScopeTerm>,
    pub span: Span,
}

/// One `col: Type = $ctx.field` term of a `scope` decl.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeTerm {
    pub col: Ident,
    pub ty: TypeExpr,
    /// The `$ctx.field` the column binds to. Sema checks it is `$ctx.<field>`
    /// (exactly one segment) — anything else is `E0180`.
    pub ctx: ParamRef,
    pub span: Span,
}

/// One `@scope Name[, Name]*` decorator on a model — **one alternative** of the
/// model's scope DNF: the comma-separated names are a conjunction (all required
/// together). A model stacks these; the stack is the OR of alternatives. `@scope`
/// is repeatable, so a model carries a `Vec<ScopeRef>`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeRef {
    pub names: Vec<Ident>,
    pub span: Span,
}

/// `scoped Name[, Name]*` — the per-callable acknowledgement of the standing scope(s)
/// injected. Mutually exclusive with `unscoped(…)`; sits where `unscoped` sits. The
/// named set must be a superset of ≥1 declared `@scope` alternative of each scoped
/// model the callable touches (`E0185`).
#[derive(Debug, Clone, PartialEq)]
pub struct Scoped {
    pub names: Vec<Ident>,
    pub span: Span,
}

// ---------- Models ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub decorators: Vec<Decorator>,
    /// `@scope Name[, Name]*` decorators. A distinct form from the generic
    /// parenthesized `decorators` (bare names, no predicate — the predicate lives in
    /// the `scope` decl). Each entry is one alternative.
    pub scopes: Vec<ScopeRef>,
    pub name: Ident,
    pub members: Vec<Member>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
// `Field` dwarfs the other variants but is kept unboxed on purpose: this build-once
// AST's match ergonomics matter more than the layout win.
#[allow(clippy::large_enum_variant)]
pub enum Member {
    Field(Field),
    Index(IndexDecl),
    SoftOverride(SoftOverride),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: Ident,
    pub ty: TypeExpr,
    pub inverse: Option<InverseRef>,
    pub modifiers: Vec<Modifier>,
    pub relation_on: Option<Predicate>,
    pub sort: Option<Vec<SortTerm>>,
    /// `@was("old_col")` — the field's previous physical column name, driving a clean
    /// `rename column` in the diff instead of drop+add. Transient by nature (removed
    /// once the rename is captured). The `Ident` carries the old name (unquoted) and
    /// the string-literal span for diagnostics.
    pub was: Option<Ident>,
    /// `@fk` / `@fk("reason", on_delete: cascade, on_update: cascade)` — opt this forward
    /// to-one relation into a real FK constraint (with optional referential actions).
    pub fk: Option<FkAnnot>,
    /// `@no_fk` / `@no_fk("reason")` — opt this forward relation out of an FK constraint.
    pub no_fk: Option<NoFkAnnot>,
    pub span: Span,
}

/// `@fk(…)` on a forward relation — opt it into a real foreign-key constraint. `reason`
/// is a leading positional string, required only when this decorator flips FK presence
/// *against* the project's toml `foreign_keys` convention (checked in the
/// manifest-dependent pass). `on_delete`/`on_update` are independent optional kwargs; a
/// bare `@fk` (both absent) is the DB-default action — no `ON DELETE`/`ON UPDATE` clause.
/// Each action carries the raw keyword verbatim so sema can validate it and the formatter
/// can reprint it.
#[derive(Debug, Clone, PartialEq)]
pub struct FkAnnot {
    pub reason: Option<Spanned<String>>,
    pub on_delete: Option<Spanned<String>>,
    pub on_update: Option<Spanned<String>>,
    pub span: Span,
}

/// `@no_fk(…)` on a forward relation (edge) or a model (whole table) — opt out of the FK
/// constraint. `reason` is required only when opting out *against* a `foreign_keys = "all"`
/// convention (checked in the manifest-dependent pass).
#[derive(Debug, Clone, PartialEq)]
pub struct NoFkAnnot {
    pub reason: Option<Spanned<String>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeExpr {
    pub base: BaseType,
    pub optional: bool,
    pub many: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BaseType {
    Primitive(Primitive),
    /// UpperCamel model reference; resolves to a declared model.
    Model(Ident),
    /// `raw("geometry(Point,4326)")` — a DB type the engine does not model. Only a
    /// model field may carry one.
    Raw(RawSpec),
}

/// An opaque `raw(…)` body: a DB type name or an index definition the engine stores and
/// compares as a literal string but never interprets. One string for every target, or a
/// per-dialect map when the spelling differs.
#[derive(Debug, Clone, PartialEq)]
pub struct RawSpec {
    pub body: RawSpecBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RawSpecBody {
    /// `raw("…")` — one literal for every compile target.
    All(Spanned<String>),
    /// `raw({ postgres: "…", mariadb: "…" })` — per-dialect literals, in source order.
    PerDialect(Vec<RawDialect>),
}

/// One `dialect: "literal"` entry of a per-dialect `raw({ … })` map.
#[derive(Debug, Clone, PartialEq)]
pub struct RawDialect {
    pub dialect: Ident,
    pub text: Spanned<String>,
}

impl RawSpec {
    /// The literal for a compile target, or `None` when a per-dialect map omits it.
    pub fn for_dialect(&self, dialect: &str) -> Option<&str> {
        match &self.body {
            RawSpecBody::All(s) => Some(s.node.as_str()),
            RawSpecBody::PerDialect(es) => es
                .iter()
                .find(|e| e.dialect.node == dialect)
                .map(|e| e.text.node.as_str()),
        }
    }

    /// Every literal this spec carries, with its span — for emptiness checks.
    pub fn literals(&self) -> Vec<&Spanned<String>> {
        match &self.body {
            RawSpecBody::All(s) => vec![s],
            RawSpecBody::PerDialect(es) => es.iter().map(|e| &e.text).collect(),
        }
    }

    /// Source-order rendering (`based fmt`).
    pub fn render(&self) -> String {
        self.render_with(false)
    }

    /// Dialect-sorted rendering — the canonical form the neutral snapshot stores, so a
    /// diff is a plain string compare no matter what order the map was written in.
    pub fn canonical(&self) -> String {
        self.render_with(true)
    }

    fn render_with(&self, sorted: bool) -> String {
        match &self.body {
            RawSpecBody::All(s) => format!("raw({})", quote(&s.node)),
            RawSpecBody::PerDialect(es) => {
                let mut parts: Vec<(String, String)> = es
                    .iter()
                    .map(|e| (e.dialect.node.clone(), quote(&e.text.node)))
                    .collect();
                if sorted {
                    parts.sort();
                }
                let inner = parts
                    .iter()
                    .map(|(d, t)| format!("{d}: {t}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("raw({{ {inner} }})")
            }
        }
    }
}

/// A double-quoted string literal with `\` and `"` escaped — the spelling both the
/// formatter and the neutral snapshot round-trip through.
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primitive {
    Text,
    Int,
    Bool,
    Timestamp,
    Date,
    Json,
    Uuid,
    Id,
    /// 64-bit binary floating point (`float`). Wire JSON number, client `f64`.
    Float,
    /// Fixed-precision base-10 numeric (`decimal(p, s)` — precision `p`, scale `s`; bare
    /// `decimal` defaults to `(38, 9)`). Carried lossless as a string end-to-end (wire
    /// JSON string, client `rust_decimal::Decimal`), never through an `f64`. `precision`
    /// and `scale` are compile-time literals (sema validates `1 ≤ s ≤ p ≤ 38`).
    Decimal {
        precision: u32,
        scale: u32,
    },
}

/// Opt-in inverse: `(Model.field)` points at the forward edge it pairs with.
#[derive(Debug, Clone, PartialEq)]
pub struct InverseRef {
    pub model: Ident,
    pub field: Ident,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Modifier {
    Unique,
    Default(DefaultVal),
    /// `(column "legacy_name")` — alias onto a legacy / reserved-word column.
    Column(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum DefaultVal {
    Lit(Literal),
    Func(FuncCall),
    /// A bare identifier default (`default pending`) — an enum variant. Only valid on
    /// an `enum`-typed column; sema checks it names a member of that enum.
    Variant(Ident),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexDecl {
    pub columns: Vec<Ident>,
    pub unique: bool,
    /// `using <method>` — the access method (`gist`/`gin`/`brin`/`hash`, MariaDB
    /// `fulltext`/`spatial`). `None` = the dialect's default (B-tree).
    pub method: Option<Ident>,
    /// `@index raw("(lower(email))")` — an opaque index body the engine records and
    /// diffs as a literal string. When set, `columns` is empty and `unique`/`method`
    /// are absent (the raw text carries whatever the index needs).
    pub raw: Option<RawSpec>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SoftOverride {
    pub op: SoftOp,
    pub raw: RawSql,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftOp {
    Restore,
    Delete,
    Read,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decorator {
    pub name: Ident,
    pub args: Vec<DecoArg>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecoArg {
    Sort(SortTerm),
    Pred(Predicate),
    Ident(Ident),
    Path(Path),
    Lit(Literal),
}

// ---------- Shapes ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub name: Ident,
    pub from: Ident,
    pub body: Vec<ShapeField>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShapeField {
    /// local same-name column
    Bare(Ident),
    /// `out = path` or `out = raw`...``
    Rename { out: Ident, value: ShapeValue },
    /// `field { ... }` — expand a relation into a sub-object
    Nest { field: Ident, body: Vec<ShapeField> },
    /// `field -> ShapeName` — expand a relation into a sub-object projected by a
    /// named shape (whose `from` model must equal the relation's target)
    NestRef { field: Ident, shape: Ident },
    /// `out = path { body }` — flatten a to-many path through a junction to the far
    /// side, hiding the junction. The first path segment is a to-many inverse edge
    /// (into the junction); the rest are forward edges to the far model, whose
    /// projection is `body`. The result is the *distinct set* of far-side rows
    /// (`Vec<far-shape>`), so a junction's link cardinality never leaks.
    Flatten {
        out: Ident,
        path: Path,
        body: Vec<ShapeField>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShapeValue {
    Path(Path),
    Raw(RawSql),
    /// `out = count()` / `out = sum(total)` — an aggregate over the shape's grouped rows.
    /// A shape with any such field is an aggregate shape (paired with a query's `group by`
    /// / `having`). Its type is set by the function: `count()` → `int`, `sum` → the
    /// column's numeric type, `avg` → `float`, `min`/`max` → the column's type; all but
    /// `count()` are nullable.
    Agg(AggCall),
}

/// One aggregate call in a shape value. `func` is the (contextual) function name —
/// `count`/`sum`/`avg`/`min`/`max`, validated in sema; `arg` is the aggregated column
/// (`None` for the arg-less `count()`).
#[derive(Debug, Clone, PartialEq)]
pub struct AggCall {
    pub func: Ident,
    pub arg: Option<Path>,
    pub span: Span,
}

// ---------- Queries / mutations / filters ---------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub name: Ident,
    pub params: Vec<Param>,
    pub ret: RetType,
    /// `scoped Name[, Name]*` — accept the standing scope(s) on the target.
    /// Mutually exclusive with `unscoped`.
    pub scoped: Option<Scoped>,
    /// `unscoped("reason")` — opt this callable out of `@scope` injection.
    pub unscoped: Option<Unscoped>,
    pub body: QueryBody,
    pub span: Span,
}

/// `unscoped("reason")` — a per-callable opt-out of `@scope` injection, the escape
/// hatch cross-scope access (admin/support/jobs) needs. The reason string is
/// mandatory (an escape hatch is never silent), greppable, and linted (`W0106` when
/// the target model carries no `@scope` to opt out of). It forfeits *only* `@scope`;
/// soft-delete still applies.
#[derive(Debug, Clone, PartialEq)]
pub struct Unscoped {
    pub reason: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum QueryBody {
    /// `;` — params are the filter
    Bare,
    /// bare params + tail clauses, e.g. `order (...)`
    Inline(Vec<Clause>),
    /// `{ get|list ... ; }`
    Block(Statement),
    /// `{ raw`…`; }` — the whole body is one raw SQL statement (raw.md's whole-query
    /// level). The engine still binds `${param}` interpolations and types the result
    /// by the declared return shape; everything else (soft-delete, scope, ordering,
    /// dialect portability) is the SQL author's.
    Raw(RawSql),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: Ident,
    pub ty: Option<TypeExpr>,
    pub binding: Option<ParamBinding>,
    pub default: Option<DefaultVal>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParamBinding {
    /// `-> edge`
    Edge(Ident),
    /// `op column`, e.g. `> created_at`
    ColOp { op: Op, col: Ident },
}

/// A callable's return form: `-> Shape` (one), `-> Shape[]` (many),
/// `-> stream Shape` (many, delivered incrementally — `stream` already means many,
/// so `stream` and `[]` never combine), or `-> ok` (a destructive mutation's
/// acknowledgement: no row survives, so there is no shape — `ack` is set and `ty`
/// holds the `ok` token for its span).
#[derive(Debug, Clone, PartialEq)]
pub struct RetType {
    pub ty: Ident,
    pub many: bool,
    pub stream: bool,
    pub ack: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub verb: Verb,
    pub model: Ident,
    pub clauses: Vec<Clause>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Get,
    List,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Clause {
    Where(Predicate),
    Order(Vec<SortTerm>),
    Page(PageClause),
    Unindexed(Unindexed),
    /// `group by (col, …)` — grouping columns for an aggregate-shape query. Every
    /// non-aggregate projected column must appear here.
    GroupBy(Vec<Path>),
    /// `having (predicate)` — filters groups by their aggregates, after grouping. Its
    /// left operands name the shape's projected aggregate aliases / group columns.
    Having(Predicate),
}

/// `unindexed(...)` — the query is knowingly unindexed: satisfies the missing-index
/// lint without declaring an index.
#[derive(Debug, Clone, PartialEq)]
pub struct Unindexed {
    pub kind: UnindexedKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UnindexedKind {
    /// `unindexed(max_rows: N)` — checked assertion: bounded-and-fine; re-fires
    /// if prod stats ever show N exceeded.
    MaxRows(u64),
    /// `unindexed(unsafe[, "reason"])` — unbounded, uncheckable; greppable.
    Unsafe(Option<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PageClause {
    pub size: u64,
    pub offset: bool,
    pub with_count: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SortTerm {
    pub path: Path,
    pub dir: SortDir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mutation {
    pub name: Ident,
    pub params: Vec<Param>,
    pub ret: RetType,
    pub guard: Option<Ident>,
    /// `scoped Name[, Name]*` — accept the standing scope(s) on the written model(s).
    /// Mutually exclusive with `unscoped`.
    pub scoped: Option<Scoped>,
    /// `unscoped("reason")` — opt this mutation out of `@scope` injection: its writes
    /// carry no scope guard *and* a `create` does not auto-set the scope column.
    pub unscoped: Option<Unscoped>,
    pub body: Vec<WriteStmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WriteStmt {
    Create {
        model: Ident,
        assigns: Vec<Assign>,
        /// `on conflict (cols) update { … }` — upsert. `None` for a plain insert.
        conflict: Option<OnConflict>,
        /// `create … as <name>` — binds this step's produced row so a later `tx` step
        /// can reference a column of it as `$name.field`. `None` for an unbound step.
        binding: Option<Ident>,
    },
    Update {
        model: Ident,
        where_: Predicate,
        assigns: Vec<Assign>,
    },
    Delete {
        model: Ident,
        where_: Predicate,
    },
    Restore {
        model: Ident,
        where_: Predicate,
    },
    HardDelete {
        model: Ident,
        where_: Predicate,
    },
    Tx(Vec<WriteStmt>),
    Raw(RawSql),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assign {
    pub col: Ident,
    pub value: AssignRhs,
}

/// `on conflict (target) update { update }` — the upsert tail of a `create`. On a
/// unique-key collision on `target` the `update` assigns run over the existing row
/// (rather than inserting a duplicate). The winning row is read back keyed on the
/// conflict target, so the mutation's declared shape decodes on both paths.
#[derive(Debug, Clone, PartialEq)]
pub struct OnConflict {
    /// The conflict target: the field(s) forming the unique key the collision is on.
    pub target: Vec<Ident>,
    /// The `update` branch assigns, applied to the existing row on a conflict — an
    /// ordinary update assign block (plain values + self-referential arithmetic).
    pub update: Vec<Assign>,
    pub span: Span,
}

/// The right-hand side of an assignment. A plain `value` in the common case (and the
/// only case a `create` accepts); an arithmetic expression over the target model's own
/// numeric columns + params for an atomic self-referential `update` (`total = total + $n`),
/// lowered to a real SQL `SET col = <expr>` rather than a read-modify-write.
#[derive(Debug, Clone, PartialEq)]
pub enum AssignRhs {
    Value(Value),
    /// `lhs op rhs`, e.g. `total + $n`. `*`/`/` bind tighter than `+`/`-`; the tree
    /// already encodes precedence and associativity. `span` covers the whole expression.
    Arith {
        lhs: Box<AssignRhs>,
        op: ArithOp,
        rhs: Box<AssignRhs>,
        span: Span,
    },
}

impl AssignRhs {
    /// The plain value, when the RHS is a single `value` (not an arithmetic expression).
    /// Lets the many sites that only handle the simple form keep their `Value` logic.
    pub fn as_value(&self) -> Option<&Value> {
        match self {
            AssignRhs::Value(v) => Some(v),
            AssignRhs::Arith { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamedFilter {
    pub name: Ident,
    pub params: Vec<Param>,
    pub pred: Predicate,
    pub span: Span,
}

// ---------- Predicate language (shared everywhere) ------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Predicate {
    Or(Box<Predicate>, Box<Predicate>),
    And(Box<Predicate>, Box<Predicate>),
    Not(Box<Predicate>),
    Cmp {
        path: Path,
        op: Op,
        value: Value,
    },
    /// `path in (v, v, …)` — membership over an explicit value list. Each element
    /// is an ordinary `Value` (literal, enum variant, `$param`, column). The single
    /// bare-value form `path in $param` stays `Cmp { op: Op::In, .. }`.
    InList {
        path: Path,
        values: Vec<Value>,
    },
    /// bare bool column, e.g. `active`
    Bare(Path),
    FilterCall {
        name: Ident,
        args: Vec<Value>,
    },
    Raw(RawSql),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
    Like,
    In,
    Has,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Param(ParamRef),
    Path(Path),
    Lit(Literal),
    Func(FuncCall),
}

/// `$name`, `$ctx.org`, or `$step.field` — a param, `$ctx` bag field, or a `tx` step
/// binding (`create … as step`) reference; `$` unifies to "a value bound in this callable".
#[derive(Debug, Clone, PartialEq)]
pub struct ParamRef {
    pub name: Ident,
    pub path: Vec<Ident>,
}

/// Dotted traversal: `address.city.name`.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub segments: Vec<Ident>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FuncCall {
    pub name: Ident,
    pub args: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Str(String),
    Int(i64),
    /// A fractional numeric literal, carried as its exact source text (`"9.99"`) rather
    /// than an `f64` — so a `decimal` default / value is byte-exact (no float rounding,
    /// no dropped trailing zero). A `float` uses it too; the text parses to `f64` when a
    /// float context needs a number.
    Decimal(String),
    Bool(bool),
    Null,
}

// ---------- Raw SQL escape hatch ------------------------------------------

/// `raw`...`` with `${param}` / `{ident}` interpolations preserved as parts so the
/// engine can bind params and lint soft-delete gaps.
#[derive(Debug, Clone, PartialEq)]
pub struct RawSql {
    pub parts: Vec<RawPart>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RawPart {
    Text(String),
    /// `${...}` — bound parameter
    Param(ParamRef),
    /// `{table}`, `{id}` — engine-provided safe interpolation
    Engine(Ident),
}
