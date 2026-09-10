//! The line-based layout engine: classify source lines as blank/comment/code,
//! walk each declaration, and emit body lines in the canonical layout while
//! reproducing full-line comments in their original slots.

use based_ast::*;

use crate::*;

/// Shapes wider than this (rendered on one line) break onto a line per field.
const SHAPE_INLINE_MAX: usize = 46;
const INDENT: &str = "  ";

#[derive(Clone)]
enum LineKind {
    Blank,
    Comment(String),
    Code,
}

pub(crate) struct Printer {
    /// Byte offset of each line's first byte.
    line_starts: Vec<usize>,
    /// Classification of each line, parallel to `line_starts`.
    lines: Vec<LineKind>,
    out: Vec<String>,
}

impl Printer {
    pub(crate) fn new(src: &str) -> Self {
        let mut line_starts = vec![0usize];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        let mut lines = Vec::with_capacity(line_starts.len());
        for (i, &start) in line_starts.iter().enumerate() {
            let end = line_starts.get(i + 1).map_or(src.len(), |&n| n - 1);
            let text = src[start..end].trim_end();
            lines.push(if text.is_empty() {
                LineKind::Blank
            } else if text.trim_start().starts_with('#') {
                LineKind::Comment(text.trim_start().to_string())
            } else {
                LineKind::Code
            });
        }
        Self {
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

    /// Emit the comment lines on `[from, to)` re-indented to a body at `indent` levels
    /// (blank lines between body members are dropped). Used to reproduce comments that sit
    /// between — or after — the members of a model / mutation / shape body.
    fn emit_body_comments(&mut self, from: usize, to: usize, indent: usize) {
        let pad = INDENT.repeat(indent);
        for i in from..to.min(self.lines.len()) {
            if let LineKind::Comment(c) = &self.lines[i] {
                self.out.push(format!("{pad}{c}"));
            }
        }
    }

    pub(crate) fn file(mut self, decls: &[Decl]) -> String {
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
        header.sort_by_key(HeaderItem::start);

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
        let close = self.line_of((m.span.end as usize).saturating_sub(1));
        let mut cursor = self.line_of(m.name.span.start as usize) + 1;
        for member in &m.members {
            let sp = member_span(member);
            let ml = self.line_of(sp.start as usize);
            self.emit_body_comments(cursor, ml.max(cursor), 1);
            cursor = self.line_of((sp.end as usize).saturating_sub(1)) + 1;
            self.out.push(match member {
                Member::Field(f) => format!("{INDENT}{}", field(f, name_w, inverse_w)),
                Member::Index(ix) => format!("{INDENT}{}", index_decl(ix)),
                Member::SoftOverride(so) => {
                    format!("{INDENT}{}: {}", soft_op(so.op), raw_sql(&so.raw))
                }
                Member::Generated(g) => {
                    format!("{INDENT}{} = {}", g.name.node, shape_expr(&g.expr, 0))
                }
            });
        }
        self.emit_body_comments(cursor, close, 1);
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
                ShapeField::Rename { out, .. } | ShapeField::Flatten { out, .. } => {
                    Some(out.node.len())
                }
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let close = self.line_of((s.span.end as usize).saturating_sub(1));
        let mut cursor = self.line_of(s.name.span.start as usize) + 1;
        for f in &s.body {
            if let Some(st) = shape_field_start(f) {
                let fl = self.line_of(st as usize);
                self.emit_body_comments(cursor, fl.max(cursor), 1);
                cursor = fl + 1;
            }
            self.out
                .push(format!("{INDENT}{}", shape_field_block(f, rename_w)));
        }
        self.emit_body_comments(cursor, close, 1);
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
            .push(format!("{INDENT}{} {}", verb_head(stmt), stmt.model.node));
        let last = stmt.clauses.len() - 1;
        for (i, c) in stmt.clauses.iter().enumerate() {
            // The trailing `for update` (if any) takes the `;`, so the last clause doesn't.
            let semi = if i == last && stmt.for_update.is_none() {
                ";"
            } else {
                ""
            };
            self.out
                .push(format!("{INDENT}{INDENT}{}{semi}", clause(c)));
        }
        if let Some(wait) = stmt.for_update {
            self.out
                .push(format!("{INDENT}{INDENT}{};", for_update_modifier(wait)));
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
        let open = self.line_of(m.name.span.start as usize);
        let close = self.line_of((m.span.end as usize).saturating_sub(1));
        self.write_body(&m.body, open, close, 1);
        self.out.push("}".to_string());
    }

    /// Render a sequence of write statements (a mutation body or a `tx` block), reproducing
    /// the comments that sit between them and after the last one. Statement start lines come
    /// from each write's model ident (`write_start`); a non-`tx` statement prints on a single
    /// line, so its start line is also its extent.
    fn write_body(
        &mut self,
        writes: &[WriteStmt],
        open_line: usize,
        close_line: usize,
        indent: usize,
    ) {
        let pad = INDENT.repeat(indent);
        let mut cursor = open_line + 1;
        for w in writes {
            if let Some(st) = write_start(w) {
                let wl = self.line_of(st as usize);
                self.emit_body_comments(cursor, wl.max(cursor), indent);
                cursor = wl + 1;
            }
            match w {
                WriteStmt::Tx(inner) => {
                    self.out.push(format!("{pad}tx {{"));
                    let inner_open = inner
                        .first()
                        .and_then(write_start)
                        .map_or(cursor, |s| self.line_of(s as usize));
                    let inner_close = inner
                        .last()
                        .and_then(write_start)
                        .map_or(inner_open, |s| self.line_of(s as usize) + 1);
                    self.write_body(inner, inner_open.saturating_sub(1), inner_close, indent + 1);
                    self.out.push(format!("{pad}}}"));
                    cursor = inner_close;
                }
                _ => self.out.push(format!("{pad}{};", write_line(w))),
            }
        }
        self.emit_body_comments(cursor, close_line, indent);
    }
}

/// Join the output lines: collapse blank runs to a single blank, drop leading and
/// trailing blanks, and end on exactly one newline.
fn finish(lines: Vec<String>) -> String {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for line in lines {
        if line.is_empty() && out.last().is_none_or(std::string::String::is_empty) {
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(std::string::String::is_empty) {
        out.pop();
    }
    if out.is_empty() {
        return String::new();
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}
