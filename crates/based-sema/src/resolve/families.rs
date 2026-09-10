use super::*;

/// What a dotted path lands on: the resolved type/target, consumed by the operand
/// type-checker below.
pub enum Terminal {
    Scalar(Primitive),
    /// A relation edge; `.0` is the target model name. Comparable to its key (Id).
    Relation(String),
    /// An opaque `raw(…)` column; `.0` is the declared type spelling. Projectable, but
    /// never a filter/sort/group/aggregate operand.
    Opaque(String),
}

pub(crate) fn prim_family(ty: Primitive) -> Family {
    match ty {
        Primitive::Text
        | Primitive::Uuid
        | Primitive::Id
        | Primitive::Ulid
        | Primitive::Timestamp
        | Primitive::Date
        // `time` rides with text: string-writable and orderable (`< > <= >=`), like the
        // other temporal types.
        | Primitive::Time => Family::Textual,
        Primitive::Int | Primitive::Serial | Primitive::Float | Primitive::Decimal { .. } => {
            Family::Numeric
        }
        Primitive::Bool => Family::Bool,
        Primitive::Json => Family::Json,
        Primitive::Bytes => Family::Binary,
    }
}

pub(crate) fn terminal_family(t: &Terminal) -> Family {
    match t {
        Terminal::Scalar(p) => prim_family(*p),
        Terminal::Relation(_) => Family::Key,
        // Never reached in a comparison — an opaque operand is rejected first.
        Terminal::Opaque(_) => Family::Json,
    }
}

/// Human-facing name for a resolved operand, for diagnostics.
pub(crate) fn terminal_name(t: &Terminal) -> String {
    match t {
        Terminal::Scalar(p) => format!("`{}`", prim_name(*p)),
        Terminal::Relation(m) => format!("relation `{m}`"),
        Terminal::Opaque(t) => format!("opaque column `{t}`"),
    }
}

pub(crate) fn prim_name(p: Primitive) -> &'static str {
    match p {
        Primitive::Text => "text",
        Primitive::Int => "int",
        Primitive::Bool => "bool",
        Primitive::Timestamp => "timestamp",
        Primitive::Date => "date",
        Primitive::Time => "time",
        Primitive::Bytes => "bytes",
        Primitive::Json => "json",
        Primitive::Uuid => "uuid",
        Primitive::Id => "id",
        Primitive::Ulid => "ulid",
        Primitive::Serial => "serial",
        Primitive::Float => "float",
        Primitive::Decimal { .. } => "decimal",
    }
}

pub(crate) fn lit_family(l: &Literal) -> Option<Family> {
    match l {
        Literal::Str(_) => Some(Family::Textual),
        Literal::Int(_) | Literal::Decimal(_) => Some(Family::Numeric),
        Literal::Bool(_) => Some(Family::Bool),
        Literal::Null => None, // null is compatible with any operand
    }
}

/// Are two operand families comparable with `=`/`!=` (and, for orderable ones, the
/// relational operators)? Json matches anything; a relation key accepts either a
/// uuid string or an integer key.
pub(crate) fn compatible(a: Family, b: Family) -> bool {
    use Family::{Json, Key, Numeric, Textual};
    if a == Json || b == Json {
        return true;
    }
    match (a, b) {
        (Key, Textual | Numeric) | (Textual | Numeric, Key) => true,
        _ => a == b,
    }
}

/// Require a computed operand to be of `want`'s family, emitting `code` at `span` otherwise.
/// A `null` literal / unmodelled leaf (`None`) and the Json catch-all are exempt (they
/// never mismatch); a nested sub-expression's family is checked recursively first.
pub(crate) fn require_family(
    e: &ShapeExpr,
    want: Family,
    code: &'static str,
    span: Span,
    mi: usize,
    cx: &Cx,
    sink: &mut Sink,
) {
    let Some(fam) = infer_shape_expr(e, mi, cx, sink) else {
        return;
    };
    if fam != want && fam != Family::Json {
        let at = operand_span(e).unwrap_or(span);
        let msg = if want == Family::Numeric {
            "an arithmetic operand must be numeric (int/float/decimal)"
        } else {
            "a `||` concatenation operand must be text"
        };
        sink.error(code, at, msg.to_string());
    }
}

/// The tightest span for a computed operand: a reached path's last segment, or a
/// sub-expression's own span. A bare literal has no span, so `None` falls back to the
/// enclosing operator's span.
pub(crate) fn operand_span(e: &ShapeExpr) -> Option<Span> {
    match e {
        ShapeExpr::Value(Value::Path(p)) => p.segments.last().map(|s| s.span),
        ShapeExpr::Value(Value::Param(pr)) => Some(pr.name.span),
        ShapeExpr::Arith { span, .. }
        | ShapeExpr::Concat { span, .. }
        | ShapeExpr::Case { span, .. } => Some(*span),
        ShapeExpr::Value(_) => None,
    }
}

/// Infer (and name-check) a single computed leaf operand's family.
pub(crate) fn infer_operand(v: &Value, mi: usize, cx: &Cx, sink: &mut Sink) -> Option<Family> {
    match v {
        Value::Param(pr) => {
            sink.error_note(
                code::CFIELD_NO_PARAMS,
                pr.name.span,
                format!(
                    "`${}` — a computed shape field has no parameters",
                    pr.name.node
                ),
                "computed-field operands are the row's own columns and literals",
            );
            None
        }
        Value::Lit(l) => lit_family(l),
        Value::Path(p) => {
            let term = resolve_path(p, mi, cx, sink)?;
            if reject_opaque(&term, p, "computed field", sink) {
                return None;
            }
            // An enum column reached as an operand rides with text (its wire string).
            Some(terminal_family(&term))
        }
        // A function's return type is unmodelled — treat as unknown (Json), never a mismatch.
        Value::Func(_) => None,
    }
}

/// Resolve a path for its type only, without reporting — name errors are already
/// surfaced by the caller's `resolve_path`, so the type pass stays silent to avoid
/// double-reporting.
pub(crate) fn resolve_quiet(path: &Path, start: usize, cx: &Cx) -> Option<Terminal> {
    let mut throwaway = Sink::default();
    resolve_path(path, start, cx, &mut throwaway)
}

/// The family a column accepts on assignment: a scalar's primitive family, or a
/// forward relation FK's key. An inverse edge owns no column, so it can't be
/// assigned — `None` skips the check (the misuse, if any, is out of scope here).
pub(crate) fn member_family(kind: &MemberKind) -> Option<Family> {
    match kind {
        // An opaque column is never assignable, so it contributes no family.
        MemberKind::Scalar {
            raw_type: Some(_), ..
        } => None,
        MemberKind::Scalar { ty, .. } => Some(prim_family(*ty)),
        MemberKind::Forward { .. } => Some(Family::Key),
        MemberKind::Inverse { .. } => None,
    }
}

pub(crate) fn family_name(f: Family) -> &'static str {
    match f {
        Family::Textual => "text",
        Family::Numeric => "numeric",
        Family::Bool => "bool",
        Family::Json => "json",
        Family::Key => "relation-key",
        Family::Binary => "bytes",
    }
}

impl<'a> Cx<'a> {
    pub fn model(&self, i: usize) -> &RModel {
        &self.models[i]
    }
    pub fn find(&self, name: &str) -> Option<usize> {
        self.index.get(name).copied()
    }
    pub fn scope(&self, name: &str) -> Option<&RScope> {
        self.scope_index.get(name).map(|&i| &self.scopes[i])
    }
    pub fn enum_(&self, name: &str) -> Option<&REnum> {
        self.enum_index.get(name).map(|&i| &self.enums[i])
    }

    /// The enum a dotted path terminates on, when the terminal column is enum-typed
    /// (`where status = …`, `where placed_by.role = …`). `None` when the path is
    /// unresolvable or lands on a non-enum column/relation. Read-only — materializes no
    /// join (the caller's `resolve_path` already reported any name error).
    pub fn terminal_enum(&self, path: &Path, start: usize) -> Option<&REnum> {
        let mut cur = start;
        let n = path.segments.len();
        for (i, seg) in path.segments.iter().enumerate() {
            let mem = self.model(cur).member(&seg.node)?;
            let last = i + 1 == n;
            match &mem.kind {
                MemberKind::Scalar {
                    enum_name: Some(name),
                    ..
                } if last => return self.enum_(name),
                MemberKind::Scalar { .. } => return None,
                MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => {
                    if last {
                        return None;
                    }
                    cur = self.find(target)?;
                }
            }
        }
        None
    }
}

/// A coarse operand-compatibility bucket. Deliberately loose — the goal is to
/// catch nonsense (`~` on an int, `age = "x"`, a relation compared with `<`), not
/// to police every SQL coercion. Timestamp/Date/Uuid/Id ride with text because
/// they are string-writable *and* orderable, which is exactly the set of ops we
/// allow on them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Family {
    Textual,
    Numeric,
    Bool,
    Json,   // holds anything → never mismatches
    Key,    // a relation edge, compared to its key (uuid string or int)
    Binary, // a `bytes` blob — equality-only, never orderable
}
