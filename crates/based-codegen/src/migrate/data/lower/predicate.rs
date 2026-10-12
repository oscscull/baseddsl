//! Lower a data step's predicate; `child.any(…)` becomes a correlated `EXISTS`.

use based_ast::{Literal, Op, Path, Predicate, Value};

use super::super::check::predicate::span_of;
use super::super::check::DataError;
use super::super::relation::child_link;
use super::super::table::{Col, Fam, TableView};
use super::Lw;
use crate::sql::dml::sql_op;

impl Lw<'_> {
    pub(crate) fn predicate(&self, p: &Predicate, t: &TableView) -> Result<String, DataError> {
        Ok(match p {
            Predicate::And(a, b) => {
                format!("({} AND {})", self.predicate(a, t)?, self.predicate(b, t)?)
            }
            Predicate::Or(a, b) => {
                format!("({} OR {})", self.predicate(a, t)?, self.predicate(b, t)?)
            }
            Predicate::Not(inner) => format!("NOT ({})", self.predicate(inner, t)?),
            Predicate::Cmp { path, op, value } => self.cmp(path, *op, value, t),
            Predicate::InList { path, values } => {
                let target = self.operand_col(path, t, values.first());
                let lhs = self.lhs(path, t, target.as_ref());
                let items: Vec<String> = values
                    .iter()
                    .map(|v| self.value(v, t, target.as_ref()))
                    .collect();
                format!("{lhs} IN ({})", items.join(", "))
            }
            Predicate::Bare(path) => {
                let bool_col = col_of_fam(Fam::Bool, "bool");
                format!(
                    "{} = {}",
                    self.lhs(path, t, Some(&bool_col)),
                    self.dialect.bool_lit(true)
                )
            }
            Predicate::Any { path, pred } => self.exists(path, pred.as_deref(), t)?,
            Predicate::FilterCall { name, .. } => {
                return Err(Self::fail(
                    name.span,
                    "named filters aren't available in a migration",
                ))
            }
            Predicate::Raw(raw) => {
                return Err(Self::fail(
                    raw.span,
                    "a raw predicate isn't available in a data step",
                ))
            }
        })
    }

    fn cmp(&self, path: &Path, op: Op, value: &Value, t: &TableView) -> String {
        let target = self.operand_col(path, t, Some(value));
        let lhs = self.lhs(path, t, target.as_ref());
        if matches!(value, Value::Lit(Literal::Null)) {
            let test = if op == Op::Ne {
                "IS NOT NULL"
            } else {
                "IS NULL"
            };
            return format!("{lhs} {test}");
        }
        let rhs = self.value(value, t, target.as_ref());
        format!("{lhs} {} {rhs}", sql_op(op))
    }

    /// The column a comparison's operands meet as: the left column itself, or — for a json
    /// path — the family its right-hand literal implies.
    fn operand_col(&self, path: &Path, t: &TableView, rhs: Option<&Value>) -> Option<Col> {
        if path.segments.len() == 1 {
            return t.col(&path.segments[0].node).cloned();
        }
        Some(match rhs {
            Some(Value::Lit(Literal::Int(_))) => col_of_fam(Fam::Int, "int"),
            Some(Value::Lit(Literal::Decimal(_))) => col_of_fam(Fam::Float, "float"),
            Some(Value::Lit(Literal::Bool(_))) => col_of_fam(Fam::Bool, "bool"),
            _ => col_of_fam(Fam::Text, "text"),
        })
    }

    /// A comparison's left side: a column, or a json path cast to `target`.
    fn lhs(&self, path: &Path, t: &TableView, target: Option<&Col>) -> String {
        self.value(&Value::Path(path.clone()), t, target)
    }

    /// `EXISTS (SELECT 1 FROM child AS s<n>_child WHERE <link> [AND <pred>])`.
    fn exists(
        &self,
        path: &Path,
        pred: Option<&Predicate>,
        t: &TableView,
    ) -> Result<String, DataError> {
        let span = span_of(path);
        let (child, link) =
            child_link(self.snap, t, &path.segments[0].node).map_err(|m| Self::fail(span, m))?;
        if self.writes_root && child.name() == self.root && self.dialect.is_mysql_family() {
            return Err(Self::fail(
                span,
                format!(
                    "{} can't read `{}` in a subquery of a statement writing it — use a raw({}) line",
                    self.dialect.name(),
                    self.root,
                    self.dialect.name()
                ),
            ));
        }
        self.sub.set(self.sub.get() + 1);
        let alias = format!("s{}_{}", self.sub.get(), child.name());
        let parent = self.qualify.clone().unwrap_or_else(|| t.name().to_string());
        let mut wheres: Vec<String> = link
            .iter()
            .map(|(c, p)| {
                format!(
                    "{} = {}",
                    self.dialect.qcol(&alias, c),
                    self.dialect.qcol(&parent, p)
                )
            })
            .collect();
        let inner = self.nested(alias.clone());
        if let Some(p) = pred {
            wheres.push(inner.predicate(p, &child)?);
        }
        self.sub.set(inner.sub.get());
        Ok(format!(
            "EXISTS (SELECT 1 FROM {} AS {} WHERE {})",
            self.qt(&child),
            self.q(&alias),
            wheres.join(" AND ")
        ))
    }
}

fn col_of_fam(fam: Fam, ty: &str) -> Col {
    Col {
        name: String::new(),
        ty: ty.to_string(),
        fam,
        nullable: true,
        has_default: false,
        generated: false,
        auto: false,
    }
}
