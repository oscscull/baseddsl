//! Lower a data-step value expression to SQL.

use std::fmt::Write as _;

use based_ast::{Literal, ShapeExpr, Value};

use super::super::check::DataError;
use super::super::table::{Col, Fam, TableView};
use super::json_path::json_path;
use super::Lw;
use crate::sql::dml::{arith_op_sql, render_func, render_lit};

impl Lw<'_> {
    /// `e` as SQL; `target` is the column it is written to or compared with.
    pub(crate) fn expr(
        &self,
        e: &ShapeExpr,
        t: &TableView,
        target: Option<&Col>,
    ) -> Result<String, DataError> {
        Ok(match e {
            ShapeExpr::Value(v) => self.value(v, t, target),
            ShapeExpr::Arith { lhs, op, rhs, .. } => {
                let numeric = target.filter(|c| c.fam.is_numeric());
                format!(
                    "({} {} {})",
                    self.arith_operand(lhs, t, numeric)?,
                    arith_op_sql(*op),
                    self.arith_operand(rhs, t, numeric)?
                )
            }
            ShapeExpr::Concat { .. } => {
                let mut parts = Vec::new();
                self.concat_parts(e, t, &mut parts)?;
                self.dialect.concat(&parts)
            }
            ShapeExpr::Case { arms, else_, .. } => {
                let mut s = String::from("CASE");
                for arm in arms {
                    let _ = write!(
                        s,
                        " WHEN {} THEN {}",
                        self.predicate(&arm.when, t)?,
                        self.expr(&arm.then, t, target)?
                    );
                }
                let _ = write!(s, " ELSE {} END", self.expr(else_, t, target)?);
                s
            }
        })
    }

    /// An arithmetic operand: a json path reads as a number.
    fn arith_operand(
        &self,
        e: &ShapeExpr,
        t: &TableView,
        numeric: Option<&Col>,
    ) -> Result<String, DataError> {
        let float = Col {
            name: String::new(),
            ty: "float".into(),
            fam: Fam::Float,
            nullable: true,
            has_default: false,
            generated: false,
            auto: false,
        };
        self.expr(e, t, Some(numeric.unwrap_or(&float)))
    }

    fn concat_parts(
        &self,
        e: &ShapeExpr,
        t: &TableView,
        parts: &mut Vec<String>,
    ) -> Result<(), DataError> {
        if let ShapeExpr::Concat { lhs, rhs, .. } = e {
            self.concat_parts(lhs, t, parts)?;
            self.concat_parts(rhs, t, parts)
        } else {
            parts.push(self.expr(e, t, None)?);
            Ok(())
        }
    }

    pub(crate) fn value(&self, v: &Value, t: &TableView, target: Option<&Col>) -> String {
        match v {
            Value::Lit(l) => render_lit(self.dialect, l),
            Value::Func(f) => render_func(f),
            Value::Param(_) => "NULL".to_string(),
            Value::Path(p) => {
                let segs: Vec<&str> = p.segments.iter().map(|s| s.node.as_str()).collect();
                match segs.as_slice() {
                    ["incoming", col] if self.incoming => self.incoming_ref(col),
                    [name] if t.col(name).is_none() => self.label(name, target),
                    [col] => self.col_ref(col),
                    [col, keys @ ..] => {
                        let to = target.map(|c| (&c.fam, c.ty.as_str()));
                        json_path(self.dialect, &self.col_ref(col), keys, to)
                    }
                    [] => "NULL".to_string(),
                }
            }
        }
    }

    /// The proposed row's `col` in an upsert's update branch.
    pub(crate) fn incoming_ref(&self, col: &str) -> String {
        if self.dialect.is_mysql_family() {
            format!("VALUES({})", self.q(col))
        } else {
            format!("excluded.{}", self.q(col))
        }
    }

    /// An enum value named bare: its wire literal.
    fn label(&self, name: &str, target: Option<&Col>) -> String {
        match target.map(|c| &c.fam) {
            Some(Fam::IntEnum(_)) => name.to_string(),
            _ => render_lit(self.dialect, &Literal::Str(name.to_string())),
        }
    }
}
