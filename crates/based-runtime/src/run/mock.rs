use super::*;

/// A test double for the whole driver stack: it is a [`Backend`] (checkout clones the
/// shared state), a [`Db`], and — via [`Db::begin`] — a [`Tx`]. It returns pre-loaded
/// row batches in call order, recording every `(sql, params)` it was asked to run
/// (`fetch` and `execute` alike) plus the transaction boundaries it saw, so tests can
/// assert the bound statements. Cheap to clone; every clone shares the same state, so
/// a test keeps a handle for assertions while the engine consumes another.
#[derive(Clone, Default)]
pub struct MockDb {
    state: std::sync::Arc<std::sync::Mutex<MockState>>,
}

impl MockDb {
    /// A mock that replies to each `fetch` with the given batches, in order.
    pub fn new(responses: Vec<Vec<Row>>) -> Self {
        Self {
            state: std::sync::Arc::new(std::sync::Mutex::new(MockState {
                responses: responses.into(),
                ..MockState::default()
            })),
        }
    }

    /// A mock whose every `fetch`/`execute` fails with `message` (the DB-fault path).
    pub fn failing(message: impl Into<String>) -> Self {
        Self {
            state: std::sync::Arc::new(std::sync::Mutex::new(MockState {
                fail: Some(message.into()),
                ..MockState::default()
            })),
        }
    }

    /// A mock whose `fetch` yields `rows`, then fails with `message` — the database
    /// breaking *mid-stream*, after the read has started delivering.
    pub fn failing_mid_stream(rows: Vec<Row>, message: impl Into<String>) -> Self {
        Self {
            state: std::sync::Arc::new(std::sync::Mutex::new(MockState {
                responses: vec![rows].into(),
                fail_mid_stream: Some(message.into()),
                ..MockState::default()
            })),
        }
    }

    /// Report `rows` as every `execute`'s rows-affected (default 0) — the knob the
    /// `-> ok` zero-row-DELETE tests turn.
    pub fn affecting(self, rows: u64) -> Self {
        self.state.lock().unwrap().affected = rows;
        self
    }

    /// Make the next `n` `execute` calls fail with a [`DbErrorKind::Deadlock`] before
    /// succeeding — an injected serialization/deadlock abort the transaction-retry path
    /// (`transaction_retrying`) re-runs the whole transaction on.
    pub fn deadlocking(self, n: usize) -> Self {
        self.state.lock().unwrap().deadlock_writes = n;
        self
    }

    /// Every executed statement so far, in order — `fetch` and `execute` alike.
    pub fn calls(&self) -> Vec<(String, Vec<SqlValue>)> {
        self.state.lock().unwrap().calls.clone()
    }

    /// The transaction boundaries seen, in order (`begin`/`commit`/`rollback` — a
    /// dropped-without-commit [`Tx`] records `rollback`).
    pub fn tx_log(&self) -> Vec<&'static str> {
        self.state.lock().unwrap().tx.clone()
    }

    fn record(&self, sql: &str, params: &[SqlValue]) -> Result<(), DbError> {
        let mut st = self.state.lock().unwrap();
        st.calls.push((sql.to_string(), params.to_vec()));
        match &st.fail {
            Some(m) => Err(DbError::new(m.clone())),
            None => Ok(()),
        }
    }

    fn pop(&self) -> Vec<Row> {
        self.state
            .lock()
            .unwrap()
            .responses
            .pop_front()
            .unwrap_or_default()
    }
}

#[async_trait]
impl DbRead for MockDb {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        let items: Vec<Result<Row, DbError>> = match self.record(sql, params) {
            Ok(()) => {
                let mut items: Vec<Result<Row, DbError>> = self.pop().into_iter().map(Ok).collect();
                if let Some(m) = self.state.lock().unwrap().fail_mid_stream.clone() {
                    items.push(Err(DbError::new(m)));
                }
                items
            }
            Err(e) => vec![Err(e)],
        };
        Box::pin(futures_util::stream::iter(items))
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        self.record(sql, params)?;
        {
            let mut st = self.state.lock().unwrap();
            if st.deadlock_writes > 0 {
                st.deadlock_writes -= 1;
                return Err(DbError::of(
                    DbErrorKind::Deadlock,
                    "mock serialization failure",
                ));
            }
        }
        Ok(self.state.lock().unwrap().affected)
    }
}

#[async_trait]
impl Db for MockDb {
    async fn begin(self: Box<Self>) -> Result<Box<dyn Tx>, DbError> {
        self.state.lock().unwrap().tx.push("begin");
        Ok(Box::new(MockTx {
            db: *self,
            committed: false,
        }))
    }
}

#[async_trait]
impl Backend for MockDb {
    async fn checkout(&self, _shard_key: &str) -> Result<Box<dyn Db>, DbError> {
        Ok(Box::new(self.clone()))
    }

    /// A mock has no live database — trivially ready.
    async fn ping(&self) -> Result<(), DbError> {
        Ok(())
    }
}

/// The mock's open transaction: statements delegate to the shared state; drop without
/// commit records the rollback the typestate guarantees.
struct MockTx {
    db: MockDb,
    committed: bool,
}

#[async_trait]
impl DbRead for MockTx {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        self.db.fetch(sql, params)
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        self.db.execute(sql, params).await
    }
}

#[async_trait]
impl Tx for MockTx {
    async fn commit(mut self: Box<Self>) -> Result<(), DbError> {
        self.committed = true;
        self.db.state.lock().unwrap().tx.push("commit");
        Ok(())
    }
}

impl Drop for MockTx {
    fn drop(&mut self) {
        if !self.committed {
            self.db.state.lock().unwrap().tx.push("rollback");
        }
    }
}

#[derive(Default)]
struct MockState {
    responses: std::collections::VecDeque<Vec<Row>>,
    calls: Vec<(String, Vec<SqlValue>)>,
    tx: Vec<&'static str>,
    fail: Option<String>,
    /// `fetch` yields its batch, then this failure — the stream-broke-late case.
    fail_mid_stream: Option<String>,
    /// What every `execute` reports as rows affected (default 0).
    affected: u64,
    /// The next this-many `execute` calls fail with a [`DbErrorKind::Deadlock`] (then
    /// succeed) — the injected serialization abort the transaction-retry path re-runs on.
    deadlock_writes: usize,
}
