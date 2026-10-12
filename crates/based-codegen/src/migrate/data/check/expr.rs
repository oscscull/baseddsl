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
        let first = &p.segments[0];
        if first.node == "incoming" && p.segments.len() == 2 {
            if !scope.incoming {
                self.err(
                    first.span,
                    "`incoming.<col>` is only available in an `on conflict … update` branch",
                );
                return Ty::Unknown;
            }
            return self.column_ty(&p.segments[1].node, p.segments[1].span, t);
        }
        if p.segments.len() == 1 && t.col(&first.node).is_none() {
            return Ty::Label(first.node.clone());
        }
        if !scope.columns {
            self.err(
                first.span,
                format!(
                    "`{}` reads a column — a created row's values are literals",
                    first.node
                ),
            );
            return Ty::Unknown;
        }
        self.column_path(p, t)
    }

    /// A column, or a dot path into a `json` column.
    pub(crate) fn column_path(&mut self, p: &Path, t: &TableView) -> Ty {
        let first = &p.segments[0];
        let ty = self.column_ty(&first.node, first.span, t);
        if p.segments.len() == 1 || ty == Ty::Unknown {
            return ty;
        }
        if ty == Ty::Col(Fam::Json) {
            Ty::JsonPath
        } else {
            self.err(
                p.segments[1].span,
                format!(
                    "`{}` is not a json column — a dot path reaches into json only",
                    first.node
                ),
            );
            Ty::Unknown
        }
    }

    fn column_ty(&mut self, name: &str, span: Span, t: &TableView) -> Ty {
        if let Some(c) = t.col(name) {
            Ty::Col(c.fam.clone())
        } else {
            self.err(span, format!("table `{}` has no column `{name}`", t.name()));
            Ty::Unknown
        }
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
    let f = &col.fam;
    let ok = match ty {
        Ty::Unknown => true,
        Ty::Null => {
            return if col.nullable {
                Ok(())
            } else {
                Err(format!("`{}` is not nullable", col.name))
            }
        }
        Ty::JsonPath => !matches!(f, Fam::Bytes | Fam::Opaque),
        Ty::Label(l) => {
            return match f {
                Fam::StrEnum(vs) | Fam::IntEnum(vs) if vs.contains(l) => Ok(()),
                Fam::StrEnum(vs) | Fam::IntEnum(vs) => Err(format!(
                    "`{l}` is not a value of `{}` ({})",
                    col.name,
                    vs.join(", ")
                )),
                _ => Err(format!("unknown column `{l}`")),
            }
        }
        Ty::Str(Some(v)) if matches!(f, Fam::StrEnum(_)) => {
            return match f {
                Fam::StrEnum(vs) if !vs.contains(v) => Err(format!(
                    "\"{v}\" is not a value of `{}` ({})",
                    col.name,
                    vs.join(", ")
                )),
                _ => Ok(()),
            }
        }
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
    };
    if *f == Fam::Opaque {
        return Err(format!(
            "`{}` has an opaque type — use a raw(<dialect>) line",
            col.name
        ));
    }
    if ok {
        Ok(())
    } else {
        Err(format!(
            "cannot write {} to `{}` ({})",
            ty_name(ty),
            col.name,
            col.ty
        ))
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
