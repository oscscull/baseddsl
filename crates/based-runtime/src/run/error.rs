use super::*;

/// A failure from the database itself — connection lost, timeout, deadlock, a shard
/// down, pool exhausted. Distinct from a [`PlanError`] (a boundary/validation failure
/// *before* any SQL): a `DbError` is an operational failure the wire maps to a
/// retryable `503`. The message is human-facing; the driver fills it from its error.
///
/// The [`kind`](DbError::kind) is the driver's classification of how to handle the failure:
/// every `DbError` is still a `503`, but a [`Deadlock`](DbErrorKind::Deadlock) additionally
/// tells the mutation path the transaction is safe to auto-retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbError {
    pub message: String,
    pub kind: DbErrorKind,
}

/// The operational class of a [`DbError`], set by the driver from the server's error code.
/// Only [`Deadlock`](DbErrorKind::Deadlock) changes engine behaviour (bounded transaction
/// retry); the rest are informational — every kind is still a wire `503`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DbErrorKind {
    /// An unclassified operational failure (connection lost, a statement timeout, a
    /// constraint violation). A `503` the caller may retry, but the engine does **not**
    /// auto-retry — re-running a statement timeout or a lost connection just fails again.
    #[default]
    Other,
    /// A deadlock or serialization failure: the server *already rolled the transaction
    /// back*, and re-running it usually succeeds (the contending transaction has moved
    /// on). The mutation path retries the whole transaction a bounded number of times.
    /// MariaDB 1213/1205, Postgres 40P01/40001, SQLite `SQLITE_BUSY`/`SQLITE_LOCKED`.
    Deadlock,
    /// No connection became free within the pool's checkout timeout — the pool is
    /// saturated. Fails fast as a `503` (the client/LB backs off), never a hang and never
    /// auto-retried in-process (the pool is still full).
    PoolExhausted,
}

impl DbError {
    /// An unclassified ([`Other`](DbErrorKind::Other)) operational failure.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: DbErrorKind::Other,
        }
    }

    /// A failure of a specific operational [`DbErrorKind`] (the driver classifies its own
    /// error codes into these).
    pub fn of(kind: DbErrorKind, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind,
        }
    }

    /// Is this a deadlock / serialization abort the mutation path may safely retry?
    pub fn is_deadlock(&self) -> bool {
        self.kind == DbErrorKind::Deadlock
    }

    /// A stable machine-readable code for the operational class of this failure.
    pub fn code(&self) -> &'static str {
        match self.kind {
            DbErrorKind::Other => "database_error",
            DbErrorKind::Deadlock => "deadlock",
            DbErrorKind::PoolExhausted => "pool_exhausted",
        }
    }
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DbError {}

/// Why running a request failed: a boundary [`PlanError`] (bad/missing input, unknown
/// callable — the caller can fix it), a [`DbError`] (the database failed — an
/// operational, retryable failure), a [`NotFound`](RunError::NotFound) (the mutation's
/// `where` matched no row), or an idempotency [`Conflict`](RunError::Conflict)
/// (a concurrent attempt with the same key is still in flight). The wire maps each to its
/// HTTP status.
#[derive(Debug, Clone, PartialEq)]
pub enum RunError {
    Plan(PlanError),
    Db(DbError),
    /// A surviving-write mutation (update / soft delete / restore) matched no row: its
    /// `where` — with the scope and soft-delete guards it carries — found nothing to
    /// write, so nothing was written and there is no row to read back. Surfaced as a
    /// `404` rather than a `200 null` the typed client cannot decode. Carries the
    /// callable name.
    NotFound(String),
    /// A mutation retry arrived while a prior attempt with the same idempotency key is
    /// still running. Running a second write would risk the double-insert the key exists to
    /// prevent, so the retry is rejected as a retryable conflict (`409`): the client retries
    /// once the first attempt settles.
    Conflict(String),
    /// The idempotency key was reused for a different request — same key, different
    /// args/`$ctx`. Replaying the first attempt's response would answer the wrong request,
    /// so the reuse is rejected loudly (a non-retryable `422`) rather than run or replayed.
    /// The client must use a fresh key for a genuinely different request.
    KeyReuse(String),
}

impl From<PlanError> for RunError {
    fn from(e: PlanError) -> Self {
        Self::Plan(e)
    }
}
impl From<DbError> for RunError {
    fn from(e: DbError) -> Self {
        Self::Db(e)
    }
}

impl RunError {
    /// A stable machine-readable code for the failure — the boundary/operational class a
    /// consumer branches on. Delegates to the inner [`PlanError::code`]/[`DbError::code`]
    /// where the failure carries its own; the idempotency variants own theirs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Plan(e) => e.code(),
            Self::Db(e) => e.code(),
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "idempotency_conflict",
            Self::KeyReuse(_) => "idempotency_key_reuse",
        }
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plan(e) => write!(f, "{e}"),
            Self::Db(e) => write!(f, "{e}"),
            Self::NotFound(name) => write!(
                f,
                "`{name}` matched no row (no such row, or it is out of scope)"
            ),
            Self::Conflict(key) => {
                write!(
                    f,
                    "a request with idempotency key `{key}` is already in progress"
                )
            }
            Self::KeyReuse(key) => write!(
                f,
                "idempotency key `{key}` was already used for a different request"
            ),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Plan(e) => Some(e),
            Self::Db(e) => Some(e),
            Self::NotFound(_) | Self::Conflict(_) | Self::KeyReuse(_) => None,
        }
    }
}
