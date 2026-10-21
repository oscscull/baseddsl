use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use rusqlite::Connection;
use tokio::sync::oneshot;

use crate::run::DbError;

/// One unit of work on a worker's connection.
pub(super) type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// Opens a fresh connection on the worker thread that will own it.
pub(super) type Opener = Arc<dyn Fn() -> Result<Connection, DbError> + Send + Sync>;

/// One SQLite connection on its own thread. rusqlite blocks, so every job runs there, in
/// the order it was submitted; the thread (and connection) ends when the handle drops.
pub(super) struct Worker {
    jobs: mpsc::Sender<Job>,
    alive: Arc<AtomicBool>,
}

/// Clears the worker's liveness flag however its thread ends (a panicking job included).
struct Exit(Arc<AtomicBool>);

impl Drop for Exit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Worker {
    /// Start a worker thread and open its connection there.
    pub(super) async fn spawn(open: Opener) -> Result<Self, DbError> {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (ready, opened) = oneshot::channel();
        let alive = Arc::new(AtomicBool::new(true));
        let exit = Exit(alive.clone());
        std::thread::Builder::new()
            .name("based-sqlite".into())
            .spawn(move || {
                let _exit = exit;
                let mut conn = match open() {
                    Ok(conn) => conn,
                    Err(e) => {
                        let _ = ready.send(Err(e));
                        return;
                    }
                };
                let _ = ready.send(Ok(()));
                while let Ok(job) = inbox.recv() {
                    job(&mut conn);
                }
            })
            .map_err(|e| DbError::new(format!("starting a sqlite connection thread: {e}")))?;
        opened.await.map_err(|_| stopped())??;
        Ok(Self { jobs, alive })
    }

    /// Whether the thread is still running jobs.
    pub(super) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Queue a job behind every job submitted before it.
    pub(super) fn submit(&self, job: Job) -> Result<(), DbError> {
        self.jobs.send(job).map_err(|_| stopped())
    }

    /// Run `f` on the connection and wait for its result.
    pub(super) async fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> R + Send + 'static,
    ) -> Result<R, DbError> {
        let (tx, rx) = oneshot::channel();
        self.submit(Box::new(move |conn| {
            let _ = tx.send(f(conn));
        }))?;
        rx.await.map_err(|_| stopped())
    }
}

fn stopped() -> DbError {
    DbError::new("the sqlite connection thread stopped")
}
