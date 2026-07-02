//! Executing a planned query and shaping the rows into the response envelope.
//!
//! The concrete MariaDB driver is the next slice; execution goes through the
//! abstract [`Db`] trait — the runtime's twin of the generated client's abstract
//! `Transport`. A [`MockDb`] returns canned rows so the whole request → JSON path
//! is testable with no database. Row shaping is where the envelope becomes real:
//! `get` → a JSON object or `null`, `list` → an array, a paginated `list` → the
//! `{ rows, cursor }` page envelope (cursor encoding is a driver concern, deferred —
//! it rides as `null` here).

use crate::load::Compiled;
use crate::plan::{plan_query, Envelope, PlanError, QueryPlan, Request, Stmt};
use crate::value::SqlValue;

/// One returned row: column alias → JSON value (the SELECT aliases each projection
/// to its output name, so a row is already the response object).
pub type Row = serde_json::Map<String, serde_json::Value>;

/// The database seam. The runtime hands it positional SQL + values; it returns
/// rows. Kept minimal — the read path needs only `fetch`. (The write path's
/// `execute`/`transaction` arrive with mutations, next slice.)
pub trait Db {
    fn fetch(&mut self, sql: &str, params: &[SqlValue]) -> Vec<Row>;
}

/// Plan and run a query request, returning the shaped JSON response.
pub fn run_query(
    compiled: &Compiled,
    db: &mut dyn Db,
    req: &Request,
) -> Result<serde_json::Value, PlanError> {
    let plan = plan_query(compiled, req)?;
    Ok(shape(db, &plan))
}

/// Execute a plan's statements and assemble the response per its envelope.
fn shape(db: &mut dyn Db, plan: &QueryPlan) -> serde_json::Value {
    use serde_json::Value as J;
    let rows = run_stmt(db, &plan.main);
    match plan.envelope {
        // `get`: the first row, or JSON null (Option<T>).
        Envelope::One => rows.into_iter().next().map(J::Object).unwrap_or(J::Null),
        // `list`: every row as an array.
        Envelope::Many => J::Array(rows.into_iter().map(J::Object).collect()),
        // paginated `list`: the { rows, cursor } envelope (cursor deferred → null),
        // plus `total` when the query asked for a count.
        Envelope::Page { with_count } => {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "rows".into(),
                J::Array(rows.into_iter().map(J::Object).collect()),
            );
            obj.insert("cursor".into(), J::Null);
            if with_count {
                if let Some(count) = &plan.count {
                    let total = run_stmt(db, count)
                        .into_iter()
                        .next()
                        .and_then(|mut r| r.remove("count"))
                        .unwrap_or(J::Null);
                    obj.insert("total".into(), total);
                }
            }
            J::Object(obj)
        }
    }
}

fn run_stmt(db: &mut dyn Db, stmt: &Stmt) -> Vec<Row> {
    db.fetch(&stmt.sql, &stmt.params)
}

/// A test double: returns pre-loaded row batches in call order, recording every
/// `(sql, params)` it was asked to run so tests can assert the bound statements.
#[derive(Default)]
pub struct MockDb {
    /// Row batches, popped front-to-back per `fetch` call.
    pub responses: std::collections::VecDeque<Vec<Row>>,
    /// Every executed statement, in order (for assertions).
    pub calls: Vec<(String, Vec<SqlValue>)>,
}

impl MockDb {
    /// A mock that replies to each `fetch` with the given batches, in order.
    pub fn new(responses: Vec<Vec<Row>>) -> Self {
        MockDb {
            responses: responses.into(),
            calls: Vec::new(),
        }
    }
}

impl Db for MockDb {
    fn fetch(&mut self, sql: &str, params: &[SqlValue]) -> Vec<Row> {
        self.calls.push((sql.to_string(), params.to_vec()));
        self.responses.pop_front().unwrap_or_default()
    }
}
