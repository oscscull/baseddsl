//! A chunked `update` / `delete`: walk the key in order, `size` rows per batch, each batch
//! one transaction. The runtime drives it; this builds each batch's SQL.

use based_ast::{Chunk, DataOp, DataStep};

use super::super::check::DataError;
use super::super::table::TableView;
use super::Lw;
use crate::Dialect;

/// The SQL of one chunked data step. Key values ride as SQL literals rendered by
/// [`ChunkPlan::key_literal`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkPlan {
    /// The table walked, for progress output.
    pub table: String,
    /// The walk key's column names, as the batch select returns them.
    pub key_columns: Vec<String>,
    pub size: u64,
    pub pause_ms: Option<u64>,
    dialect: Dialect,
    from: String,
    keys: Vec<String>,
    filter: Option<String>,
    write_head: String,
}

impl ChunkPlan {
    /// The next batch's keys after `after` (`None` = from the start), in key order.
    pub fn select_sql(&self, after: Option<&[String]>) -> String {
        let keys = self.keys.join(", ");
        let mut conds: Vec<String> = self.filter.iter().cloned().collect();
        conds.extend(after.map(|a| self.compare(">", a)));
        let wheres = if conds.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conds.join(" AND "))
        };
        format!(
            "SELECT {keys} FROM {}{wheres} ORDER BY {keys} LIMIT {}",
            self.from, self.size
        )
    }

    /// The write over the batch `(after, upto]`.
    pub fn apply_sql(&self, after: Option<&[String]>, upto: &[String]) -> String {
        let mut conds: Vec<String> = self.filter.iter().cloned().collect();
        conds.extend(after.map(|a| self.compare(">", a)));
        conds.push(self.compare("<=", upto));
        format!("{} WHERE {}", self.write_head, conds.join(" AND "))
    }

    /// A fetched key value as a SQL literal; `None` for a value that can't be a walk key.
    pub fn key_literal(&self, v: &serde_json::Value) -> Option<String> {
        match v {
            serde_json::Value::String(s) => Some(format!("'{}'", s.replace('\'', "''"))),
            serde_json::Value::Number(n) => Some(n.to_string()),
            serde_json::Value::Bool(b) => Some(self.dialect.bool_lit(*b).to_string()),
            _ => None,
        }
    }

    /// The review text: the batch select and write with `<cursor>` / `<last>` placeholders.
    pub fn review(&self) -> String {
        let cursor: Vec<String> = self.key_columns.iter().map(|_| "<cursor>".into()).collect();
        let last: Vec<String> = self.key_columns.iter().map(|_| "<last>".into()).collect();
        let pause = self.pause_ms.map_or(String::new(), |ms| {
            format!(", pausing {ms}ms between batches")
        });
        format!(
            "-- chunked: walks `{}` by ({}) in batches of {}, one transaction each{pause}\n{};\n{};",
            self.table,
            self.key_columns.join(", "),
            self.size,
            self.select_sql(Some(&cursor)),
            self.apply_sql(Some(&cursor), &last),
        )
    }

    fn compare(&self, op: &str, values: &[String]) -> String {
        if let ([k], [v]) = (self.keys.as_slice(), values) {
            format!("{k} {op} {v}")
        } else {
            format!("({}) {op} ({})", self.keys.join(", "), values.join(", "))
        }
    }
}

impl Lw<'_> {
    pub(crate) fn chunk_plan(
        &self,
        step: &DataStep,
        chunk: &Chunk,
        t: &TableView,
    ) -> Result<ChunkPlan, DataError> {
        let key_columns = match &chunk.by {
            Some(by) => vec![by.node.clone()],
            None => t.pk(),
        };
        if key_columns.is_empty() {
            return Err(Self::fail(chunk.span, "a chunked step needs a walk key"));
        }
        let (filter, write_head) = match &step.op {
            DataOp::Update {
                where_, assigns, ..
            } => (
                where_.as_ref().map(|p| self.predicate(p, t)).transpose()?,
                self.update(None, assigns, t, None)?,
            ),
            DataOp::Delete { where_, .. } => (
                where_.as_ref().map(|p| self.predicate(p, t)).transpose()?,
                self.delete(None, t, None)?,
            ),
            DataOp::Create { .. } => {
                return Err(Self::fail(
                    chunk.span,
                    "`chunk` applies to `update` / `delete`",
                ))
            }
        };
        Ok(ChunkPlan {
            table: t.name().to_string(),
            keys: key_columns.iter().map(|k| self.col_ref(k)).collect(),
            key_columns,
            size: chunk.size,
            pause_ms: chunk.pause_ms,
            dialect: self.dialect,
            from: self.qt(t),
            filter,
            write_head,
        })
    }
}
