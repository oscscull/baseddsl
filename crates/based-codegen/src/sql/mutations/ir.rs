/// A mutation lowered to its ordered write statements. The whole body already runs
/// under one engine-owned transaction, so a `tx { ... }` block is
/// flattened here — its statements sit inline in execution order. The in-process
/// runtime (write path) consumes this directly, exactly as it consumes
/// [`crate::sql::LoweredQuery`] for reads, so the executed SQL and its bind surface stay in
/// lockstep with `based gen sql`. The text emitter and the runtime both read this one lowering.
#[derive(Debug, Clone)]
pub struct LoweredMutation {
    pub name: String,
    pub stmts: Vec<LoweredWrite>,
    /// The declared-shape re-select: a `SELECT <return shape> FROM <return model> WHERE
    /// <key> [AND <live>] AND <scope>` that reads back the mutation's written row, so the
    /// write response matches the client's decoded output type (the same projection a `get`
    /// of that shape emits). `<key>` is either `id = :result_id` for a create
    /// or the write's own `where` for a surviving update / soft delete / restore. `None`
    /// only when the row does not survive the write — a real DELETE (plain-model `delete` /
    /// `hard delete`) — where the response falls back to `{}`.
    pub ret_select: Option<String>,
    /// The declared-shape read-back for a structured `create … from`: an
    /// IN-keyed re-select over the written rows' keys, projecting the return shape (single
    /// `-> Shape` or bulk `-> Shape[]`). Present in place of [`ret_select`](Self::ret_select)
    /// for a from-create that returns a shape; `None` for a `-> ok` insert / any ordinary
    /// mutation.
    pub bulk_readback: Option<BulkReadback>,
    /// Output field-paths of every `json`-typed leaf in the return shape (same basis as a
    /// query's [`LoweredQuery::json_paths`](crate::sql::LoweredQuery::json_paths)), so a
    /// written `json` value reads back structured — the write round-trips what it stored.
    /// Empty for an `-> ok` mutation with no shape re-select.
    pub json_paths: Vec<String>,
}

/// The read-back for a structured `create … from`. After the chunked INSERT
/// runs, the runtime collects each written row's key (app-known from the payload, or a
/// DB-generated `serial` id learned from the INSERT), re-selects the declared shape keyed on
/// those keys (reusing `project_return`, so nested shapes decode as on a read), and returns
/// the rows in **input order** — one object (single) or an array (bulk).
#[derive(Debug, Clone)]
pub struct BulkReadback {
    /// The shape re-select, with `project_return`'s projection plus hidden `__bkk_<i>` key
    /// columns and a `/*BULK_KEYS*/` sentinel where the key-tuple IN-list is spliced. `:name`
    /// placeholders (scope `$ctx`) are bound by the runtime; the sentinel is replaced with
    /// the per-row key binds.
    pub sql: String,
    /// The physical key columns (in tuple order) the read-back keys on.
    pub key_cols: Vec<String>,
    /// `true` for a bulk `-> Shape[]` (array response), `false` for a single `-> Shape`.
    pub bulk: bool,
    /// Whether the keys are DB-generated (`serial`, learned from the INSERT) or app-known
    /// from the payload.
    pub serial: bool,
}

/// The token in a [`BulkReadback::sql`] the runtime replaces with the key-tuple IN-list.
pub const BULK_KEYS_SENTINEL: &str = "/*BULK_KEYS*/";
/// The alias prefix for the hidden key columns a bulk read-back's projection carries (so the
/// runtime can pair each fetched row with its input-order key). Stripped before the response.
pub const BULK_KEY_ALIAS: &str = "__bkk_";

/// One write statement of a mutation: header-free SQL plus the metadata the runtime
/// needs to bind and respond.
#[derive(Debug, Clone)]
pub struct LoweredWrite {
    /// The `-- create …` / `-- delete (soft): …` comment lines the text emitter
    /// frames the SQL with (a `tx` banner is prepended to the block's first write).
    /// The runtime ignores this.
    pub header: String,
    /// Header-free SQL, ending in `;\n`. `:name` placeholders — including the engine
    /// `:id` / `:id_<step>` for a create — are bound by the runtime.
    pub sql: String,
    /// The model this statement writes. A create's model identifies the row the
    /// mutation's declared return refers to (empty for a raw write, which has none).
    pub model: String,
    /// For a `create` whose `id` the engine generates (no caller-set id), the
    /// bind name that id fills (`id`, or `id_<step>` inside a `tx`); else `None`.
    pub gen_id: Option<String>,
    /// For an upsert `create … on conflict (…)`, the read-back key: each conflict
    /// column's `(physical_col, value_sql)` — the value the create sets for it. The
    /// declared-shape re-select keys on this (not the INSERT's generated id, which a
    /// conflict path discards), so the winning row reads back on both paths. `None` for
    /// a plain create / any other write.
    pub conflict_key: Option<Vec<(String, String)>>,
    /// For a `create` on a **keyless** (`@no_id`) model, the read-back key: a `(unique)`
    /// column the create sets, as `(physical_col, value_sql)` — the declared-shape
    /// re-select keys on it since there is no generated `id`. `None` for a keyed model /
    /// any other write.
    pub read_key: Option<Vec<(String, String)>>,
    /// The physical column whose DB-generated value the run stage recovers for a `serial`
    /// (DB-generated PK) create: the sole `serial` `id`, or a composite `@key`'s `serial`
    /// part. `None` for an app-minted / keyless / natural-key create. Its value is
    /// captured by [`capture`](Self::capture) (a `result_id` bind); `serial_col` marks the
    /// column so the re-select keying knows the id is DB-generated.
    pub serial_col: Option<String>,
    /// Whether this statement is a `create` — the mutation's declared re-select keys on a
    /// create of the return model (`:result_id`), so the assembler needs to know which
    /// writes create.
    pub creates: bool,
    /// For a bound `create` (`create … as name`) and/or a `serial`/composite return
    /// create, the row read-back: after the INSERT runs, the run stage captures the
    /// listed columns' committed values into per-column binds a later step (or the
    /// declared re-select) reads. `None` for a create that neither binds a step nor needs
    /// a DB-generated id read back, and for every non-create write.
    pub capture: Option<Capture>,
    /// A whole-table wipe (`delete all` / `hard delete all`): no `where` narrows it, so
    /// "zero rows affected" is a legitimate success (the table was already empty), not the
    /// absent-row 404 an ordinary delete's zero rows signals. The runtime therefore skips
    /// the ack-row-count check for a wipe.
    pub wipe: bool,
    /// A structured shape-input create (`create Model from $row` / `create Model[] from
    /// $rows`): the row values come from a shape-typed param (the `sql` field holds only a
    /// review template). The runtime reads the param, expands it to a chunked, atomic
    /// multi-row `INSERT`. `None` for every ordinary inline write.
    pub bulk: Option<BulkInsert>,
    /// A filtered **real** DELETE (`hard delete M where …`, or a plain-model `delete M
    /// where …`) — the only write whose zero-rows-affected is an absent-row 404 under an
    /// `-> ok` acknowledgement. A wipe, a soft tombstone, a create, and an update all leave
    /// it `false`, so a surviving-write `-> ok` still acks.
    pub real_delete: bool,
}

/// A structured shape-input `create`. The runtime materializes the actual SQL — the
/// row count is dynamic, so codegen carries the column plan and the runtime finishes the
/// statement. Rows above the driver's bind limit are transparently chunked; the whole
/// insert is one atomic unit within the surrounding transaction.
#[derive(Debug, Clone)]
pub struct BulkInsert {
    /// The target model (the runtime resolves per-column coercion families + the id
    /// strategy from it).
    pub model: String,
    /// The fully-qualified, quoted table name.
    pub table: String,
    /// The mutation param the row(s) come from — a JSON object (single) or array (bulk).
    pub param: String,
    /// `Model[] from` (many rows) vs `Model from` (one row).
    pub bulk: bool,
    /// The INSERT columns in order, each with the per-row value source.
    pub columns: Vec<BulkCol>,
    /// A DB-generated `serial` id column to `RETURNING` after insert, so a single
    /// `create Model from $row -> Shape` can key its declared re-select on it. Empty for
    /// an app-minted / natural key, a bulk (`-> ok`) insert, or a keyless model.
    pub returning: Vec<String>,
    /// A bulk upsert's per-dialect tail (`\nON CONFLICT (…) DO UPDATE SET …` /
    /// `\nON DUPLICATE KEY UPDATE …`), appended to every chunk's INSERT. `:name`
    /// placeholders (a param / `$ctx`) are bound per chunk by the runtime; a stored column,
    /// `incoming.<col>` (→ `excluded`/`VALUES()`), enum/literal, and arithmetic are inline.
    /// `None` for a plain bulk insert.
    pub conflict_tail: Option<String>,
    /// The physical columns keying the declared-shape read-back: the conflict target
    /// (upsert), the surrogate/natural/composite key, or a `(unique)` column. Empty for a
    /// `-> ok` insert (no read-back).
    pub readback_key: Vec<String>,
    /// Whether [`readback_key`](Self::readback_key) is a DB-generated `serial` id, learned
    /// from the INSERT (`RETURNING` on Postgres/SQLite, the `LAST_INSERT_ID()` range on
    /// MySQL/MariaDB), else known from the payload.
    pub readback_serial: bool,
    /// Nested writes: to-one forward relations whose block names non-key payload create the
    /// related row *before* this insert; the created row's key feeds this insert's FK
    /// columns (a [`BulkSource::NestedOneId`] column). Empty for a plain / FK-link create.
    pub nested_one: Vec<NestedCreate>,
    /// Nested writes: to-many inverse relations whose block creates the child collection
    /// *after* this insert; each child's back-FK column ([`BulkSource::ParentId`]) is filled
    /// from this insert's key. Empty for a plain / FK-link / to-one create.
    pub nested_many: Vec<NestedCreate>,
    /// How to recover this insert's primary key per row, so a *parent* insert can source
    /// its FK columns from it (used only when this `BulkInsert` is a nested child). Each
    /// part is either DB-generated (`serial`, learned from the INSERT) or read back from a
    /// value the insert itself wrote (an app-minted id, a natural `@key`).
    pub pk_parts: Vec<PkPart>,
}

/// A nested-write child of a [`BulkInsert`]: the related row(s) created to satisfy a
/// relation block that named non-key payload. A to-one forward child (`nested_one`) is
/// created before its parent; the child's key then fills the parent's FK column.
#[derive(Debug, Clone)]
pub struct NestedCreate {
    /// The relation field naming this nest in the parent's input shape — the runtime reads
    /// each parent row's `row[relation]` as the child payload.
    pub relation: String,
    /// The child insert (recursive — a nested write may nest to any depth).
    pub child: BulkInsert,
}

/// One primary-key part of a [`BulkInsert`], and how the runtime recovers its per-row value
/// to link a parent's FK to it.
#[derive(Debug, Clone)]
pub struct PkPart {
    /// The key part's field name — matches a parent [`BulkSource::NestedOneId::key_field`].
    pub field: String,
    /// The physical PK column.
    pub column: String,
    /// DB-generated (`serial`): learned from the INSERT (`RETURNING` / `LAST_INSERT_ID()`).
    pub serial: bool,
}

/// One INSERT column of a structured shape-input create: the physical column and where its
/// per-row value comes from.
#[derive(Debug, Clone)]
pub struct BulkCol {
    /// Physical column name (unquoted — the runtime quotes per dialect).
    pub column: String,
    pub source: BulkSource,
}

/// The per-row value source for one bulk-insert column. The presence-driven rule:
/// a column named in the shape is written verbatim from the payload; an absent
/// engine-managed column is filled by the engine; `@scope` is *always* engine-injected.
#[derive(Debug, Clone)]
pub enum BulkSource {
    /// `row[json_key]` — a scalar column written verbatim from the payload. `field` names
    /// the model member whose type coerces the value (usually == `json_key`).
    Field {
        json_key: String,
        field: String,
    },
    /// `row[relation][key_field]` — an FK column linking an existing row (a nested
    /// `rel { key }` block in the input shape).
    FkPart {
        relation: String,
        key_field: String,
    },
    /// An FK column whose value is the key of a nested-write child created for `nest` (a
    /// to-one forward block naming non-key payload). Filled at run time from the child
    /// insert's recovered key, aligned per row.
    NestedOneId {
        nest: String,
        key_field: String,
    },
    /// A child's back-FK column (a to-many inverse nested write): its value is the parent's
    /// key part `key_field`, filled at run time after the parent insert, per child row.
    ParentId {
        key_field: String,
    },
    /// An app-minted id per row (`uuid` / `ulid`), absent from the shape.
    MintUuid,
    MintUlid,
    /// A `@scope` column — always the caller's `$ctx.<field>`, identical for every row and
    /// overriding any value the shape names for it.
    Ctx {
        ctx_field: String,
    },
    /// `CURRENT_TIMESTAMP` — an engine `@created`/`@updated` stamp, absent from the shape.
    Now,
}

/// A bound `create`'s row read-back: the committed column values the run stage captures
/// after the INSERT, so a later `tx` step's `$name.field` (and a DB-generated id's
/// `:result_id`) reads the row the database actually wrote.
#[derive(Debug, Clone)]
pub struct Capture {
    /// Each column to capture: the bind a later step reads it under, the physical column
    /// it comes from, and the field name (a member of the created model) whose type the
    /// run stage coerces the value by.
    pub cols: Vec<CaptureCol>,
    /// On Postgres/SQLite/MariaDB the INSERT's own `RETURNING <cols>` returns the row, so
    /// this is `None`. On MySQL (no `INSERT … RETURNING`) it is a follow-up keyed `SELECT`
    /// (unbound `:name` SQL) the run stage executes right after the INSERT to read the row.
    pub followup_select: Option<String>,
}

/// One captured column of a bound create's re-selected row.
#[derive(Debug, Clone)]
pub struct CaptureCol {
    /// The `:name` bind (without the colon) a later step / the re-select reads this value
    /// under — `bref_<binding>__<column>`, or `result_id` for a DB-generated return id.
    pub bind: String,
    /// The physical column read back from the written row.
    pub column: String,
    /// The created model's field whose type coerces the captured value at the later bind.
    pub field: String,
}
