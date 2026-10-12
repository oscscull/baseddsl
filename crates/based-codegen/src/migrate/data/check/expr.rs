//! Type a data-step value expression and check it fits a target column.

use based_ast::{Literal, Path, ShapeExpr, Span, Value};

use super::super::table::{Col, Fam, TableView};
use super::{Ck, Scope};

/// The type of a value expression, as far as a column-fit check needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ty {
    Col(Fam),
    /// A dot path into a `json` column: its text, cast to whatever it is assigned or compared to.
    JsonPath,
    /// A bare name that is not a column — an enum value, if the target is an enum.
    Label(String),
    /// A string; the literal's value when it is one.
    Str(Option<String>),
    Int,
    Dec,
    Bool,
    Null,
    Now,
    Num,
    /// Already reported, or nothing to check against.
    Unknown,
}

impl Ck<'_> {
    pub(crate) fn expr(&mut self, e: &ShapeExpr, t: &TableView, scope: Scope) -> Ty {
        match e {
            ShapeExpr::Value(v) => self.value(v, t, scope),
            ShapeExpr::Arith { lhs, rhs, span, .. } => {
                for side in [lhs, rhs] {
                    let ty = self.expr(side, t, scope);
                    if !numeric_operand(&ty) {
                        self.err(*span, "arithmetic needs numeric operands");
                    }
                }
                Ty::Num
            }
            ShapeExpr::Concat { lhs, rhs, .. } => {
                self.expr(lhs, t, scope);
                self.expr(rhs, t, scope);
                Ty::Str(None)
            }
            ShapeExpr::Case { arms, else_, .. } => {
                for arm in arms {
                    self.predicate(&arm.when, t, scope);
                    self.expr(&arm.then, t, scope);
                }
                self.expr(else_, t, scope)
            }
        }
    }

    pub(crate) fn value(&mut self, v: &Value, t: &TableView, scope: Scope) -> Ty {
        match v {
            Value::Lit(l) => match l {
                Literal::Str(v) => Ty::Str(Some(v.clone())),
                Literal::Int(_) => Ty::Int,
                Literal::Decimal(_) => Ty::Dec,
                Literal::Bool(_) => Ty::Bool,
                Literal::Null => Ty::Null,
            },
            Value::Path(p) => self.path_value(p, t, scope),
            Value::Func(f) if f.name.node == "now" && f.args.is_empty() => Ty::Now,
            Value::Func(f) => {
                self.err(f.name.span, format!("unknown function `{}`", f.name.node));
                Ty::Unknown
            }
            Value::Param(pr) => {
                self.err(
                    pr.name.span,
                    format!("`${}` — a migration step has no parameters", pr.name.node),
                );
                Ty::Unknown
            }
        }
    }

    fn path_value(&mut self, p: &Path, t: &TableView, scope: Scope) -> Ty {
        match p.segments.as_slice() {
            [first, _] if first.node == "incoming" && !scope.incoming => {
                self.err(
                    first.span,
                    "`incoming.<col>` is only available in an `on conflict … update` branch",
                );
                Ty::Unknown
            }
            [first, col] if first.node == "incoming" => self.column_ty(&col.node, col.span, t),
            [only] if t.col(&only.node).is_none() => Ty::Label(only.node.clone()),
            [first, ..] if !scope.columns => {
                self.err(
                    first.span,
                    format!(
                        "`{}` reads a column — a created row's values are literals",
                        first.node
                    ),
                );
                Ty::Unknown
            }
            _ => self.column_path(p, t),
        }
    }

    /// A column, or a dot path into a `json` column.
    pub(crate) fn column_path(&mut self, p: &Path, t: &TableView) -> Ty {
        let first = &p.segments[0];
        match (
            self.column_ty(&first.node, first.span, t),
            p.segments.get(1),
        ) {
            (ty, None) | (ty @ Ty::Unknown, Some(_)) => ty,
            (Ty::Col(Fam::Json), Some(_)) => Ty::JsonPath,
            (_, Some(dot)) => {
                self.err(
                    dot.span,
                    format!(
                        "`{}` is not a json column — a dot path reaches into json only",
                        first.node
                    ),
                );
                Ty::Unknown
            }
        }
    }

    fn column_ty(&mut self, name: &str, span: Span, t: &TableView) -> Ty {
        t.col(name).map_or_else(
            || {
                self.err(span, format!("table `{}` has no column `{name}`", t.name()));
                Ty::Unknown
            },
            |c| Ty::Col(c.fam.clone()),
        )
    }

    /// Report `ty` not fitting `col`.
    pub(crate) fn fits(&mut self, col: &Col, ty: &Ty, span: Span) {
        if let Err(msg) = fits(col, ty) {
            self.err(span, msg);
        }
    }
}

fn numeric_operand(ty: &Ty) -> bool {
    match ty {
        Ty::Col(f) => f.is_numeric(),
        Ty::Int | Ty::Dec | Ty::Num | Ty::JsonPath | Ty::Unknown => true,
        _ => false,
    }
}

/// Can a value of type `ty` be written to `col`?
pub(crate) fn fits(col: &Col, ty: &Ty) -> Result<(), String> {
    match (ty, &col.fam) {
        (Ty::Null, _) if col.nullable => Ok(()),
        (Ty::Null, _) => Err(format!("`{}` is not nullable", col.name)),
        (Ty::Label(l), Fam::StrEnum(vs) | Fam::IntEnum(vs)) if vs.contains(l) => Ok(()),
        (Ty::Label(l), Fam::StrEnum(vs) | Fam::IntEnum(vs)) => Err(format!(
            "`{l}` is not a value of `{}` ({})",
            col.name,
            vs.join(", ")
        )),
        (Ty::Label(l), _) => Err(format!("unknown column `{l}`")),
        (Ty::Str(Some(v)), Fam::StrEnum(vs)) if !vs.contains(v) => Err(format!(
            "\"{v}\" is not a value of `{}` ({})",
            col.name,
            vs.join(", ")
        )),
        (Ty::Str(Some(_)), Fam::StrEnum(_)) => Ok(()),
        (_, Fam::Opaque) => Err(format!(
            "`{}` has an opaque type — use a raw(<dialect>) line",
            col.name
        )),
        (_, f) if accepts(f, ty) => Ok(()),
        _ => Err(format!(
            "cannot write {} to `{}` ({})",
            ty_name(ty),
            col.name,
            col.ty
        )),
    }
}

/// Does family `f` take a value of type `ty`, nulls and labels aside?
fn accepts(f: &Fam, ty: &Ty) -> bool {
    match ty {
        Ty::Unknown => true,
        Ty::JsonPath => !matches!(f, Fam::Bytes | Fam::Opaque),
        Ty::Str(_) => matches!(
            f,
            Fam::Text
                | Fam::Uuid
                | Fam::Ulid
                | Fam::Timestamp
                | Fam::Date
                | Fam::Time
                | Fam::Json
                | Fam::Decimal
                | Fam::StrEnum(_)
        ),
        Ty::Int => matches!(f, Fam::Int | Fam::Float | Fam::Decimal | Fam::IntEnum(_)),
        Ty::Dec => matches!(f, Fam::Float | Fam::Decimal),
        Ty::Num => f.is_numeric(),
        Ty::Bool => *f == Fam::Bool,
        Ty::Now => matches!(f, Fam::Timestamp | Fam::Date),
        Ty::Col(src) => {
            src.name() == f.name()
                || (src.is_numeric() && f.is_numeric())
                || (matches!(src, Fam::StrEnum(_)) && *f == Fam::Text)
                || (*src == Fam::Text && matches!(f, Fam::StrEnum(_)))
        }
        Ty::Null | Ty::Label(_) => false,
    }
}

pub(crate) fn ty_name(ty: &Ty) -> String {
    match ty {
        Ty::Col(f) => format!("a {} column", f.name()),
        Ty::JsonPath => "a json path".into(),
        Ty::Label(l) => format!("`{l}`"),
        Ty::Str(_) => "a string".into(),
        Ty::Int => "an integer".into(),
        Ty::Dec => "a decimal".into(),
        Ty::Bool => "a bool".into(),
        Ty::Null => "null".into(),
        Ty::Now => "now()".into(),
        Ty::Num => "a number".into(),
        Ty::Unknown => "a value".into(),
    }
}
