//! Lower a data step's predicate; `child.any(…)` becomes a correlated `EXISTS`.

use based_ast::{Literal, Op, Path, Predicate, Value};

use super::super::check::predicate::span_of;
use super::super::check::DataError;
use super::super::relation::child_link;
use super::super::table::{Col, Fam, TableView};
use super::{col_of, Lw};
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
                let bool_col = col_of(Fam::Bool, "bool");
                format!(
                    "{} = {}",
                    self.lhs(path, t, Some(&bool_col)),
                    self.dialect.bool_lit(true)
                )
            }
            Predicate::Any { path, by, pred } => self.exists(
                path,
                by.as_ref().map(|b| b.node.as_str()),
                pred.as_deref(),
                t,
            )?,
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
        match value {
            Value::Lit(Literal::Null) if op == Op::Ne => format!("{lhs} IS NOT NULL"),
            Value::Lit(Literal::Null) => format!("{lhs} IS NULL"),
            _ => format!(
                "{lhs} {} {}",
                sql_op(op),
                self.value(value, t, target.as_ref())
            ),
        }
    }

    /// The column a comparison's operands meet as: the left column itself, or — for a json
    /// path — the family its right-hand literal implies.
    fn operand_col(&self, path: &Path, t: &TableView, rhs: Option<&Value>) -> Option<Col> {
        Some(match (path.segments.as_slice(), rhs) {
            ([col], _) => return t.col(&col.node).cloned(),
            (_, Some(Value::Lit(Literal::Int(_)))) => col_of(Fam::Int, "int"),
            (_, Some(Value::Lit(Literal::Decimal(_)))) => col_of(Fam::Float, "float"),
            (_, Some(Value::Lit(Literal::Bool(_)))) => col_of(Fam::Bool, "bool"),
            _ => col_of(Fam::Text, "text"),
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
        by: Option<&str>,
        pred: Option<&Predicate>,
        t: &TableView,
    ) -> Result<String, DataError> {
        let span = span_of(path);
        let (child, link) = child_link(self.snap, t, &path.segments[0].node, by)
            .map_err(|m| Self::fail(span, m))?;
        self.sub.set(self.sub.get() + 1);
        let alias = format!("s{}_{}", self.sub.get(), child.name());
        let parent = self.qualify.clone().unwrap_or_else(|| t.name().to_string());
        let inner = self.nested(alias.clone());
        let filter = pred.map(|p| inner.predicate(p, &child)).transpose()?;
        self.sub.set(inner.sub.get());
        let from = format!("{} AS {}", self.qt(&child), self.q(&alias));
        // MySQL/MariaDB can't read the table a statement writes in a subquery; a derived
        // table is materialized first, so its rows are read before the write.
        if self.writes_root && child.name() == self.root && self.dialect.is_mysql_family() {
            return Ok(self.materialized(&link, &parent, &alias, &from, filter));
        }
        let wheres = link
            .iter()
            .map(|(c, p)| {
                format!(
                    "{} = {}",
                    self.dialect.qcol(&alias, c),
                    self.dialect.qcol(&parent, p)
                )
            })
            .chain(filter)
            .collect::<Vec<_>>()
            .join(" AND ");
        Ok(format!("EXISTS (SELECT 1 FROM {from} WHERE {wheres})"))
    }

    /// `(<parent key>) IN (SELECT * FROM (SELECT <child fk> FROM … WHERE …) AS d<n>)` — the
    /// uncorrelated form of `EXISTS` for a self-referential write.
    fn materialized(
        &self,
        link: &[(String, String)],
        parent: &str,
        alias: &str,
        from: &str,
        filter: Option<String>,
    ) -> String {
        let wheres = link
            .iter()
            .map(|(c, _)| format!("{} IS NOT NULL", self.dialect.qcol(alias, c)))
            .chain(filter)
            .collect::<Vec<_>>()
            .join(" AND ");
        let keys = link
            .iter()
            .enumerate()
            .map(|(i, (c, _))| {
                format!(
                    "{} AS {}",
                    self.dialect.qcol(alias, c),
                    self.q(&format!("k{i}"))
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let parents: Vec<String> = link
            .iter()
            .map(|(_, p)| self.dialect.qcol(parent, p))
            .collect();
        let lhs = match parents.as_slice() {
            [one] => one.clone(),
            many => format!("({})", many.join(", ")),
        };
        self.sub.set(self.sub.get() + 1);
        format!(
            "{lhs} IN (SELECT * FROM (SELECT {keys} FROM {from} WHERE {wheres}) AS {})",
            self.q(&format!("d{}", self.sub.get()))
        )
    }
}
