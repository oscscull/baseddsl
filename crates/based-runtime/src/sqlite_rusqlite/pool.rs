use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::worker::{Opener, Worker};
use crate::run::DbError;

/// How long a checkout waits for a free connection (sqlx's pool default).
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);

/// A bounded pool of connection workers, opened lazily and kept until the pool drops.
/// Cloning shares the pool.
#[derive(Clone)]
pub(super) struct Pool {
    shared: Arc<Shared>,
}

struct Shared {
    idle: Mutex<Vec<Worker>>,
    slots: Arc<Semaphore>,
    open: Opener,
}

impl Pool {
    pub(super) fn new(max_connections: u32, open: Opener) -> Self {
        let max = usize::try_from(max_connections.max(1)).unwrap_or(usize::MAX);
        Self {
            shared: Arc::new(Shared {
                idle: Mutex::new(Vec::new()),
                slots: Arc::new(Semaphore::new(max)),
                open,
            }),
        }
    }

    /// Check out an idle connection, or open one while under the bound.
    pub(super) async fn acquire(&self) -> Result<Pooled, DbError> {
        let permit =
            tokio::time::timeout(ACQUIRE_TIMEOUT, self.shared.slots.clone().acquire_owned())
                .await
                .map_err(|_| DbError::new("pool timed out while waiting for an open connection"))?
                .map_err(|_| DbError::new("the sqlite pool is closed"))?;
        let idle = self
            .shared
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        let worker = match idle {
            Some(worker) => worker,
            None => Worker::spawn(self.shared.open.clone()).await?,
        };
        Ok(Pooled {
            worker: Some(worker),
            shared: self.shared.clone(),
            _permit: permit,
        })
    }
}

/// A checked-out connection; dropping it returns a live worker to the pool.
pub(super) struct Pooled {
    worker: Option<Worker>,
    shared: Arc<Shared>,
    _permit: OwnedSemaphorePermit,
}

impl std::ops::Deref for Pooled {
    type Target = Worker;

    fn deref(&self) -> &Worker {
        self.worker.as_ref().expect("a pooled worker until drop")
    }
}

impl Drop for Pooled {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take().filter(Worker::is_alive) {
            self.shared
                .idle
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(worker);
        }
    }
}
