use super::*;

/// A failure from the database itself — connection lost, timeout, deadlock, a shard
/// down, pool exhausted, or a write that broke an integrity constraint. Distinct from a
/// [`PlanError`] (a boundary/validation failure *before* any SQL). The message is
/// human-facing; the driver fills it from its error, so it names the violated column or
/// constraint.
///
/// The [`kind`](DbError::kind) is the driver's classification of the failure. It sets the
/// wire [`status`](DbError::status) and [`code`](DbError::code), and a
/// [`Deadlock`](DbErrorKind::Deadlock) also tells the mutation path the transaction is
/// safe to auto-retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbError {
    pub message: String,
    pub kind: DbErrorKind,
}

/// The class of a [`DbError`], set by the driver from the server's error code.
/// Operational kinds are a retryable `503`. Integrity violations are caller errors that
/// the same request can never get past: `409` for a clash with stored rows, `422` for a
/// value the column rejects. Only [`Deadlock`](DbErrorKind::Deadlock) changes engine
/// behaviour (bounded transaction retry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DbErrorKind {
    /// An unclassified operational failure (connection lost, a statement timeout). A
    /// `503` the caller may retry, but the engine does **not** auto-retry — re-running a
    /// statement timeout or a lost connection just fails again.
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
    /// The write duplicates a stored unique or primary key value (`409`).
    /// MariaDB 1062, Postgres 23505, SQLite `SQLITE_CONSTRAINT_UNIQUE`/`_PRIMARYKEY`.
    UniqueViolation,
    /// The write references a missing row, or removes a row still referenced (`409`).
    /// MariaDB 1451/1452, Postgres 23503, SQLite `SQLITE_CONSTRAINT_FOREIGNKEY`.
    ForeignKeyViolation,
    /// The write leaves a non-nullable column null (`422`).
    /// MariaDB 1048/1364, Postgres 23502, SQLite `SQLITE_CONSTRAINT_NOTNULL`.
    NotNullViolation,
    /// The write fails a check constraint (`422`).
    /// MariaDB 4025, Postgres 23514, SQLite `SQLITE_CONSTRAINT_CHECK`.
    CheckViolation,
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

    /// A stable machine-readable code for the class of this failure.
    pub fn code(&self) -> &'static str {
        match self.kind {
            DbErrorKind::Other => "database_error",
            DbErrorKind::Deadlock => "deadlock",
            DbErrorKind::PoolExhausted => "pool_exhausted",
            DbErrorKind::UniqueViolation => "unique_violation",
            DbErrorKind::ForeignKeyViolation => "foreign_key_violation",
            DbErrorKind::NotNullViolation => "not_null_violation",
            DbErrorKind::CheckViolation => "check_violation",
        }
    }

    /// The HTTP status the wire answers this failure with: a retryable `503` for an
    /// operational fault, a non-retryable `409`/`422` for an integrity violation.
    pub fn status(&self) -> u16 {
        match self.kind {
            DbErrorKind::Other | DbErrorKind::Deadlock | DbErrorKind::PoolExhausted => 503,
            DbErrorKind::UniqueViolation | DbErrorKind::ForeignKeyViolation => 409,
            DbErrorKind::NotNullViolation | DbErrorKind::CheckViolation => 422,
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
/// callable — the caller can fix it), a [`DbError`] (the database failed or rejected the
/// write — see [`DbError::status`]), a [`NotFound`](RunError::NotFound) (the mutation's
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
