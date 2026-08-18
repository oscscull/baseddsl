//! Resolved schema IR + diagnostic codes.
//!
//! `check()` turns the flat `[Decl]` into this cross-linked form: models with
//! their implicit columns, resolved relations, and engine-managed roles, plus
//! resolved summaries of every shape / query / mutation / filter. It is the seed
//! codegen reads (alongside the AST) — the resolution facts that are *not* in the
//! AST (inferred verb, relation targets, table names, soft-delete mode) live here.

use based_ast::{DefaultVal, Predicate, Primitive, RawSpec, SortTerm, Span, Verb};
use based_diagnostics::Diagnostic;
use std::collections::HashMap;

// ---------- diagnostic codes ----------------------------------------------
// E01xx = sema errors, W01xx = sema lints. Parser owns E0001/E0002, manifest
// E001x. Codes are stable so lints can be referenced in the spec and ratcheted.
pub mod code {
    // resolution / uniqueness
    pub const DUP_MODEL: &str = "E0100";
    pub const DUP_SHAPE: &str = "E0101";
    pub const DUP_CALLABLE: &str = "E0102"; // query/mutation share the wire namespace
    pub const DUP_FILTER: &str = "E0103";
    pub const DUP_FIELD: &str = "E0104";
    pub const DUP_SCOPE: &str = "E0105"; // duplicate `scope` decl name
    pub const DUP_ENUM: &str = "E0106"; // enum name collides with a model/shape/scope/enum
    pub const UNKNOWN_MODEL: &str = "E0110";
    pub const UNKNOWN_FIELD: &str = "E0111";
    pub const TRAVERSE_SCALAR: &str = "E0112"; // dotted past a scalar column
    pub const UNKNOWN_PARAM: &str = "E0113";
    pub const UNKNOWN_FILTER: &str = "E0114";
    pub const FILTER_ARITY: &str = "E0115";
    pub const UNKNOWN_FUNC: &str = "E0116";
    // models
    pub const SOFT_DELETE_TYPE: &str = "E0120"; // field not in the covered subset
    pub const DECO_TARGET: &str = "E0121"; // @created/@updated/@soft_delete target
    pub const INDEX_COLUMN: &str = "E0122";
    pub const INVERSE_REF: &str = "E0123"; // (Model.field) does not name a forward edge
    pub const INVERSE_INFER: &str = "E0124"; // to-many with no inferable / ambiguous inverse
    pub const JOIN_TABLE: &str = "E0125"; // custom `on:` join names a table not in scope
    pub const JOIN_FORM: &str = "E0126"; // custom `on:` join malformed (not `<table>.<col>`, or not a to-one relation)
                                         // shapes
    pub const SHAPE_BARE_RELATION: &str = "E0130"; // bare relation must nest or `=`
    pub const SHAPE_NEST_SCALAR: &str = "E0131"; // nested a non-relation
    pub const SHAPE_REF_UNKNOWN: &str = "E0132"; // `field -> Name` names no shape
    pub const SHAPE_REF_MODEL: &str = "E0133"; // referenced shape's model ≠ relation target
    pub const SHAPE_REF_CYCLE: &str = "E0134"; // a shape transitively nests itself by reference
                                               // queries / mutations
    pub const UNKNOWN_RETURN: &str = "E0140";
    pub const RETURN_MODEL_MISMATCH: &str = "E0141";
    pub const FULL_NEEDS_MODEL: &str = "E0142";
    pub const BINDING_EDGE: &str = "E0143"; // `-> edge` not a relation
    pub const GET_NOT_UNIQUE: &str = "E0144"; // get must key a unique field
    pub const RESTORE_NOT_SOFT: &str = "E0145";
    // create omits a required (non-optional, non-defaulted) column.
    pub const CREATE_MISSING: &str = "E0146";
    // operand typing
    pub const OP_TYPE: &str = "E0150"; // operator not applicable to the operand type
    pub const CMP_TYPE: &str = "E0151"; // incompatible operand types in a comparison
    pub const PARAM_TYPE: &str = "E0152"; // param annotation disagrees with its mapped column
    pub const ASSIGN_TYPE: &str = "E0153"; // create/update assigns a value of the wrong type to a column
    pub const ENUM_VARIANT: &str = "E0154"; // a where/create/update value is not a variant of the column's enum
    pub const ENUM_DEFAULT: &str = "E0155"; // a field's `default <variant>` is not a member of its enum (or the field isn't an enum)
    pub const ENUM_MIXED: &str = "E0156"; // an enum mixes an int-valued variant with a bare/string one (kind is ambiguous)
    pub const ENUM_DUP_VALUE: &str = "E0157"; // two variants of an enum share a wire value (string or int)
    pub const ENUM_ORDERED_OP: &str = "E0158"; // an ordered comparison (< > <= >=) on a string enum column
    pub const DECIMAL_INVALID: &str = "E0159"; // a `decimal(p,s)` has a bad precision/scale, or a decimal column's default isn't a decimal literal

    // $ctx typing : the caller-supplied request context. Its type is not
    // declared — it is inferred per callable from use and checked for coherence.
    pub const CTX_BAD_PATH: &str = "E0160"; // $ctx used without exactly one field segment
    pub const CTX_CONFLICT: &str = "E0161"; // $ctx.<field> used at incompatible types across uses

    // tx step bindings: `create … as name` binds a step's produced row, referenced by a
    // later step as `$name.field` (`$` unifies params + step bindings).
    pub const BINDING_SHADOW: &str = "E0280"; // a step binding shadows a param, or duplicates another binding
    pub const BINDING_UNBOUND: &str = "E0281"; // `$name` names no param or *prior* step binding (unbound / forward reference)

    // Named scope: a `scope` decl referenced by
    // `@scope Name` on a model + `scoped Name` on every callable that touches it.
    pub const SCOPE_FORM: &str = "E0180"; // a `scope` decl's predicate isn't a conjunction of `col = $ctx.field`
    pub const SCOPE_ASSIGN: &str = "E0181"; // a `create` assigns a scope column (engine-managed)
    pub const SCOPE_MISSING_ACK: &str = "E0182"; // scoped callable writes neither `scoped …` nor `unscoped(…)`
    pub const SCOPE_UNKNOWN: &str = "E0183"; // `@scope Name` / `scoped Name` names no `scope` decl
    pub const SCOPE_MODEL_COLUMN: &str = "E0184"; // `@scope` model lacks the scope's column / wrong type
    pub const SCOPE_ACK_MISMATCH: &str = "E0185"; // `scoped …` set ⊉ any alternative of a touched scoped model
    pub const SCOPE_CREATE_UNSAT: &str = "E0186"; // a `create` can satisfy no alternative

    // `@was("old")` rename directive: declares a field's/model's
    // previous physical name so the diff emits a clean rename instead of drop+add.
    pub const WAS_NOOP: &str = "E0190"; // `@was` names the field's/model's own current name (a no-op)
    pub const WAS_LIVE: &str = "E0191"; // `@was("old")` but `old` is still a live column/table (can't be the rename source)
                                        // lints
    pub const NONDET_SORT: &str = "W0100";
    pub const UNKNOWN_DECORATOR: &str = "W0101";
    pub const RAW_SOFT_DELETE_GAP: &str = "W0102";
    // index lints
    pub const USELESS_INDEX: &str = "W0104"; // declared index no query uses (pure write-tax)
    pub const STALE_UNINDEXED: &str = "W0105"; // unindexed(...) on a query that is indexed
    pub const STALE_UNSCOPED: &str = "W0106"; // unscoped(...) on a callable whose model has no @scope
    pub const WAS_SPENT: &str = "W0107"; // `@was` rename already captured — remove it (offline, LSP)
    pub const MIGRATE_DRIFT: &str = "W0108"; // schema is ahead of migrations — run `based migrate gen` (offline, LSP)
    pub const RAW_MIGRATION_MODELED: &str = "W0109"; // a raw migration step names a snapshot-modeled table (the snapshot is blind to it)

    // streaming: the `-> stream Shape` return form (E02xx)
    pub const STREAM_GET: &str = "E0200"; // stream body verb must be `list` (`get` is a cardinality mismatch)
    pub const STREAM_PAGE: &str = "E0201"; // `page` forbidden on a stream query (bounded chunk vs unbounded pass)
    pub const STREAM_MUTATION: &str = "E0202"; // a mutation return never streams

    // whole-query raw bodies (raw.md's third level, E021x)
    pub const RAW_QUERY_PARAM: &str = "E0210"; // a raw-bodied query's param must be typed and unbound (nothing to infer from / bind against)
    pub const RAW_QUERY_SCOPED: &str = "E0211"; // `scoped` on a raw-bodied query — the engine can't inject scope into raw SQL
    pub const RAW_QUERY_STREAM: &str = "E0212"; // `-> stream` with a raw body is unsupported
    pub const RAW_QUERY_NEST: &str = "E0213"; // a raw-bodied query's return shape must be flat (no nested sub-objects)
    pub const RAW_QUERY_CTX: &str = "E0214"; // `${ctx.…}` in a raw query body has no type source — pass a typed param

    // destructive mutations: the `-> ok` acknowledgement (E022x)
    pub const SHAPE_ON_DELETE: &str = "E0220"; // a real-DELETE mutation declares a shape — no surviving row to read back
    pub const ACK_SURVIVING: &str = "E0221"; // `-> ok` on a mutation with a surviving write (or no real DELETE at all)
    pub const ACK_QUERY: &str = "E0222"; // `-> ok` on a query — a query returns data

    // atomic update expressions (E023x)
    pub const ARITH_CREATE: &str = "E0230"; // an arithmetic assign expression in a `create` — no existing row to reference (update-only)
    pub const ARITH_OPERAND: &str = "E0231"; // a non-numeric operand in an arithmetic assign expression

    // aggregations + group by + having (E024x)
    pub const AGG_CALL: &str = "E0240"; // an aggregate call names an unknown function, or has the wrong argument arity (`count()` takes none; `sum`/`avg`/`min`/`max` take one)
    pub const AGG_OPERAND: &str = "E0241"; // an aggregate over an ineligible column (`sum`/`avg` need numeric; `min`/`max` need a comparable column; never an enum/relation)
    pub const AGG_GROUP_BY: &str = "E0242"; // a non-aggregate projected column is not a `group by` column, or an `order`/`having` names something not projected
    pub const AGG_CONTEXT: &str = "E0243"; // `group by` / `having` on a query whose return shape carries no aggregate
    pub const AGG_PAGE: &str = "E0244"; // `page` on an aggregate query (grouped keyset paging is unsupported)
    pub const AGG_COMPOSE: &str = "E0245"; // an aggregate shape nests/references a relation, is nested/referenced, or is a mutation return

    // explicit-in-source structure (E026x): the two engine-created facts that carry
    // independent write/disk cost are written in source, not silently derived.
    pub const UNINDEXED_JOIN: &str = "E0260"; // a traversed join key (or a query filter) is not covered by an `@index` (opt out with `unindexed(…)`)
    pub const NO_ID: &str = "E0261"; // a model declares no `id` field
                                     // `@no_id("reason")`: the opt-out for a genuinely keyless legacy table, and the
                                     // operations a keyless model forfeits.
    pub const NO_ID_REASON: &str = "E0262"; // `@no_id` without a non-empty reason string
    pub const KEYLESS_KEYSET: &str = "E0263"; // a keyset `page` on a `@no_id` model whose sort has no unique tiebreaker (a non-deterministic cursor)
    pub const KEYLESS_CREATE: &str = "E0264"; // a create on a `@no_id` model with a declared read-back but no `(unique)` column set to read it back by
    pub const REL_TO_KEYLESS: &str = "E0265"; // a forward relation targets a `@no_id` model (its `id` doesn't exist to reference)

    // primary-key generation strategy (`id: uuid | ulid | serial`, D110)
    pub const PK_BARE_INT: &str = "E0266"; // a bare `int` (or other numeric) as the `id` PK — a DB-generated integer key must be spelled `serial` (its generation is consequential and must be visible)
    pub const PK_STRATEGY_MISPLACED: &str = "E0267"; // `serial`/`ulid` on a non-primary-key column — they are PK generation strategies, valid only as the `id` type
    pub const PK_SERIAL_BACKREF: &str = "E0268"; // a `tx` step reads `$name.id` of a `serial` (DB-generated) create — the id is unknown until the row is written, so it can't bind a sibling step

    // nominated primary key `@key(f1, f2, …)` (D111): declared column(s) *are* the primary
    // key — no surrogate `id` is synthesized. One field is a natural single-column key; two
    // or more form a composite key over those columns in list order.
    pub const KEY_UNKNOWN_FIELD: &str = "E0275"; // `@key(x)` names a field the model does not declare
    pub const KEY_UNSUITABLE: &str = "E0276"; // the nominated field can't be a primary key (optional, `[]`, or a relation/opaque — a PK must be a required scalar column)
    pub const KEY_WITH_NO_ID: &str = "E0277"; // `@key` and `@no_id` on one model — a model either nominates a key or declares itself keyless, never both
    pub const KEY_EMPTY: &str = "E0278"; // `@key()` names no field — a primary key must nominate at least one column
    pub const KEY_DUPLICATE: &str = "E0279"; // `@key(f, …, f)` names the same field twice — each key column appears once

    // opaque `raw(…)` column types + exotic indexes (E027x): the escape hatch for a DB
    // type or index form the engine does not model. The literal string is stored and
    // diffed verbatim; nothing here interprets it.
    pub const RAW_TYPE_DIALECT: &str = "E0270"; // a per-dialect `raw({…})` map omits the compile target
    pub const OPAQUE_OPERAND: &str = "E0271"; // filter/sort/group/aggregate on an opaque column (use a `raw` leaf)
    pub const INDEX_METHOD: &str = "E0272"; // `using <method>` names an unknown method, or one this target lacks
    pub const OPAQUE_ASSIGN: &str = "E0273"; // a create/update assigns an opaque column, or a create can't supply a required one
    pub const RAW_EMPTY: &str = "E0274"; // an empty `raw(…)` body

    // opt-in FK referential actions (E029x): `@fk` opts a forward relation into an FK
    // constraint (with optional `on_delete`/`on_update` actions); `@no_fk` opts out (edge
    // or whole model). Presence is resolved against the toml `foreign_keys` convention.
    pub const FK_TARGET: &str = "E0290"; // `@fk`/`@no_fk` on something that is not a forward to-one relation (an inverse/`[]` edge or a scalar)
    pub const FK_CUSTOM_JOIN: &str = "E0291"; // `@fk`/`@no_fk` on a custom-join (`on:`) relation — it owns no conventional FK column
    pub const FK_CONFLICT: &str = "E0292"; // `@fk` and `@no_fk` on the same relation
    pub const FK_SET_NULL_REQUIRED: &str = "E0293"; // `on_delete: set_null` on a required (non-nullable) relation
    pub const FK_ACTION: &str = "E0294"; // an unknown referential action (not cascade/restrict/set_null/no_action)
    pub const FK_DIVERGE_REASON: &str = "E0295"; // a decorator flips FK presence against the `foreign_keys` convention without a reason
    pub const FK_REDUNDANT: &str = "W0110"; // a decorator restates the `foreign_keys` convention (no effect) — remove it

    // --- upsert (`create … on conflict update`) ---
    pub const UPSERT_TARGET: &str = "E0250"; // the conflict target is not a declared unique key (unique column / `@index (…) unique` / pk)
    pub const UPSERT_TARGET_SET: &str = "E0251"; // the `on conflict update` branch assigns a conflict-target column (moving the key breaks the read-back)
    pub const UPSERT_TARGET_UNSET: &str = "E0252"; // a conflict-target column is neither set by the create nor scope-managed (no value to conflict / read back on)
    pub const UPSERT_SOFT_DELETE: &str = "E0253"; // `on conflict` on a @soft_delete model — a tombstoned row would be silently updated instead of inserted
    pub const UPSERT_SCOPE: &str = "E0254"; // a scoped model's conflict target omits a scope column — a conflict could match/modify another scope's row

    // schema / database namespace qualifier `@schema("name")` (D113): places a model's
    // table in a named SQL schema (Postgres) / database (MySQL/MariaDB) instead of the
    // connection default, so a table outside the default namespace is representable.
    pub const SCHEMA_INVALID: &str = "E0296"; // `@schema("")` / a name that is not a single bare identifier (empty, dotted, or whitespace)
    pub const TABLE_QUALIFIED: &str = "E0297"; // a `.` in `@table("a.b")` — a namespace prefix belongs in `@schema`, not the table name

    // far-side flattening projection (E030x): `out = path { body }` skips a junction to
    // the distinct far side of a many-to-many.
    pub const FLATTEN_NOT_TOMANY: &str = "E0300"; // the flatten path's first segment is not a to-many inverse edge (nothing to flatten through)
    pub const FLATTEN_SEGMENT: &str = "E0301"; // a segment after the first does not resolve as a forward edge to the next model
    pub const FLATTEN_KEYLESS: &str = "E0302"; // the far-side model is `@no_id` (keyless) — a distinct set of far rows has no key to dedup on

    // `list distinct <M>` (E031x): dedup the projected rows (`SELECT DISTINCT`).
    pub const DISTINCT_KEYSET: &str = "E0310"; // `distinct` with a keyset `page` — the hidden id/cursor columns defeat the dedup
    pub const DISTINCT_AGGREGATE: &str = "E0311"; // `distinct` on an aggregate query — a `group by` already returns distinct groups
    pub const DISTINCT_ORDER_UNPROJECTED: &str = "E0312"; // an `order` column under `distinct` is not projected (invalid `SELECT DISTINCT … ORDER BY` on Postgres)
    pub const DISTINCT_NOOP: &str = "W0111"; // `distinct` on a query that projects the primary key — every row is already unique
}

/// The closed set of aggregate functions usable in a shape value (shapes.md). `count`
/// is arg-less; the rest take one column. Sema restricts the grammar's open `agg_func`
/// to this set (`E0240`).
pub const KNOWN_AGGS: &[&str] = &["count", "sum", "avg", "min", "max"];

/// The known model-level decorators. Anything else is a `W0101` (still a modifier,
/// just not one the engine understands).
pub const KNOWN_DECORATORS: &[&str] = &[
    "soft_delete",
    "sort",
    "scope",
    "created",
    "updated",
    "table",
    "schema",
    "was",
    "no_id",
    "no_fk",
    "key",
];

/// The project-wide FK-constraint convention, from the manifest `[schema] foreign_keys`
/// key. `None` (default): a relation gets an FK only if it writes `@fk`. `All`: every
/// forward relation gets a bare FK unless it (or its model) writes `@no_fk`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ForeignKeys {
    #[default]
    None,
    All,
}

impl ForeignKeys {
    /// Parse the manifest value; anything but `"all"` is the safe `None` default.
    pub fn parse(s: &str) -> Self {
        match s {
            "all" => Self::All,
            _ => Self::None,
        }
    }
}

/// A model's primary-key **generation strategy** — how the PK value comes to exist. Set
/// by the `id` column's declared type (`id: uuid | ulid | serial`; `id: Id` resolves to
/// the manifest `[schema] id` default). Drives minting (app-side vs DB-side), DDL, and
/// the wire id repr. A `@no_id` (keyless) model has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PkStrategy {
    /// `uuid` — an app-minted random v4 string (the project default). Known before the
    /// INSERT, so it binds like any other value.
    #[default]
    Uuid,
    /// `ulid` — an app-minted lexicographically-sortable string. App-side, like uuid.
    Ulid,
    /// `serial` — a DB-generated sequential integer. Unknown until the row is written, so
    /// the INSERT omits it and the engine reads the assigned id back.
    Serial,
}

impl PkStrategy {
    /// The strategy a primary-key column's declared primitive implies. `Id` folds to the
    /// project default (`uuid`) — a manifest pass rewrites a non-uuid default onto the
    /// member first ([`resolve_pk_default`]), so by codegen time the primitive is concrete.
    pub fn of(ty: Primitive) -> Self {
        match ty {
            Primitive::Serial => Self::Serial,
            Primitive::Ulid => Self::Ulid,
            _ => Self::Uuid,
        }
    }
    /// Parse the manifest `[schema] id` default; anything but `ulid`/`serial` is `uuid`.
    pub fn parse(s: &str) -> Self {
        match s {
            "ulid" => Self::Ulid,
            "serial" => Self::Serial,
            _ => Self::Uuid,
        }
    }
    /// Is the id minted inside the database (unknown until the INSERT runs)?
    pub fn is_db_generated(self) -> bool {
        matches!(self, Self::Serial)
    }
}

/// A standard-SQL referential action on an FK (`on_delete`/`on_update`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FkAction {
    Cascade,
    Restrict,
    SetNull,
    NoAction,
}

impl FkAction {
    /// Map the source keyword to an action, or `None` for an unknown spelling (`E0294`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "cascade" => Some(Self::Cascade),
            "restrict" => Some(Self::Restrict),
            "set_null" => Some(Self::SetNull),
            "no_action" => Some(Self::NoAction),
            _ => None,
        }
    }
    /// The SQL clause spelling (`ON DELETE <this>`).
    pub fn sql(self) -> &'static str {
        match self {
            Self::Cascade => "CASCADE",
            Self::Restrict => "RESTRICT",
            Self::SetNull => "SET NULL",
            Self::NoAction => "NO ACTION",
        }
    }
    /// The neutral snapshot spelling (matches the source keyword).
    pub fn snap(self) -> &'static str {
        match self {
            Self::Cascade => "cascade",
            Self::Restrict => "restrict",
            Self::SetNull => "set_null",
            Self::NoAction => "no_action",
        }
    }
}

/// The parsed `@fk`/`@no_fk` intent carried on a forward relation. Presence is *not*
/// resolved here — that needs the `foreign_keys` convention (see [`RModel::resolved_fk`]).
/// Reason strings + spans ride along so the manifest-dependent divergence pass can check
/// them without re-reading the AST.
#[derive(Debug, Clone, Default)]
pub struct FkDecl {
    /// `@fk` present on this edge.
    pub fk: bool,
    pub fk_reason: Option<String>,
    pub fk_span: Option<Span>,
    /// `@no_fk` present on this edge.
    pub no_fk: bool,
    pub no_fk_reason: Option<String>,
    pub no_fk_span: Option<Span>,
    /// Resolved referential actions (an unknown action maps to `None` here and is `E0294`).
    pub on_delete: Option<FkAction>,
    pub on_update: Option<FkAction>,
}

/// A resolved foreign-key constraint on a forward relation: whether it is emitted is
/// decided by [`RModel::resolved_fk`]; this carries only the actions once it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFk {
    pub on_delete: Option<FkAction>,
    pub on_update: Option<FkAction>,
}

/// The closed set of value-position functions (the grammar leaves the set to sema).
pub const KNOWN_FUNCS: &[&str] = &["now"];

// ---------- resolved schema -----------------------------------------------

/// A checked, cross-linked schema: the IR seed for codegen.
#[derive(Debug, Default)]
pub struct CheckedSchema {
    pub models: Vec<RModel>,
    pub shapes: Vec<RShape>,
    /// Named scope decls, keyed by name in `scope_index`.
    pub scopes: Vec<RScope>,
    /// Enum decls, keyed by name in `enum_index`. A field typed by an enum name is a
    /// scalar column (`MemberKind::Scalar` carrying `enum_name`), never a relation.
    pub enums: Vec<REnum>,
    pub queries: Vec<RQuery>,
    pub mutations: Vec<RMutation>,
    pub filters: Vec<RFilter>,
    /// model name -> index into `models`.
    pub model_index: HashMap<String, usize>,
    /// scope name -> index into `scopes`.
    pub scope_index: HashMap<String, usize>,
    /// enum name -> index into `enums`.
    pub enum_index: HashMap<String, usize>,
}

impl CheckedSchema {
    pub fn model(&self, name: &str) -> Option<&RModel> {
        self.model_index.get(name).map(|&i| &self.models[i])
    }
    pub fn scope(&self, name: &str) -> Option<&RScope> {
        self.scope_index.get(name).map(|&i| &self.scopes[i])
    }
    pub fn enum_(&self, name: &str) -> Option<&REnum> {
        self.enum_index.get(name).map(|&i| &self.enums[i])
    }

    /// The physical FK column(s) a forward relation occupies, each paired with the target
    /// primary-key part it references (its type + physical column). A single-column-key
    /// target yields one entry on the relation's own `fk_col` (its `(column "…")` override
    /// honored). A composite-key target yields one `<field>_<part_col>` column per key
    /// part, in key order — the auto-expanded multi-column FK. Empty for an inverse edge,
    /// a missing target, or a keyless target (no key to reference).
    pub fn fk_columns<'s>(&'s self, mem: &RMember) -> Vec<(String, &'s RMember)> {
        let MemberKind::Forward { target, fk_col, .. } = &mem.kind else {
            return Vec::new();
        };
        let Some(t) = self.model(target) else {
            return Vec::new();
        };
        let parts = t.pk_members();
        if parts.len() <= 1 {
            parts.into_iter().map(|p| (fk_col.clone(), p)).collect()
        } else {
            parts
                .into_iter()
                .map(|p| (format!("{}_{}", mem.name, p.physical_col()), p))
                .collect()
        }
    }
}

/// A resolved `enum Name { … }` decl: its inferred kind and ordered variant list. The
/// variants are the closed member set every enum-typed value is checked against (by
/// name); each carries its wire value (a string or an int).
#[derive(Debug, Clone)]
pub struct REnum {
    pub name: String,
    pub span: Span,
    pub kind: EnumKind,
    pub variants: Vec<REnumVariant>,
}

/// An enum's kind, inferred from its variant values: `Str` when no variant carries an
/// int (bare or explicit-string variants — stored as text + CHECK), `Int` when every
/// variant carries an int (stored as an integer column + CHECK, ordered-comparable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumKind {
    Str,
    Int,
}

/// One resolved variant: the bare identifier name (the Rust variant / go-to-def target)
/// and its wire value.
#[derive(Debug, Clone)]
pub struct REnumVariant {
    pub name: String,
    pub span: Span,
    pub value: EnumValue,
}

/// A variant's wire representation: a string (a string enum) or an integer (an int enum).
#[derive(Debug, Clone, PartialEq)]
pub enum EnumValue {
    Str(String),
    Int(i64),
}

impl REnum {
    pub fn has_variant(&self, v: &str) -> bool {
        self.variants.iter().any(|x| x.name == v)
    }
    pub fn is_int(&self) -> bool {
        self.kind == EnumKind::Int
    }
    /// The wire value of a variant by name, or `None` if it names no variant.
    pub fn wire_of(&self, name: &str) -> Option<&EnumValue> {
        self.variants
            .iter()
            .find(|v| v.name == name)
            .map(|v| &v.value)
    }
    /// The variant names, comma-joinable for a diagnostic's "expected one of" list.
    pub fn variant_names(&self) -> Vec<&str> {
        self.variants.iter().map(|v| v.name.as_str()).collect()
    }
}

/// A resolved `scope` decl: its terms carry the column,
/// the `$ctx` field, and the type declared once here (the one source of truth for
/// both the governed models' column and the `$ctx.field`).
#[derive(Debug, Clone)]
pub struct RScope {
    pub name: String,
    pub span: Span,
    pub terms: Vec<RScopeTerm>,
}

#[derive(Debug, Clone)]
pub struct RScopeTerm {
    /// The scope column (the field name a governed model must carry).
    pub column: String,
    /// The `$ctx.<field>` the column binds to.
    pub ctx_field: String,
    /// The type declared in the decl (`col: Type`) — a primitive or a relation.
    pub ty: CtxField,
}

/// The scope injection a single callable chose for one touched scoped model .
/// A model may declare several `@scope` alternatives (DNF); the callable's `scoped …`
/// clause selects which axes confine *this* callable. `terms` is the flattened
/// `(column_field, ctx_field)` set of the chosen axes — exactly the equalities codegen
/// ANDs into the root `WHERE`, the joined `ON`, and the create auto-set for `model`.
/// Two callables naming different alternatives of the same model therefore inject
/// different predicates. For a single-alternative model this is that model's whole
/// scope, so the emitted SQL is unchanged from iteration 1 .
#[derive(Debug, Clone)]
pub struct ScopeInject {
    /// The touched scoped model this injection confines (by name).
    pub model: String,
    /// The `(column_field, ctx_field)` terms to inject, in scope-decl order, deduped.
    pub terms: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct RModel {
    pub name: String,
    pub span: Span,
    /// Generated table name (`snake_case`) or the `@table("…")` override.
    pub table: String,
    /// `@schema("name")` — the SQL schema (Postgres) / database (MySQL/MariaDB) the table
    /// lives in, so it can be consumed/emitted outside the connection's default namespace.
    /// `None` = the default namespace. Rendered as a per-part-quoted `schema.table` prefix
    /// at every table reference.
    pub schema: Option<String>,
    pub members: Vec<RMember>,
    pub soft_delete: Option<SoftDelete>,
    /// Model default sort (`@sort`); empty when none is declared.
    pub sort: Vec<SortTerm>,
    /// The standing scope filter injected into every read/write on this model
    /// Synthesized from the model's `@scope Name` reference(s) —
    /// the conjunction of the referenced `scope` decls' `col = $ctx.field` terms
    /// (the single alternative, iteration 1). `None` when the model is not scoped.
    /// Codegen lowers it exactly like any `where` , so scope injection is
    /// unchanged in effect from the old inline `@scope(pred)`.
    pub scope: Option<Predicate>,
    /// The model's `@scope` alternatives as scope-name sets (DNF): each
    /// `@scope Name[, Name]*` decorator is one alternative (an AND of names). Empty
    /// when the model is not scoped. Iteration 1 resolves exactly one alternative but
    /// stores a list so multi-scope  adds DNF without reshaping this.
    pub scope_alts: Vec<Vec<String>>,
    /// `@created` / `@updated` engine-managed timestamp fields .
    pub created: Option<String>,
    pub updated: Option<String>,
    pub indexes: Vec<RIndex>,
    /// `@no_id("reason")` — the model is a genuinely keyless legacy table: it carries no
    /// synthesized `id` primary key, and forfeits the id-keyed operations (get-by-id,
    /// the keyset id tiebreaker, create read-back by generated id). `false` for the
    /// ordinary case (a model always has an `id`).
    pub no_id: bool,
    /// `@key(field, …)` — the declared field(s) that *are* the primary key, so no surrogate
    /// `id` is synthesized. Empty for the ordinary case (the model uses `id`) or a keyless
    /// (`@no_id`) model. The single-column case holds one field; composite keys are PR6.
    pub key: Vec<String>,
    /// `@no_fk` on the model — opt *every* forward relation out of an FK constraint (the
    /// whole-table legacy escape). Reason + span ride along for the divergence check.
    pub no_fk: bool,
    pub no_fk_reason: Option<String>,
    pub no_fk_span: Option<Span>,
    /// Field names that are individually unique (id, `(unique)`, single-col unique
    /// index). Drives `get`-must-be-keyed lint and codegen constraints.
    pub unique_cols: Vec<String>,
    /// `@was("old_table")` — the model's previous table name, driving a `rename table`
    /// step in the migration diff instead of drop+add. `None` for
    /// an un-renamed model. Transient: removed once the rename migration is captured.
    pub was: Option<String>,
}

impl RModel {
    pub fn member(&self, name: &str) -> Option<&RMember> {
        self.members.iter().find(|m| m.name == name)
    }
    /// Find a member by its *physical* column name (not the field name): a scalar's
    /// `column` or a forward relation's `fk_col`. Custom `on:` join conditions are
    /// written in terms of DB columns (legacy keys), so they resolve through this,
    /// not `member`.
    pub fn column(&self, col: &str) -> Option<&RMember> {
        self.members.iter().find(|m| match &m.kind {
            MemberKind::Scalar { column, .. } => column == col,
            MemberKind::Forward { fk_col, .. } => fk_col == col,
            MemberKind::Inverse { .. } => false,
        })
    }
    pub fn is_unique(&self, field: &str) -> bool {
        self.unique_cols.iter().any(|c| c == field)
    }

    /// This model's primary-key generation strategy, read off the `id` column's declared
    /// type. `None` for a `@no_id` (keyless) model, and for a `@key`-nominated natural key
    /// (its value is app-supplied like any column, not engine-generated). A `Primitive::Id`
    /// id folds to [`PkStrategy::Uuid`] unless the manifest pass rewrote a non-uuid default.
    pub fn pk_strategy(&self) -> Option<PkStrategy> {
        if self.no_id || !self.key.is_empty() {
            return None;
        }
        match self.member("id").map(|m| &m.kind) {
            Some(MemberKind::Scalar { ty, .. }) => Some(PkStrategy::of(*ty)),
            _ => None,
        }
    }

    /// The field name(s) forming this model's primary key: the `@key(…)` nominated fields,
    /// else the surrogate `id`. Empty for a keyless (`@no_id`) model.
    pub fn pk_field_names(&self) -> Vec<&str> {
        if self.no_id {
            Vec::new()
        } else if self.key.is_empty() {
            vec!["id"]
        } else {
            self.key.iter().map(String::as_str).collect()
        }
    }

    /// The single primary-key field name — `id`, or a single-column `@key(field)` natural
    /// key. `None` for a keyless model or a composite key (use [`pk_field_names`] there).
    pub fn pk_field(&self) -> Option<&str> {
        self.pk_field_names().into_iter().next()
    }

    /// The resolved primary-key member (the `id` field, or the single `@key`-nominated
    /// field). `None` for a keyless model; the first part of a composite key.
    pub fn pk_member(&self) -> Option<&RMember> {
        self.pk_field().and_then(|f| self.member(f))
    }

    /// The physical primary-key column (its `(column "…")` override, else the field name).
    /// `None` for a keyless model; the first column of a composite key.
    pub fn pk_column(&self) -> Option<String> {
        self.pk_member().map(|m| m.physical_col().to_string())
    }

    /// The resolved primary-key member(s), in key order — one for a surrogate `id` or a
    /// single-column `@key`, several for a composite `@key(f1, f2, …)`. Empty for a
    /// keyless model. The one place the whole key is read for multi-column DDL/FK/joins.
    pub fn pk_members(&self) -> Vec<&RMember> {
        self.pk_field_names()
            .into_iter()
            .filter_map(|f| self.member(f))
            .collect()
    }

    /// The physical primary-key column(s), in key order. Empty for a keyless model; more
    /// than one for a composite `@key`.
    pub fn pk_columns(&self) -> Vec<String> {
        self.pk_members()
            .into_iter()
            .map(|m| m.physical_col().to_string())
            .collect()
    }

    /// A composite (multi-column) `@key(f1, f2, …)` primary key — two or more nominated
    /// key columns, so its id surface is structured (a per-part object, not one scalar).
    pub fn is_composite_key(&self) -> bool {
        self.key.len() >= 2
    }

    /// Does this model's PK come from the database (serial), so a `create` omits the id
    /// column and the engine reads the assigned value back?
    pub fn pk_is_db_generated(&self) -> bool {
        self.pk_strategy().is_some_and(PkStrategy::is_db_generated)
    }

    /// The resolved FK constraint for a forward-relation member under the project
    /// `foreign_keys` convention, or `None` when no constraint is emitted. Per-relation /
    /// per-model `@fk`/`@no_fk` always wins over the convention; a custom-join relation
    /// (no conventional FK column) never gets one. Actions declared with `@fk(on_delete: …)`
    /// apply regardless of which side (decorator or convention) supplies the presence.
    pub fn resolved_fk(&self, mem: &RMember, fks: ForeignKeys) -> Option<ResolvedFk> {
        let MemberKind::Forward {
            custom_join, fk, ..
        } = &mem.kind
        else {
            return None;
        };
        if *custom_join || self.no_fk || fk.no_fk {
            return None;
        }
        let present = fk.fk || matches!(fks, ForeignKeys::All);
        present.then_some(ResolvedFk {
            on_delete: fk.on_delete,
            on_update: fk.on_update,
        })
    }
    /// The `@scope` equality terms as `(lhs_field, ctx_field)` pairs : for
    /// `@scope(org = $ctx.org)`, `[("org", "org")]`. Sema restricts `@scope` to a
    /// conjunction of `col = $ctx.field` (`E0180`), so this is exactly the set of
    /// columns the engine injects into every read/write and **auto-sets on create**
    /// from `:ctx_<ctx_field>`. Empty when the model has no `@scope`. A malformed
    /// scope (already `E0180`) contributes only its well-formed terms.
    pub fn scope_terms(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(p) = &self.scope {
            collect_scope_terms(p, &mut out);
        }
        out
    }

    /// The single `$ctx` field a request on this model **shards** on , or `None`
    /// when the model has no `@scope`. A scope is a conjunction of `col = $ctx.field`
    /// ; the shard key is the *owner* the scope filters by, i.e. the `$ctx` field
    /// of the **first** scope term (`@scope(org = $ctx.org)` → `Some("org")`). This is
    /// the one field the router hashes to pick a physical shard (single-shard-
    /// per-request), read from the same `@scope` that filters rows — one source of
    /// truth, so the shard a row lives in and the shard its owner's requests route to
    /// can never drift. A multi-term scope shards on its first `$ctx` field (the
    /// remaining terms narrow *within* that owner's shard); a model with no scope has
    /// no owning shard (single-shard deployments send it to shard 0).
    pub fn shard_key_ctx_field(&self) -> Option<String> {
        self.scope_terms().into_iter().next().map(|(_, ctx)| ctx)
    }
}

/// Flatten a well-formed `@scope` predicate (an `and`-tree of `col = $ctx.field`) into
/// its `(lhs_field, ctx_field)` terms. Non-conforming nodes are skipped (they are the
/// `E0180` the caller already reported); this never errors.
fn collect_scope_terms(p: &Predicate, out: &mut Vec<(String, String)>) {
    match p {
        Predicate::And(a, b) => {
            collect_scope_terms(a, out);
            collect_scope_terms(b, out);
        }
        Predicate::Cmp {
            path,
            op: based_ast::Op::Eq,
            value: based_ast::Value::Param(pr),
        } if path.segments.len() == 1 && pr.name.node == "ctx" && pr.path.len() == 1 => {
            out.push((path.segments[0].node.clone(), pr.path[0].node.clone()));
        }
        _ => {}
    }
}

/// One resolved field: a scalar column or a relation edge.
#[derive(Debug, Clone)]
pub struct RMember {
    pub name: String,
    pub span: Span,
    pub kind: MemberKind,
    /// `@was("old_col")` — the field's previous physical column name, driving a
    /// `rename column` step in the migration diff. `None` for an
    /// un-renamed field. Transient: removed once the rename migration is captured.
    pub was: Option<String>,
    /// Relation `@sort` — orders the target's rows when reached *via this edge*
    /// (most visibly, a to-many nest's array). Overrides the target model's own
    /// `@sort` for that traversal; empty when undeclared (fall back to the model's).
    pub sort: Vec<SortTerm>,
}

#[derive(Debug, Clone)]
pub enum MemberKind {
    /// A stored column. `column` is the physical name (`(column "…")` override or
    /// the field name verbatim).
    Scalar {
        ty: Primitive,
        optional: bool,
        many: bool,
        column: String,
        /// `(unique)` modifier — a single-column UNIQUE constraint at codegen.
        unique: bool,
        /// `(default …)` value, carried through for DDL column defaults.
        default: Option<DefaultVal>,
        /// The enum this column is typed by, when its declared type resolved to an
        /// `enum` decl (`status: Status`). `Some(name)` marks an enum-valued column —
        /// stored as text (`ty` is `Text`), constrained to the enum's variants, emitted
        /// as a real enum in the client. `None` for an ordinary primitive column.
        enum_name: Option<String>,
        /// `raw("geometry(Point,4326)")` — the column's opaque DB type. `Some` marks the
        /// column engine-unmodelled: the literal string is what DDL and the snapshot
        /// carry, the value is opaque to the client, and filtering/sorting/assigning it
        /// is rejected. `ty` is `Text` so the value rides the ordinary text path.
        raw_type: Option<RawSpec>,
    },
    /// To-one relation: FK lives on this table (`<field>_id`, or a custom join).
    Forward {
        target: String,
        optional: bool,
        fk_col: String,
        custom_join: bool,
        /// The `@fk`/`@no_fk` intent on this edge (presence resolved via
        /// [`RModel::resolved_fk`] against the `foreign_keys` convention).
        fk: FkDecl,
    },
    /// Back edge (to-many, or a one-to-one inverse): FK lives on `target`, paired
    /// with its forward field `via`.
    Inverse { target: String, via: String },
}

impl RMember {
    /// The member's physical column name: a scalar's `column`, a forward relation's
    /// `fk_col`, else the field name (an inverse owns no column). The rename target a
    /// `@was` maps its old name to.
    pub fn physical_col(&self) -> &str {
        match &self.kind {
            MemberKind::Scalar { column, .. } => column,
            MemberKind::Forward { fk_col, .. } => fk_col,
            MemberKind::Inverse { .. } => &self.name,
        }
    }
}

impl MemberKind {
    /// The column's opaque `raw(…)` type, when it has one.
    pub fn opaque(&self) -> Option<&RawSpec> {
        match self {
            Self::Scalar { raw_type, .. } => raw_type.as_ref(),
            _ => None,
        }
    }
    pub fn is_relation(&self) -> bool {
        !matches!(self, Self::Scalar { .. })
    }
    pub fn target(&self) -> Option<&str> {
        match self {
            Self::Forward { target, .. } | Self::Inverse { target, .. } => Some(target),
            Self::Scalar { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftMode {
    /// nullable `timestamp`/`date`: live `IS NULL`.
    Timestamp,
    /// `bool`: live `= false`.
    Bool,
}

#[derive(Debug, Clone)]
pub struct SoftDelete {
    pub field: String,
    pub mode: SoftMode,
}

#[derive(Debug, Clone)]
pub struct RIndex {
    pub columns: Vec<String>,
    pub unique: bool,
    /// `using <method>` — the declared access method, lowercased. `None` = the
    /// dialect's default.
    pub method: Option<String>,
    /// `@index raw("…")` — an opaque index body. When `Some`, `columns` is empty.
    pub raw: Option<RawSpec>,
    pub span: Span,
}

/// The compile targets a `raw({ … })` map and a `using <method>` check may name. The
/// spelling is the canonical dialect name codegen reports.
pub const DIALECTS: &[&str] = &["mariadb", "postgres", "sqlite"];

/// The index access methods `using <method>` accepts, and which targets have them.
/// SQLite has no access-method syntax at all, so every method is an error there
/// (`E0272`) rather than a silent downgrade to a plain index.
pub const INDEX_METHODS: &[(&str, &[&str])] = &[
    ("btree", &["postgres", "mariadb"]),
    ("hash", &["postgres", "mariadb"]),
    ("gist", &["postgres"]),
    ("spgist", &["postgres"]),
    ("gin", &["postgres"]),
    ("brin", &["postgres"]),
    ("fulltext", &["mariadb"]),
    ("spatial", &["mariadb"]),
];

/// The targets an index access method is valid on, or `None` when the name is not a
/// known method at all.
pub fn index_method_targets(method: &str) -> Option<&'static [&'static str]> {
    INDEX_METHODS
        .iter()
        .find(|(m, _)| *m == method)
        .map(|(_, ts)| *ts)
}

#[derive(Debug, Clone)]
pub struct RShape {
    pub name: String,
    pub from: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct RQuery {
    pub name: String,
    pub span: Span,
    /// Model the query reads from (inferred from the return shape).
    pub target: String,
    /// `get`/`list` — explicit in a block body, inferred from cardinality otherwise.
    pub verb: Verb,
    pub many: bool,
    /// Declared `-> stream Shape`: the same many-cardinality read (identical SQL),
    /// delivered row-by-row on the wire instead of as one collected array.
    pub stream: bool,
    /// The return shape, or `None` when the return type is a bare model.
    pub ret_shape: Option<String>,
    pub paginated: bool,
    /// The `$ctx.<field>` this query requires (its own `where` + the target model's
    /// `@scope` + expanded filters), each typed by inference . Deduped per
    /// callable; the client sends exactly these as request context.
    pub ctx_requires: Vec<CtxReq>,
    /// The `$ctx` field this query **shards** on : the target model's `@scope`
    /// owner field ([`RModel::shard_key_ctx_field`]), or `None` when the model has no
    /// `@scope` *or* the query is `unscoped` (a cross-scope read has no single
    /// owning shard, so it must route explicitly, never by a scope it disabled). The
    /// runtime pulls this field out of the request `$ctx` to route to one shard.
    pub shard_key: Option<String>,
    /// The per-touched-model scope injection this query chose: for
    /// each scoped model it reads (root + every joined reach), the terms of the
    /// alternative its `scoped …` clause satisfied. Empty when `unscoped` or nothing
    /// scoped is touched. Codegen injects exactly these, so a callable naming one
    /// alternative and another naming a different one filter by different predicates.
    pub scope_inject: Vec<ScopeInject>,
}

#[derive(Debug, Clone)]
pub struct RMutation {
    pub name: String,
    pub span: Span,
    /// The primary written model: the declared return's model, or — for an `-> ok`
    /// acknowledgement — the first real DELETE's model (there is no declared return).
    pub ret_model: String,
    /// Declared `-> ok`: a destructive mutation with no surviving row. No re-select,
    /// wire `{}`, unit-returning client method; a zero-row DELETE is a 404 `not_found`.
    pub ack: bool,
    /// The `guard <name>` host hook (auth.md Handle 3), or `None`. The name is a
    /// host-language function's — nothing in the schema defines it; the runtime
    /// invokes the registered fn before the write body and enforces its verdict.
    pub guard: Option<String>,
    /// The return shape, or `None` when the return type is a bare model — the twin of
    /// [`RQuery::ret_shape`]. Codegen projects it when re-selecting the written row's
    /// declared shape after the write .
    pub ret_shape: Option<String>,
    /// The `$ctx.<field>` this mutation requires (its write `where`s + the write
    /// models' `@scope` + `create`/`update` assigns), each typed by inference
    /// Deduped per callable.
    pub ctx_requires: Vec<CtxReq>,
    /// The `$ctx` field this mutation **shards** on : the return model's `@scope`
    /// owner field ([`RModel::shard_key_ctx_field`]), or `None` when it has no `@scope`
    /// *or* the mutation is `unscoped` . A `tx` is a single-shard unit, so
    /// the whole mutation routes on this one field (the return model is the primary
    /// written model). The runtime pulls it out of the request `$ctx` to pick a shard.
    pub shard_key: Option<String>,
    /// The per-touched-model scope injection this mutation chose: the
    /// twin of [`RQuery::scope_inject`] for the write side — the chosen alternative's
    /// terms per written/joined scoped model, injected into every write `WHERE`, the
    /// joined `ON`, the create auto-set, and the declared-shape re-select. Empty when
    /// `unscoped`.
    pub scope_inject: Vec<ScopeInject>,
}

#[derive(Debug, Clone)]
pub struct RFilter {
    pub name: String,
    pub span: Span,
    pub arity: usize,
}

// ---------- diagnostics sink ----------------------------------------------

/// Thin accumulator so passes can push errors/warnings without ceremony.
#[derive(Default)]
pub struct Sink {
    pub diags: Vec<Diagnostic>,
}

impl Sink {
    pub fn error(&mut self, code: &'static str, span: Span, msg: impl Into<String>) {
        self.diags.push(Diagnostic::error(code, msg).at(span));
    }
    pub fn warn(&mut self, code: &'static str, span: Span, msg: impl Into<String>) {
        self.diags.push(Diagnostic::warning(code, msg).at(span));
    }
    pub fn error_note(
        &mut self,
        code: &'static str,
        span: Span,
        msg: impl Into<String>,
        note: impl Into<String>,
    ) {
        self.diags
            .push(Diagnostic::error(code, msg).at(span).note(note));
    }
    pub fn warn_note(
        &mut self,
        code: &'static str,
        span: Span,
        msg: impl Into<String>,
        note: impl Into<String>,
    ) {
        self.diags
            .push(Diagnostic::warning(code, msg).at(span).note(note));
    }

    /// An error carrying a note and a one-key autofix (insert `line` into model
    /// `model`'s body).
    #[allow(clippy::too_many_arguments)]
    pub fn error_fix(
        &mut self,
        code: &'static str,
        span: Span,
        msg: impl Into<String>,
        note: impl Into<String>,
        model: impl Into<String>,
        line: impl Into<String>,
    ) {
        self.diags.push(
            Diagnostic::error(code, msg)
                .at(span)
                .note(note)
                .with_fix(model, line),
        );
    }
}

/// One `$ctx.<field>` requirement of a single callable : the field name and
/// the type it was used at, inferred from the column the use compared against.
/// `$ctx` is per-request; there is no global context type — each query/mutation
/// requires exactly the fields *it* (plus its `@scope`/filters) reads. Cross-
/// callable coherence (a field must mean one type everywhere the caller's context
/// bag is shared) is checked separately (`CTX_CONFLICT`).
#[derive(Debug, Clone)]
pub struct CtxReq {
    pub field: String,
    pub ty: CtxField,
    pub span: Span,
}

/// A `$ctx` field's inferred type: a primitive, or a relation to a model (the
/// caller supplies that model's key).
#[derive(Debug, Clone)]
pub enum CtxField {
    Scalar(Primitive),
    Relation(String),
}

/// Table name for a model : `snake_case(Name)`, no pluralization.
pub fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.char_indices() {
        if c.is_ascii_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}
