use super::*;

/// A wire request: which signature, the JSON args, and the request `$ctx`.
#[derive(Debug, Clone)]
pub struct Request {
    pub callable: String,
    pub args: serde_json::Map<String, serde_json::Value>,
    pub ctx: serde_json::Map<String, serde_json::Value>,
    /// An optional idempotency key for a mutation retry: the caller attaches a stable key
    /// so the engine runs the write body at most once per key. Request metadata, supplied
    /// out of band (the `Idempotency-Key` header), never the JSON body — the same
    /// trusted-edge discipline as `$ctx`, and never a schema field. `None` → run every time
    /// (the default). Ignored by queries.
    pub idempotency_key: Option<String>,
}

impl Request {
    /// Convenience: a request whose args/ctx come from JSON objects (a non-object
    /// value is treated as empty — the wire layer will have rejected it already), with
    /// no idempotency key. Use [`Request::with_idempotency_key`] to attach one.
    pub fn new(
        callable: impl Into<String>,
        args: serde_json::Value,
        ctx: serde_json::Value,
    ) -> Self {
        Self {
            callable: callable.into(),
            args: args.as_object().cloned().unwrap_or_default(),
            ctx: ctx.as_object().cloned().unwrap_or_default(),
            idempotency_key: None,
        }
    }

    /// Attach a mutation idempotency key. A blank/whitespace-only key is treated as absent
    /// (a header set to `""` is not a real key), so an empty header never claims a store
    /// slot.
    pub fn with_idempotency_key(mut self, key: Option<String>) -> Self {
        self.idempotency_key = key.filter(|k| !k.trim().is_empty());
        self
    }

    /// A stable hash of this request's payload — its args and `$ctx` — for the idempotency
    /// store. A genuine retry produces the same fingerprint (the stored response replays); a
    /// key reused for a different request produces a different one, which the store rejects.
    ///
    /// Only the payload is fingerprinted: the store already scopes an entry by
    /// `(callable, key)`, so the fingerprint's job is to detect a payload change under a
    /// reused `(callable, key)`. `serde_json::Map` is BTreeMap-backed (sorted keys), so
    /// `to_string` is a canonical serialization and the hash is stable across attempts.
    pub fn fingerprint(&self) -> crate::idempotency::Fingerprint {
        // FNV-1a over the canonical JSON of (args, ctx). A field-count prefix separates the
        // two maps so that moving a field from args to ctx changes the hash (no ambiguous
        // concatenation). FNV is stable across releases (unlike `DefaultHasher`), which a
        // durable multi-instance store relies on.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a 64-bit offset basis.
        for part in [&self.args, &self.ctx] {
            let s = serde_json::Value::Object(part.clone()).to_string();
            for b in s.as_bytes() {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0000_0100_0000_01b3); // FNV prime.
            }
            // A separator byte between the two maps.
            h ^= 0xff;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
}

/// How the executed rows become the response body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Envelope {
    /// `get` — at most one row → `Option<T>` (JSON object or `null`).
    One,
    /// `list` — every row → a JSON array.
    Many,
    /// paginated `list` → the `{ rows, cursor }` envelope; `with_count` adds `total`.
    Page { with_count: bool },
}

/// One executable statement: positional SQL + its bound values, in `?` order.
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub sql: String,
    pub params: Vec<SqlValue>,
}

/// A planned query: the main statement, an optional live-row count (for a
/// `with count` page), and the response envelope.
#[derive(Debug, Clone)]
pub struct QueryPlan {
    pub name: String,
    pub main: Stmt,
    pub count: Option<Stmt>,
    pub envelope: Envelope,
    /// Keyset descriptor for a cursor-paginated `list`, else `None`. The run stage reads
    /// the last row's hidden `__keyset_<i>` columns to mint the next cursor and strips them
    /// from the response.
    pub keyset: Option<KeysetPlan>,
    /// Output field-paths of every `json`-typed leaf in the result (codegen's
    /// [`based_codegen::sql::LoweredQuery::json_paths`]). The run stage parses the value at
    /// each — a `json` column read back as a text string — into structured JSON, so a
    /// `json` field round-trips as the object/array it holds, not a double-encoded string.
    pub json_paths: Vec<String>,
}

/// What the run stage needs to finish a keyset page: how many sort-key columns the
/// cursor carries (`__keyset_0..keys`) and the page size, so a full page yields a
/// "more" cursor and a short page (the last page) yields none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeysetPlan {
    pub keys: usize,
    pub page_size: u64,
}

/// A planned mutation: the write steps in execution order, run under one engine-owned
/// transaction. Each step's SQL is **bound late** (at run time), because a bound
/// `create`'s row read-back captures values (`bref_*`, a DB-generated `:result_id`) that a
/// later step — or the declared re-select — reads: the run stage binds every step from a
/// value environment that accumulates those captures.
#[derive(Debug, Clone)]
pub struct MutationPlan {
    pub name: String,
    pub dialect: based_codegen::Dialect,
    /// The write steps, in execution order — unbound `:name` SQL plus each step's
    /// optional row read-back.
    pub steps: Vec<WriteStep>,
    /// The plan-time value environment: params, `$ctx`, the app-minted engine ids (and,
    /// for an app-minted return create, `result_id`). The run stage clones this and adds
    /// each step's captured values as they are read back.
    pub env0: std::collections::HashMap<String, SqlValue>,
    /// The engine-generated `id` of the create matching the mutation's return model, when
    /// **app-minted** (known at plan time) — the `{ id }` fallback identifier. `None` for a
    /// pure update/delete, a caller-set id, or a DB-generated (`serial`) id (captured at run
    /// time into `env` under `result_id`).
    pub result_id: Option<String>,
    /// The declared-shape re-select (unbound `:name` SQL): reads the written row back in
    /// the mutation's return shape so the write response matches the client's decoded
    /// output type. Bound late from the run environment (its `:result_id` is either
    /// app-minted in `env0` or captured from a DB-generated create). `None` only when the
    /// row does not survive the write (a real DELETE) — the response falls back to `{}`.
    pub ret_select: Option<String>,
    /// For an `-> ok` mutation, the index (into `steps`) of the primary DELETE — the
    /// write on the mutation's primary model. Zero rows affected there means the row
    /// was absent (or out of scope): the transaction rolls back and the mutation is a
    /// 404 `not_found`, mirroring a surviving write's empty re-select. `None` for a
    /// shape-returning mutation.
    pub ack_check: Option<usize>,
    /// The declared-shape read-back for a structured `create … from` (BW1b/BW2): after the
    /// chunked INSERT runs, the runtime re-selects the written rows keyed on their keys and
    /// returns them in input order (one object for `-> Shape`, an array for `-> Shape[]`).
    /// Replaces [`ret_select`](Self::ret_select) for a from-create; `None` otherwise.
    pub bulk_readback: Option<BulkReadbackPlan>,
    /// Output field-paths of every `json`-typed leaf in the return shape (codegen's
    /// [`based_codegen::sql::LoweredMutation::json_paths`]). The run stage parses the value
    /// at each in the write's read-back, so a written `json` value round-trips as the
    /// object/array it holds. Empty for an `-> ok` mutation.
    pub json_paths: Vec<String>,
}

/// The runtime read-back plan for a structured `create … from` (BW1b/BW2).
#[derive(Debug, Clone)]
pub struct BulkReadbackPlan {
    /// The shape re-select with a `/*BULK_KEYS*/` sentinel where the key-tuple IN-list is
    /// spliced, plus hidden `__bkk_<i>` key columns the runtime reads to reorder + strip.
    pub sql: String,
    /// The number of key columns per tuple.
    pub key_count: usize,
    /// `true` for a bulk `-> Shape[]` (array response), `false` for a single `-> Shape`.
    pub bulk: bool,
    /// Whether the keys are DB-generated (`serial`), learned from the INSERT.
    pub serial: bool,
}

/// One write step: unbound `:name` SQL and an optional row read-back.
#[derive(Debug, Clone)]
pub struct WriteStep {
    pub sql: String,
    /// After the INSERT runs, capture these committed column values into the run
    /// environment so a later step / the re-select reads the row the database wrote.
    /// `None` for a write with no read-back (a plain unbound create, or any
    /// update/delete/restore).
    pub capture: Option<StepCapture>,
    /// A structured shape-input create (`create Model[]? from $param`, BW1): its rows are
    /// fully resolved at plan time (id-minted, `$ctx`-scoped, coerced). The run stage
    /// materializes a chunked, atomic multi-row INSERT from it. `None` for every ordinary
    /// write (which uses `sql`).
    pub bulk: Option<BulkStep>,
}

/// A structured shape-input create resolved for execution (BW1): the target table, the
/// INSERT columns (each a bind or an engine literal like `CURRENT_TIMESTAMP`), and every
/// row's already-resolved bind values. The run stage chunks the rows below the driver's
/// bind limit and executes each chunk as one multi-row INSERT inside the surrounding
/// transaction (all-or-nothing).
#[derive(Debug, Clone)]
pub struct BulkStep {
    pub table: String,
    pub columns: Vec<BulkOutCol>,
    /// One entry per input row: the bind values for the non-literal columns, in column
    /// order. Empty (zero rows) is a valid no-op success.
    pub rows: Vec<Vec<SqlValue>>,
    /// Count of columns that bind a value per row (every column whose `literal` is `None`).
    pub binds_per_row: usize,
    /// A bulk upsert's per-dialect tail (`ON CONFLICT … / ON DUPLICATE KEY UPDATE …`, BW2),
    /// appended to every chunk's INSERT. `:name` placeholders (a param / `$ctx`) are bound
    /// from the run environment. `None` for a plain bulk insert.
    pub conflict_tail: Option<String>,
    /// The app-known read-back key of each written row (BW1b/BW2), in input order — the
    /// conflict target / surrogate / natural key values pulled from the payload. Empty when
    /// there is no read-back, or when the key is DB-generated (`serial`, see
    /// [`serial_return`](Self::serial_return)).
    pub key_rows: Vec<Vec<SqlValue>>,
    /// For a `serial` read-back, the physical id column to recover from the INSERT
    /// (`RETURNING` on Postgres/SQLite, the `LAST_INSERT_ID()` range on MySQL/MariaDB) — the
    /// keys aren't known until the DB assigns them. `None` otherwise.
    pub serial_return: Option<String>,
    /// Nested writes: to-one forward children created *before* this insert. Each child's
    /// recovered key fills this step's FK columns (a [`NestedBulk::link_slots`] indexing this
    /// step's rows). Empty for a plain / FK-link create.
    pub nested_one: Vec<NestedBulk>,
    /// Nested writes: to-many inverse children created *after* this insert. Each child's
    /// back-FK column ([`NestedBulk::link_slots`] indexing the *child's* rows) is filled from
    /// this insert's key. Empty for a plain / FK-link / to-one create.
    pub nested_many: Vec<NestedBulk>,
    /// How to recover this insert's primary key per row (for a parent to link its FK to it,
    /// when this step is a nested child). Each part is read from a bound column, or is a
    /// DB-generated `serial` learned from the INSERT (see [`serial_return`](Self::serial_return)).
    pub pk_recover: Vec<PkRecover>,
}

/// A nested-write child of a [`BulkStep`]: the related insert plus how its rows and key link
/// to the parent's rows. In `nested_one` the FK lives on the parent (filled from the child's
/// key); in `nested_many` the FK lives on the child (filled from the parent's key) — the
/// `link_slots` bind index indexes whichever step owns the FK.
#[derive(Debug, Clone)]
pub struct NestedBulk {
    pub step: BulkStep,
    /// One entry per child row: the parent row index it belongs to. A to-one child links to
    /// exactly one parent (some parents may be absent — optional relation); a to-many child
    /// row belongs to the parent whose collection it came from.
    pub parent_of: Vec<usize>,
    /// The FK columns to fill and which key field of the *other* side supplies each value.
    pub link_slots: Vec<LinkSlot>,
}

/// One FK column linked across a nested write: the per-row bind index of the FK-owning step,
/// and the key field (of the other side) whose value fills it.
#[derive(Debug, Clone)]
pub struct LinkSlot {
    pub bind: usize,
    pub key_field: String,
}

/// How to recover one primary-key part of a [`BulkStep`] per row: read a bound column
/// (`Some(bind_index)`), or take the DB-generated `serial` id (`None`).
#[derive(Debug, Clone)]
pub struct PkRecover {
    pub field: String,
    pub bind: Option<usize>,
}

/// One INSERT column of a [`BulkStep`]: its quoted name and, for an engine-filled column,
/// the SQL literal used verbatim in every row's tuple (`CURRENT_TIMESTAMP`). A `None`
/// literal is a per-row bound value pulled from [`BulkStep::rows`].
#[derive(Debug, Clone)]
pub struct BulkOutCol {
    pub quoted: String,
    pub literal: Option<String>,
}

/// A bound `create`'s row read-back plan: the columns to capture and how to obtain the row.
#[derive(Debug, Clone)]
pub struct StepCapture {
    pub cols: Vec<CaptureBind>,
    /// On MySQL (no `INSERT … RETURNING`), the follow-up keyed `SELECT` (unbound) run right
    /// after the INSERT to read the row. `None` on Postgres/SQLite/MariaDB — the INSERT's
    /// own `RETURNING` returns the row, so the run stage fetches the INSERT itself.
    pub followup_select: Option<String>,
}

/// One captured column: the bind a later step reads it under, the physical column read
/// from the written row, and the coercion family for the value at that later bind.
#[derive(Debug, Clone)]
pub struct CaptureBind {
    pub bind: String,
    pub column: String,
    pub family: Family,
}

/// Why a request could not be planned — all boundary failures, before any SQL.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanError {
    /// No query with this name.
    UnknownQuery(String),
    /// No mutation with this name.
    UnknownMutation(String),
    /// A required arg was absent (and had no default).
    MissingArg(String),
    /// An arg was present but the wrong JSON type for its param.
    BadArg {
        name: String,
        expected: Family,
        got: String,
    },
    /// A required `$ctx.<field>` was absent from the request context.
    MissingCtx(String),
    /// A `$ctx.<field>` was present but the wrong JSON type.
    BadCtx {
        field: String,
        expected: Family,
        got: String,
    },
    /// The SQL referenced a `:name` the runtime could not resolve — an internal
    /// invariant break (codegen emitted a placeholder the planner did not bind).
    UnboundPlaceholder(String),
    /// A keyset `cursor` arg was present but malformed/tampered/of the wrong arity. The
    /// caller sent a bad cursor — a boundary error.
    BadCursor(String),
}

impl PlanError {
    /// The stable, machine-readable code for this failure — the single source of truth for
    /// the wire `error.code` (`serve`) and any library consumer that branches on the class
    /// of failure rather than the message text. Stable across releases.
    pub fn code(&self) -> &'static str {
        use PlanError::{
            BadArg, BadCtx, BadCursor, MissingArg, MissingCtx, UnboundPlaceholder, UnknownMutation,
            UnknownQuery,
        };
        match self {
            UnknownQuery(_) => "unknown_query",
            UnknownMutation(_) => "unknown_mutation",
            MissingArg(_) => "missing_arg",
            BadArg { .. } => "bad_arg",
            MissingCtx(_) => "missing_ctx",
            BadCtx { .. } => "bad_ctx",
            UnboundPlaceholder(_) => "internal",
            BadCursor(_) => "bad_cursor",
        }
    }
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use PlanError::{
            BadArg, BadCtx, BadCursor, MissingArg, MissingCtx, UnboundPlaceholder, UnknownMutation,
            UnknownQuery,
        };
        match self {
            UnknownQuery(n) => write!(f, "no query `{n}`"),
            UnknownMutation(n) => write!(f, "no mutation `{n}`"),
            MissingArg(n) => write!(f, "missing argument `{n}`"),
            BadArg {
                name,
                expected,
                got,
            } => write!(
                f,
                "argument `{name}`: expected {}, got {got}",
                expected.label()
            ),
            MissingCtx(field) => write!(f, "missing request context `$ctx.{field}`"),
            BadCtx {
                field,
                expected,
                got,
            } => write!(
                f,
                "context `$ctx.{field}`: expected {}, got {got}",
                expected.label()
            ),
            UnboundPlaceholder(n) => {
                write!(f, "unbound placeholder `:{n}` (codegen/planner mismatch)")
            }
            BadCursor(msg) => write!(f, "invalid cursor: {msg}"),
        }
    }
}

impl std::error::Error for PlanError {}
