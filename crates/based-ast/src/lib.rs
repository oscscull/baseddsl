//! based-ast — the shared AST vocabulary.
//!
//! Mirrors `spec/grammar.ebnf` node-for-node. No logic lives here; parser builds
//! these, sema/codegen read them. Spans ride the nodes that produce diagnostics.

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

/// An identifier token. Casing is load-bearing (decisions.md D7) — preserved verbatim.
pub type Ident = Spanned<String>;

// ---------- Files ----------------------------------------------------------
// One extension (`.bsl`); the grammar is uniform across files. Any declaration
// may appear in any file. Splitting schema vs access into separate files
// (e.g. `product/model.bsl` + `product/queries.bsl`) is a recommended
// convention, not enforced (decisions.md D6/D9).

/// A parsed source file: an ordered list of top-level declarations.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaFile {
    pub decls: Vec<Decl>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decl {
    Model(Model),
    Shape(Shape),
    Query(Query),
    Mutation(Mutation),
    Filter(NamedFilter),
}

// ---------- Models ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub decorators: Vec<Decorator>,
    pub name: Ident,
    pub members: Vec<Member>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
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
    /// UpperCamel model reference; resolves to a declared model (D7).
    Model(Ident),
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
    /// `(column "legacy_name")` — alias onto a legacy / reserved-word column (D8).
    Column(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum DefaultVal {
    Lit(Literal),
    Func(FuncCall),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexDecl {
    pub columns: Vec<Ident>,
    pub unique: bool,
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
    /// `out = path` or `out = sql`...``
    Rename { out: Ident, value: ShapeValue },
    /// `field { ... }` — expand a relation into a sub-object
    Nest { field: Ident, body: Vec<ShapeField> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShapeValue {
    Path(Path),
    Raw(RawSql),
}

// ---------- Queries / mutations / filters ---------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub name: Ident,
    pub params: Vec<Param>,
    pub ret: RetType,
    pub body: QueryBody,
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

#[derive(Debug, Clone, PartialEq)]
pub struct RetType {
    pub ty: Ident,
    pub many: bool,
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
    pub body: Vec<WriteStmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WriteStmt {
    Create { model: Ident, assigns: Vec<Assign> },
    Update { model: Ident, where_: Predicate, assigns: Vec<Assign> },
    Delete { model: Ident, where_: Predicate },
    Restore { model: Ident, where_: Predicate },
    HardDelete { model: Ident, where_: Predicate },
    Tx(Vec<WriteStmt>),
    Raw(RawSql),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assign {
    pub col: Ident,
    pub value: Value,
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
    Cmp { path: Path, op: Op, value: Value },
    /// bare bool column, e.g. `active`
    Bare(Path),
    FilterCall { name: Ident, args: Vec<Value> },
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

/// `$name` or `$ctx.org` (decisions.md D4).
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
    Float(f64),
    Bool(bool),
    Null,
}

// ---------- Raw SQL escape hatch ------------------------------------------

/// `sql`...`` with `${param}` / `{ident}` interpolations preserved as parts so
/// the engine can bind params and lint soft-delete gaps (raw.md).
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
