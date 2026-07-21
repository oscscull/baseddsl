//! based-fmt — the canonical `.bsl` source formatter.
//!
//! One entry point, [`format_source`]: parse a `.bsl` source, then pretty-print its
//! declarations in the canonical layout the worked examples use. Deterministic and
//! idempotent — `format(format(x)) == format(x)` — and structure-preserving:
//! re-parsing the output yields the same declarations.
//!
//! Comments are lexer-skipped, so the printer recovers them from the source text
//! directly. Every `.bsl` comment is a full line at column 0 (before a declaration
//! or between a model's decorators), never inside a body; the printer reproduces
//! each verbatim in its original slot.

use based_ast::*;
use based_diagnostics::Diagnostic;

/// Shapes wider than this (rendered on one line) break onto a line per field.
const SHAPE_INLINE_MAX: usize = 46;
const INDENT: &str = "  ";

/// Format a `.bsl` source into its canonical layout. `Err` carries the parse
/// diagnostics when the source does not parse (an unparseable file can't be
/// formatted).
pub fn format_source(src: &str) -> Result<String, Vec<Diagnostic>> {
    let parsed = based_parser::parse_file(src, FileId(0))?;
    Ok(Printer::new(src).file(&parsed.decls))
}

/// Whether a source is already canonically formatted.
pub fn is_formatted(src: &str) -> Result<bool, Vec<Diagnostic>> {
    Ok(format_source(src)? == src)
}

#[derive(Clone)]
enum LineKind {
    Blank,
    Comment(String),
    Code,
}

struct Printer {
    /// Byte offset of each line's first byte.
    line_starts: Vec<usize>,
    /// Classification of each line, parallel to `line_starts`.
    lines: Vec<LineKind>,
    out: Vec<String>,
}

impl Printer {
    fn new(src: &str) -> Self {
        let mut line_starts = vec![0usize];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        let mut lines = Vec::with_capacity(line_starts.len());
        for (i, &start) in line_starts.iter().enumerate() {
            let end = line_starts.get(i + 1).map(|&n| n - 1).unwrap_or(src.len());
            let text = src[start..end].trim_end();
            lines.push(if text.is_empty() {
                LineKind::Blank
            } else if text.trim_start().starts_with('#') {
                LineKind::Comment(text.trim_start().to_string())
            } else {
                LineKind::Code
            });
        }
        Printer {
            line_starts,
            lines,
            out: Vec::new(),
        }
    }

    /// Line index containing byte `offset` (the last line whose start is `<= offset`).
    fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(l) => l,
            Err(l) => l - 1,
        }
    }

    /// Emit the comment/blank trivia on source lines `[from, to)`. Comments print
    /// verbatim; blank runs collapse to a single blank in the final pass.
    fn emit_trivia(&mut self, from: usize, to: usize) {
        for i in from..to {
            match &self.lines[i] {
                LineKind::Comment(c) => self.out.push(c.clone()),
                LineKind::Blank => self.out.push(String::new()),
                LineKind::Code => {}
            }
        }
    }

    /// Emit only the comment lines on `[from, to)` (used within a model header, where
    /// blank lines between decorators are dropped).
    fn emit_header_comments(&mut self, from: usize, to: usize) {
        for i in from..to {
            if let LineKind::Comment(c) = &self.lines[i] {
                self.out.push(c.clone());
            }
        }
    }

    fn file(mut self, decls: &[Decl]) -> String {
        let mut cursor = 0usize;
        for decl in decls {
            let start = decl_start(decl) as usize;
            let end = decl_span(decl).end as usize;
            self.emit_trivia(cursor, self.line_of(start));
            self.decl(decl);
            cursor = self.line_of(end.saturating_sub(1)) + 1;
        }
        self.emit_trivia(cursor, self.lines.len());
        finish(self.out)
    }

    fn decl(&mut self, decl: &Decl) {
        match decl {
            Decl::Model(m) => self.model(m),
            Decl::Shape(s) => self.shape(s),
            Decl::Scope(s) => self.out.push(scope_decl(s)),
            Decl::Enum(e) => self.out.push(enum_decl(e)),
            Decl::Query(q) => self.query(q),
            Decl::Mutation(m) => self.mutation(m),
            Decl::Filter(f) => self.out.push(named_filter(f)),
        }
    }

    // ---------- models -----------------------------------------------------

    fn model(&mut self, m: &Model) {
        // Decorators + `@scope` refs share the header, one per line, in source order.
        let mut header: Vec<HeaderItem> = Vec::new();
        header.extend(m.decorators.iter().map(HeaderItem::Deco));
        header.extend(m.scopes.iter().map(HeaderItem::Scope));
        header.sort_by_key(|h| h.start());

        let mut prev_end_line: Option<usize> = None;
        for item in &header {
            let item_line = self.line_of(item.start() as usize);
            if let Some(prev) = prev_end_line {
                self.emit_header_comments(prev + 1, item_line);
            }
            self.out.push(item.render());
            prev_end_line = Some(self.line_of(item.end().saturating_sub(1) as usize));
        }
        if let Some(prev) = prev_end_line {
            self.emit_header_comments(prev + 1, self.line_of(m.name.span.start as usize));
        }

        if m.members.is_empty() {
            self.out.push(format!("{} {{}}", m.name.node));
            return;
        }
        self.out.push(format!("{} {{", m.name.node));
        let name_w = m
            .members
            .iter()
            .filter_map(field_of)
            .map(|f| f.name.node.len())
            .max()
            .unwrap_or(0);
        let inverse_w = m
            .members
            .iter()
            .filter_map(field_of)
            .filter(|f| f.inverse.is_some())
            .map(|f| type_expr(&f.ty).len())
            .max()
            .unwrap_or(0);
        for member in &m.members {
            self.out.push(match member {
                Member::Field(f) => format!("{INDENT}{}", field(f, name_w, inverse_w)),
                Member::Index(ix) => format!("{INDENT}{}", index_decl(ix)),
                Member::SoftOverride(so) => {
                    format!("{INDENT}{}: {}", soft_op(so.op), raw_sql(&so.raw))
                }
            });
        }
        self.out.push("}".to_string());
    }

    // ---------- shapes -----------------------------------------------------

    fn shape(&mut self, s: &Shape) {
        let inline = format!(
            "shape {} from {} {{ {} }}",
            s.name.node,
            s.from.node,
            s.body
                .iter()
                .map(shape_field_inline)
                .collect::<Vec<_>>()
                .join(", ")
        );
        if s.body.is_empty() {
            self.out
                .push(format!("shape {} from {} {{}}", s.name.node, s.from.node));
            return;
        }
        if inline.len() <= SHAPE_INLINE_MAX {
            self.out.push(inline);
            return;
        }
        self.out
            .push(format!("shape {} from {} {{", s.name.node, s.from.node));
        let rename_w = s
            .body
            .iter()
            .filter_map(|f| match f {
                ShapeField::Rename { out, .. } => Some(out.node.len()),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        for f in &s.body {
            self.out
                .push(format!("{INDENT}{}", shape_field_block(f, rename_w)));
        }
        self.out.push("}".to_string());
    }

    // ---------- queries ----------------------------------------------------

    fn query(&mut self, q: &Query) {
        let prefix = format!(
            "query {}({}) -> {}{}",
            q.name.node,
            params(&q.params),
            ret_type(&q.ret),
            scope_ack(q.scoped.as_ref(), q.unscoped.as_ref()),
        );
        match &q.body {
            QueryBody::Bare => self.out.push(format!("{prefix};")),
            QueryBody::Inline(clauses) => {
                let tail: String = clauses.iter().map(|c| format!(" {}", clause(c))).collect();
                self.out.push(format!("{prefix}{tail};"));
            }
            QueryBody::Block(stmt) => {
                if stmt.clauses.len() <= 1 {
                    self.out
                        .push(format!("{prefix} {{ {} }}", statement_inline(stmt)));
                } else {
                    self.out.push(format!("{prefix} {{"));
                    self.statement_block(stmt);
                    self.out.push("}".to_string());
                }
            }
            // A whole-query raw body: the SQL text is opaque and reprints
            // byte-exactly; a multi-line block keeps its own layout.
            QueryBody::Raw(raw) => {
                let sql = raw_sql(raw);
                if sql.contains('\n') {
                    self.out.push(format!("{prefix} {{"));
                    self.out.push(format!("{INDENT}{sql};"));
                    self.out.push("}".to_string());
                } else {
                    self.out.push(format!("{prefix} {{ {sql}; }}"));
                }
            }
        }
    }

    /// The read statement inside an expanded query block (2+ clauses). Two or fewer
    /// clauses stay on one line; three or more break a clause per line.
    fn statement_block(&mut self, stmt: &Statement) {
        if stmt.clauses.len() <= 2 {
            self.out.push(format!("{INDENT}{}", statement_inline(stmt)));
            return;
        }
        self.out
            .push(format!("{INDENT}{} {}", verb(stmt.verb), stmt.model.node));
        let last = stmt.clauses.len() - 1;
        for (i, c) in stmt.clauses.iter().enumerate() {
            let semi = if i == last { ";" } else { "" };
            self.out
                .push(format!("{INDENT}{INDENT}{}{semi}", clause(c)));
        }
    }

    // ---------- mutations --------------------------------------------------

    fn mutation(&mut self, m: &Mutation) {
        let guard = match &m.guard {
            Some(g) => format!(" guard {}", g.node),
            None => String::new(),
        };
        self.out.push(format!(
            "mutation {}({}) -> {}{guard}{} {{",
            m.name.node,
            params(&m.params),
            ret_type(&m.ret),
            scope_ack(m.scoped.as_ref(), m.unscoped.as_ref()),
        ));
        for w in &m.body {
            self.write_stmt(w, 1);
        }
        self.out.push("}".to_string());
    }

    fn write_stmt(&mut self, w: &WriteStmt, indent: usize) {
        let pad = INDENT.repeat(indent);
        match w {
            WriteStmt::Tx(inner) => {
                self.out.push(format!("{pad}tx {{"));
                for w in inner {
                    self.write_stmt(w, indent + 1);
                }
                self.out.push(format!("{pad}}}"));
            }
            _ => self.out.push(format!("{pad}{};", write_line(w))),
        }
    }
}

// ---------- header items ---------------------------------------------------

enum HeaderItem<'a> {
    Deco(&'a Decorator),
    Scope(&'a ScopeRef),
}

impl HeaderItem<'_> {
    fn start(&self) -> u32 {
        match self {
            HeaderItem::Deco(d) => d.span.start,
            HeaderItem::Scope(s) => s.span.start,
        }
    }
    fn end(&self) -> u32 {
        match self {
            HeaderItem::Deco(d) => d.span.end,
            HeaderItem::Scope(s) => s.span.end,
        }
    }
    fn render(&self) -> String {
        match self {
            HeaderItem::Deco(d) => decorator(d),
            HeaderItem::Scope(s) => format!(
                "@scope {}",
                s.names
                    .iter()
                    .map(|n| n.node.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

// ---------- declaration span helpers ---------------------------------------

fn decl_span(d: &Decl) -> Span {
    match d {
        Decl::Model(m) => m.span,
        Decl::Shape(s) => s.span,
        Decl::Scope(s) => s.span,
        Decl::Enum(e) => e.span,
        Decl::Query(q) => q.span,
        Decl::Mutation(m) => m.span,
        Decl::Filter(f) => f.span,
    }
}
fn decl_start(d: &Decl) -> u32 {
    decl_span(d).start
}

fn field_of(m: &Member) -> Option<&Field> {
    match m {
        Member::Field(f) => Some(f),
        _ => None,
    }
}

// ---------- decorators / scopes --------------------------------------------

fn decorator(d: &Decorator) -> String {
    if d.args.is_empty() {
        format!("@{}", d.name.node)
    } else {
        format!(
            "@{}({})",
            d.name.node,
            d.args.iter().map(deco_arg).collect::<Vec<_>>().join(", ")
        )
    }
}

fn deco_arg(a: &DecoArg) -> String {
    match a {
        DecoArg::Sort(s) => sort_term(s),
        DecoArg::Pred(p) => predicate(p, 0),
        DecoArg::Ident(i) => i.node.clone(),
        DecoArg::Path(p) => path(p),
        DecoArg::Lit(l) => literal(l),
    }
}

/// `enum Name { a, b = "B", c }` — one line, variants comma-joined (a closed value set
/// reads as a compact list, unlike a model's field-per-line body). An explicit variant
/// value (`= "PAID"` / `= 0`) is preserved; a bare variant stays bare.
fn enum_decl(e: &EnumDecl) -> String {
    format!(
        "enum {} {{ {} }}",
        e.name.node,
        e.variants
            .iter()
            .map(enum_variant)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn enum_variant(v: &EnumVariant) -> String {
    match v.value.as_ref().map(|s| &s.node) {
        None => v.name.node.clone(),
        Some(VariantValue::Str(s)) => format!("{} = \"{}\"", v.name.node, esc(s)),
        Some(VariantValue::Int(n)) => format!("{} = {}", v.name.node, n),
    }
}

fn scope_decl(s: &ScopeDecl) -> String {
    format!(
        "scope {} ({})",
        s.name.node,
        s.terms
            .iter()
            .map(|t| format!(
                "{}: {} = {}",
                t.col.node,
                type_expr(&t.ty),
                param_ref(&t.ctx)
            ))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

// ---------- fields ---------------------------------------------------------

/// Render one field line body (no indent). `name_w` aligns the type column across
/// the model's fields; `inverse_w` aligns the inverse-ref column across the fields
/// that carry one.
fn field(f: &Field, name_w: usize, inverse_w: usize) -> String {
    let ty = type_expr(&f.ty);
    let mut s = format!(
        "{:<width$} {}",
        format!("{}:", f.name.node),
        ty,
        width = name_w + 1
    );
    if let Some(iv) = &f.inverse {
        let pad = inverse_w.saturating_sub(ty.len());
        s.push_str(&" ".repeat(pad + 1));
        s.push_str(&format!("({}.{})", iv.model.node, iv.field.node));
    }
    if !f.modifiers.is_empty() {
        s.push(' ');
        s.push_str(&modifiers_group(&f.modifiers));
    }
    if let Some(pred) = &f.relation_on {
        s.push_str(&format!(" (on: {})", predicate(pred, 0)));
    }
    if let Some(w) = &f.was {
        s.push_str(&format!(" @was(\"{}\")", esc(&w.node)));
    }
    if let Some(sort) = &f.sort {
        s.push_str(&format!(
            " @sort({})",
            sort.iter().map(sort_term).collect::<Vec<_>>().join(", ")
        ));
    }
    s
}

fn modifiers_group(mods: &[Modifier]) -> String {
    format!(
        "({})",
        mods.iter().map(modifier).collect::<Vec<_>>().join(", ")
    )
}

fn modifier(m: &Modifier) -> String {
    match m {
        Modifier::Unique => "unique".to_string(),
        Modifier::Default(v) => format!("default {}", default_val(v)),
        Modifier::Column(c) => format!("column \"{}\"", esc(c)),
    }
}

fn index_decl(ix: &IndexDecl) -> String {
    let cols = if ix.columns.len() == 1 {
        ix.columns[0].node.clone()
    } else {
        format!(
            "({})",
            ix.columns
                .iter()
                .map(|c| c.node.clone())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let sep = if ix.columns.len() == 1 { " " } else { "" };
    format!(
        "@index{sep}{cols}{}",
        if ix.unique { " unique" } else { "" }
    )
}

fn soft_op(op: SoftOp) -> &'static str {
    match op {
        SoftOp::Restore => "restore",
        SoftOp::Delete => "delete",
        SoftOp::Read => "read",
    }
}

// ---------- shapes ---------------------------------------------------------

fn shape_field_inline(f: &ShapeField) -> String {
    match f {
        ShapeField::Bare(id) => id.node.clone(),
        ShapeField::Rename { out, value } => format!("{} = {}", out.node, shape_value(value)),
        ShapeField::Nest { field, body } => format!(
            "{} {{ {} }}",
            field.node,
            body.iter()
                .map(shape_field_inline)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ShapeField::NestRef { field, shape } => format!("{} -> {}", field.node, shape.node),
    }
}

fn shape_field_block(f: &ShapeField, rename_w: usize) -> String {
    match f {
        ShapeField::Bare(id) => id.node.clone(),
        ShapeField::Rename { out, value } => {
            format!("{:<rename_w$} = {}", out.node, shape_value(value))
        }
        ShapeField::Nest { field, body } => format!(
            "{} {{ {} }}",
            field.node,
            body.iter()
                .map(shape_field_inline)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ShapeField::NestRef { field, shape } => format!("{} -> {}", field.node, shape.node),
    }
}

fn shape_value(v: &ShapeValue) -> String {
    match v {
        ShapeValue::Path(p) => path(p),
        ShapeValue::Raw(r) => raw_sql(r),
        ShapeValue::Agg(a) => aggregate(a),
    }
}

fn aggregate(a: &AggCall) -> String {
    match &a.arg {
        Some(p) => format!("{}({})", a.func.node, path(p)),
        None => format!("{}()", a.func.node),
    }
}

// ---------- queries / statements / clauses ---------------------------------

fn statement_inline(stmt: &Statement) -> String {
    let mut s = format!("{} {}", verb(stmt.verb), stmt.model.node);
    for c in &stmt.clauses {
        s.push(' ');
        s.push_str(&clause(c));
    }
    s.push(';');
    s
}

fn verb(v: Verb) -> &'static str {
    match v {
        Verb::Get => "get",
        Verb::List => "list",
    }
}

fn clause(c: &Clause) -> String {
    match c {
        Clause::Where(p) => format!("where ({})", predicate(p, 0)),
        Clause::Order(terms) => format!(
            "order ({})",
            terms.iter().map(sort_term).collect::<Vec<_>>().join(", ")
        ),
        Clause::Page(pc) => {
            let mut s = format!("page ({})", pc.size);
            if pc.offset {
                s.push_str(" offset");
            }
            if pc.with_count {
                s.push_str(" with count");
            }
            s
        }
        Clause::Unindexed(u) => match &u.kind {
            UnindexedKind::MaxRows(n) => format!("unindexed(max_rows: {n})"),
            UnindexedKind::Unsafe(None) => "unindexed(unsafe)".to_string(),
            UnindexedKind::Unsafe(Some(r)) => format!("unindexed(unsafe, \"{}\")", esc(r)),
        },
        Clause::GroupBy(cols) => format!(
            "group by ({})",
            cols.iter().map(path).collect::<Vec<_>>().join(", ")
        ),
        Clause::Having(p) => format!("having ({})", predicate(p, 0)),
    }
}

fn ret_type(r: &RetType) -> String {
    format!(
        "{}{}{}",
        if r.stream { "stream " } else { "" },
        r.ty.node,
        if r.many { "[]" } else { "" }
    )
}

fn scope_ack(scoped: Option<&Scoped>, unscoped: Option<&Unscoped>) -> String {
    if let Some(s) = scoped {
        format!(
            " scoped {}",
            s.names
                .iter()
                .map(|n| n.node.clone())
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else if let Some(u) = unscoped {
        format!(" unscoped(\"{}\")", esc(&u.reason))
    } else {
        String::new()
    }
}

fn params(ps: &[Param]) -> String {
    ps.iter().map(param).collect::<Vec<_>>().join(", ")
}

fn param(p: &Param) -> String {
    let mut s = p.name.node.clone();
    if let Some(ty) = &p.ty {
        s.push_str(&format!(": {}", type_expr(ty)));
    }
    match &p.binding {
        Some(ParamBinding::Edge(e)) => s.push_str(&format!(" -> {}", e.node)),
        Some(ParamBinding::ColOp { op, col }) => {
            s.push_str(&format!(" {} {}", op_str(*op), col.node))
        }
        None => {}
    }
    if let Some(d) = &p.default {
        s.push_str(&format!(" = {}", default_val(d)));
    }
    s
}

// ---------- mutations ------------------------------------------------------

fn write_line(w: &WriteStmt) -> String {
    match w {
        WriteStmt::Create { model, assigns } => {
            format!("create {} {}", model.node, assign_block(assigns))
        }
        WriteStmt::Update {
            model,
            where_,
            assigns,
        } => format!(
            "update {} where ({}) {}",
            model.node,
            predicate(where_, 0),
            assign_block(assigns)
        ),
        WriteStmt::Delete { model, where_ } => {
            format!("delete {} where ({})", model.node, predicate(where_, 0))
        }
        WriteStmt::Restore { model, where_ } => {
            format!("restore {} where ({})", model.node, predicate(where_, 0))
        }
        WriteStmt::HardDelete { model, where_ } => {
            format!(
                "hard delete {} where ({})",
                model.node,
                predicate(where_, 0)
            )
        }
        WriteStmt::Raw(r) => raw_sql(r),
        // Tx is handled by the caller (it spans multiple lines).
        WriteStmt::Tx(_) => String::new(),
    }
}

fn assign_block(assigns: &[Assign]) -> String {
    if assigns.is_empty() {
        return "{}".to_string();
    }
    format!(
        "{{ {} }}",
        assigns
            .iter()
            .map(|a| format!("{} = {}", a.col.node, assign_rhs(&a.value)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn named_filter(f: &NamedFilter) -> String {
    let ps = if f.params.is_empty() {
        String::new()
    } else {
        format!("({})", params(&f.params))
    };
    format!("filter {}{} = {};", f.name.node, ps, predicate(&f.pred, 0))
}

// ---------- predicates -----------------------------------------------------

/// Precedence of the top operator: `or` < `and` < everything atomic. `min` is the
/// precedence the enclosing context requires — a lower-precedence child is wrapped.
fn predicate(p: &Predicate, min: u8) -> String {
    let (prec, body) = match p {
        Predicate::Or(a, b) => (1, format!("{} or {}", predicate(a, 1), predicate(b, 2))),
        Predicate::And(a, b) => (2, format!("{} and {}", predicate(a, 2), predicate(b, 3))),
        Predicate::Not(a) => (3, format!("not {}", predicate(a, 4))),
        Predicate::Cmp {
            path: pa,
            op,
            value: v,
        } => (4, format!("{} {} {}", path(pa), op_str(*op), value(v))),
        Predicate::InList { path: pa, values } => (
            4,
            format!(
                "{} in ({})",
                path(pa),
                values.iter().map(value).collect::<Vec<_>>().join(", ")
            ),
        ),
        Predicate::Bare(pa) => (4, path(pa)),
        Predicate::FilterCall { name, args } => (
            4,
            format!(
                "{}({})",
                name.node,
                args.iter().map(value).collect::<Vec<_>>().join(", ")
            ),
        ),
        Predicate::Raw(r) => (4, raw_sql(r)),
    };
    if prec < min {
        format!("({body})")
    } else {
        body
    }
}

fn op_str(op: Op) -> &'static str {
    match op {
        Op::Eq => "=",
        Op::Ne => "!=",
        Op::Gt => ">",
        Op::Lt => "<",
        Op::Ge => ">=",
        Op::Le => "<=",
        Op::Like => "~",
        Op::In => "in",
        Op::Has => "has",
    }
}

// ---------- values / paths / literals --------------------------------------

/// An assignment RHS: a plain value, or an arithmetic expression with minimal
/// parentheses (only where precedence/associativity require them).
fn assign_rhs(r: &AssignRhs) -> String {
    arith(r, 0)
}

fn arith(r: &AssignRhs, min: u8) -> String {
    let (prec, body) = match r {
        AssignRhs::Value(v) => (3, value(v)),
        AssignRhs::Arith { lhs, op, rhs, .. } => {
            let prec = match op {
                ArithOp::Add | ArithOp::Sub => 1,
                ArithOp::Mul | ArithOp::Div => 2,
            };
            (
                prec,
                format!(
                    "{} {} {}",
                    arith(lhs, prec),
                    arith_op(*op),
                    arith(rhs, prec + 1)
                ),
            )
        }
    };
    if prec < min {
        format!("({body})")
    } else {
        body
    }
}

fn arith_op(op: ArithOp) -> &'static str {
    match op {
        ArithOp::Add => "+",
        ArithOp::Sub => "-",
        ArithOp::Mul => "*",
        ArithOp::Div => "/",
    }
}

fn value(v: &Value) -> String {
    match v {
        Value::Param(pr) => param_ref(pr),
        Value::Path(p) => path(p),
        Value::Lit(l) => literal(l),
        Value::Func(f) => func_call(f),
        Value::Back(b) => format!("^.{}", b.field.node),
    }
}

fn param_ref(pr: &ParamRef) -> String {
    let mut s = format!("${}", pr.name.node);
    for seg in &pr.path {
        s.push('.');
        s.push_str(&seg.node);
    }
    s
}

fn path(p: &Path) -> String {
    p.segments
        .iter()
        .map(|s| s.node.clone())
        .collect::<Vec<_>>()
        .join(".")
}

fn func_call(f: &FuncCall) -> String {
    format!(
        "{}({})",
        f.name.node,
        f.args.iter().map(value).collect::<Vec<_>>().join(", ")
    )
}

fn default_val(d: &DefaultVal) -> String {
    match d {
        DefaultVal::Lit(l) => literal(l),
        DefaultVal::Func(f) => func_call(f),
        DefaultVal::Variant(v) => v.node.clone(),
    }
}

fn literal(l: &Literal) -> String {
    match l {
        Literal::Str(s) => format!("\"{}\"", esc(s)),
        Literal::Int(n) => n.to_string(),
        // Emitted verbatim — the exact source text is preserved (`Literal::Decimal`).
        Literal::Decimal(s) => s.clone(),
        Literal::Bool(b) => b.to_string(),
        Literal::Null => "null".to_string(),
    }
}

fn sort_term(s: &SortTerm) -> String {
    match s.dir {
        SortDir::Desc => format!("{} desc", path(&s.path)),
        SortDir::Asc => path(&s.path),
    }
}

// ---------- types / raw sql ------------------------------------------------

fn type_expr(t: &TypeExpr) -> String {
    let mut s = match &t.base {
        BaseType::Primitive(p) => primitive(*p),
        BaseType::Model(m) => m.node.clone(),
    };
    if t.optional {
        s.push('?');
    }
    if t.many {
        s.push_str("[]");
    }
    s
}

fn primitive(p: Primitive) -> String {
    match p {
        Primitive::Text => "text".into(),
        Primitive::Int => "int".into(),
        Primitive::Bool => "bool".into(),
        Primitive::Timestamp => "timestamp".into(),
        Primitive::Date => "date".into(),
        Primitive::Json => "json".into(),
        Primitive::Uuid => "uuid".into(),
        Primitive::Id => "Id".into(),
        Primitive::Float => "float".into(),
        Primitive::Decimal { precision, scale } => format!("decimal({precision}, {scale})"),
    }
}

fn raw_sql(r: &RawSql) -> String {
    let mut s = String::from("raw`");
    for part in &r.parts {
        match part {
            RawPart::Text(t) => s.push_str(t),
            RawPart::Param(pr) => {
                s.push_str("${");
                s.push_str(&pr.name.node);
                for seg in &pr.path {
                    s.push('.');
                    s.push_str(&seg.node);
                }
                s.push('}');
            }
            RawPart::Engine(id) => {
                s.push('{');
                s.push_str(&id.node);
                s.push('}');
            }
        }
    }
    s.push('`');
    s
}

// ---------- small helpers --------------------------------------------------

/// Escape a string literal's body (the printer re-adds the surrounding quotes).
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Join the output lines: collapse blank runs to a single blank, drop leading and
/// trailing blanks, and end on exactly one newline.
fn finish(lines: Vec<String>) -> String {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for line in lines {
        if line.is_empty() && out.last().map(|l| l.is_empty()).unwrap_or(true) {
            continue;
        }
        out.push(line);
    }
    while out.last().map(|l| l.is_empty()).unwrap_or(false) {
        out.pop();
    }
    if out.is_empty() {
        return String::new();
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}
