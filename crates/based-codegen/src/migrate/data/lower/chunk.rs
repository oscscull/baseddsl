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
        let wheres = Some(self.conds(after).join(" AND "))
            .filter(|w| !w.is_empty())
            .map(|w| format!(" WHERE {w}"))
            .unwrap_or_default();
        format!(
            "SELECT {keys} FROM {}{wheres} ORDER BY {keys} LIMIT {}",
            self.from, self.size
        )
    }

    /// The write over the batch `(after, upto]`.
    pub fn apply_sql(&self, after: Option<&[String]>, upto: &[String]) -> String {
        let mut conds = self.conds(after);
        conds.push(self.compare("<=", upto));
        format!("{} WHERE {}", self.write_head, conds.join(" AND "))
    }

    /// The step's own filter, then the lower key bound when resuming past `after`.
    fn conds(&self, after: Option<&[String]>) -> Vec<String> {
        self.filter
            .iter()
            .cloned()
            .chain(after.map(|a| self.compare(">", a)))
            .collect()
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
        let cursor = vec!["<cursor>".to_string(); self.key_columns.len()];
        let last = vec!["<last>".to_string(); self.key_columns.len()];
        let pause = self
            .pause_ms
            .map(|ms| format!(", pausing {ms}ms between batches"))
            .unwrap_or_default();
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
        match (self.keys.as_slice(), values) {
            ([k], [v]) => format!("{k} {op} {v}"),
            (keys, values) => format!("({}) {op} ({})", keys.join(", "), values.join(", ")),
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
        let key_columns = chunk
            .by
            .as_ref()
            .map_or_else(|| t.pk(), |by| vec![by.node.clone()]);
        if key_columns.is_empty() {
            return Err(Self::fail(chunk.span, "a chunked step needs a walk key"));
        }
        let (where_, assigns) = match &step.op {
            DataOp::Update {
                where_, assigns, ..
            } => (where_, Some(assigns)),
            DataOp::Delete { where_, .. } => (where_, None),
            DataOp::Create { .. } => {
                return Err(Self::fail(
                    chunk.span,
                    "`chunk` applies to `update` / `delete`",
                ))
            }
        };
        let filter = where_.as_ref().map(|p| self.predicate(p, t)).transpose()?;
        let write_head =
            assigns.map_or_else(|| self.delete(None, t), |a| self.update(None, a, t))?;
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
