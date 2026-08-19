//! based-codegen — turns a [`CheckedSchema`] into target artifacts.
//!
//! SQL **DDL** ([`sql::ddl`]): `CheckedSchema` -> `CREATE TABLE`. SQL **DML**:
//! queries -> parameterized `SELECT`s ([`sql::dml`]) and mutations ->
//! INSERT/UPDATE/DELETE ([`sql::mutations`]). The typed **client**
//! ([`client`]): `CheckedSchema` -> a Rust client module. The **OpenAPI** spec
//! ([`openapi`]): `CheckedSchema` -> one OpenAPI document over the same wire, so
//! `openapi-generator` yields clients in any language (polyglot via one emitter, not
//! N). The **migration** engine ([`migrate`]): `CheckedSchema` -> a canonical
//! `schema.snap` + a `diff` producing the dialect-neutral `up.mig` step list. Each
//! is a module reading the same resolved IR.
//!
//! The compiler seed is `based_sema::CheckedSchema`. Codegen never re-derives
//! resolution facts (table names, FK columns, soft-delete mode) — those live on the
//! IR. It only picks physical representations (SQL types, index names) per dialect.

pub mod client;
pub mod migrate;
pub mod openapi;
pub mod sql;

/// The SQL compile target (manifest `dialect`). MariaDB is the default.
///
/// - `MariaDb` — the original target. Backtick idents, `DATETIME`, `MEMBER OF`,
///   positional `?`.
/// - `MySql` — Oracle MySQL. Shares MariaDB's wire protocol, quoting, operators, and
///   DDL syntax, so the *driver* is shared; a distinct codegen variant because the two
///   diverge on type support (e.g. MySQL has no native `UUID` type) and will diverge
///   further. The MySQL/MariaDB *family* ([`Dialect::is_mysql_family`]) shares almost
///   every branch.
/// - `Sqlite` — the infra-free backend + its DDL. Backtick idents, `IS NULL`, `= TRUE`,
///   positional `?` all run on SQLite too, so only DDL branches.
/// - `Postgres` — the standards-track target. It diverges the most: identifiers are
///   double-quoted (`"order"`), placeholders are `$1, $2, …` (not `?` — the one such
///   coupling, fixed in the runtime scanner), JSON containment is `@>` (not MySQL's
///   `MEMBER OF`), and the multi-table UPDATE/DELETE forms use `FROM`/`USING` rather
///   than MySQL's `JOIN` clause. Its `Db`/`Backend` driver lives in `based-runtime`
///   (`postgres`); this variant is the *codegen* + scanner half.
///
/// Two things branch on the dialect: identifier quoting + a handful of operator/type
/// spellings ([`Dialect::quote`], [`Dialect::bool_lit`], …) used by the DML/mutation
/// emitters, and the DDL type map + index syntax (`sql::ddl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    MariaDb,
    MySql,
    Sqlite,
    Postgres,
}

impl Dialect {
    /// Parse the manifest `dialect` string. Unknown values fall back to MariaDB
    /// (the documented default) rather than failing — dialect selection is not a
    /// schema error.
    pub fn parse(s: &str) -> Self {
        match s {
            "sqlite" => Self::Sqlite,
            "postgres" | "postgresql" => Self::Postgres,
            "mysql" => Self::MySql,
            "mariadb" => Self::MariaDb,
            _ => Self::MariaDb,
        }
    }

    /// The dialect's manifest name — used in the generated-header comment so the
    /// emitted artifact records which target it was compiled for.
    pub fn name(self) -> &'static str {
        match self {
            Self::MariaDb => "mariadb",
            Self::MySql => "mysql",
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
        }
    }

    /// The MySQL/MariaDB family — the two share a wire protocol, backtick quoting,
    /// operators, and DDL syntax, so nearly every branch treats them identically.
    pub fn is_mysql_family(self) -> bool {
        matches!(self, Self::MariaDb | Self::MySql)
    }

    /// Quote a single SQL identifier for this dialect. MySQL/MariaDB and SQLite use
    /// backticks (`` `order` ``); Postgres uses ANSI double quotes (`"order"`). An
    /// embedded quote char is doubled, the standard escape in both quoting styles.
    /// This is the one difference that pervades the DML/mutation SQL, so it is routed
    /// through here rather than hardcoded at each `format!` site.
    pub fn quote(self, ident: &str) -> String {
        match self {
            Self::MariaDb | Self::MySql | Self::Sqlite => {
                format!("`{}`", ident.replace('`', "``"))
            }
            Self::Postgres => format!("\"{}\"", ident.replace('"', "\"\"")),
        }
    }

    /// A `table`.`column` qualified reference, each part quoted for the dialect.
    pub fn qcol(self, table: &str, column: &str) -> String {
        format!("{}.{}", self.quote(table), self.quote(column))
    }

    /// A table reference, optionally prefixed by its `@schema("…")` namespace: bare
    /// `` `table` `` in the default namespace, or `` `schema`.`table` `` when the model is
    /// placed in a named SQL schema (Postgres) / database (MySQL/MariaDB). Each part is
    /// quoted separately — the dot is a separator, never quoted. The syntax is identical
    /// across dialects; only the concept the namespace denotes differs.
    pub fn quote_table(self, schema: Option<&str>, table: &str) -> String {
        match schema {
            Some(s) => format!("{}.{}", self.quote(s), self.quote(table)),
            None => self.quote(table),
        }
    }

    /// The boolean literal spelling. MariaDB/Postgres have the `TRUE`/`FALSE`
    /// keywords; SQLite stores bools as integers, so it is `1`/`0`.
    pub fn bool_lit(self, b: bool) -> &'static str {
        match self {
            Self::MariaDb | Self::MySql | Self::Postgres => {
                if b {
                    "TRUE"
                } else {
                    "FALSE"
                }
            }
            Self::Sqlite => {
                if b {
                    "1"
                } else {
                    "0"
                }
            }
        }
    }

    /// The JSON-object constructor for this dialect, taking `'key', value, …` pairs:
    /// `json_object` (SQLite), `JSON_OBJECT` (MariaDB), `json_build_object` (Postgres).
    /// One element of a to-many nested shape array (L1) is built with it.
    pub fn json_object_fn(self) -> &'static str {
        match self {
            Self::Sqlite => "json_object",
            Self::MariaDb | Self::MySql => "JSON_OBJECT",
            Self::Postgres => "json_build_object",
        }
    }

    /// Aggregate a per-row JSON object `elem` into a JSON array — the value of a to-many
    /// nested shape edge (`items { … }`, L1). Coalesced to `[]` for an empty group, since
    /// MariaDB/Postgres aggregate a NULL over zero rows (SQLite's `json_group_array`
    /// already yields `[]`, so the coalesce there is a harmless no-op). `order_by`
    /// (rendered `col dir, …` sort keys) rides *inside* the aggregate — all three
    /// dialects support an ordered aggregate (SQLite ≥ 3.44, which the bundled build
    /// exceeds) — so the array's element order is the child rows' sort order.
    pub fn json_array_agg(self, elem: &str, order_by: Option<&str>) -> String {
        let ordered = match order_by {
            Some(o) => format!("{elem} ORDER BY {o}"),
            None => elem.to_string(),
        };
        match self {
            Self::Sqlite => format!("json_group_array({ordered})"),
            Self::MariaDb | Self::MySql => {
                format!("COALESCE(JSON_ARRAYAGG({ordered}), JSON_ARRAY())")
            }
            Self::Postgres => format!("COALESCE(json_agg({ordered}), '[]'::json)"),
        }
    }

    /// The SQL that opens a transaction at the requested [`Isolation`] + [`AccessMode`],
    /// per dialect. Routed through this seam so the isolation spelling can never drift
    /// from the compile target. The runtime driver issues [`TxBegin::pre`] (if any) on the
    /// connection, then opens the transaction with [`TxBegin::begin`] (a custom `BEGIN …`)
    /// or its default `BEGIN` when `begin` is `None`.
    ///
    /// - **Postgres** — one `BEGIN ISOLATION LEVEL <level> READ WRITE|READ ONLY`.
    /// - **MySQL/MariaDB** — no `BEGIN ISOLATION LEVEL` form, so a
    ///   `SET TRANSACTION ISOLATION LEVEL <level>, READ WRITE|READ ONLY` (which applies to
    ///   the very next transaction) runs first, then the default `BEGIN`.
    /// - **SQLite** — no SQL-standard levels; the access/serializability intent maps to the
    ///   locking mode of `BEGIN`: `SERIALIZABLE` → `BEGIN EXCLUSIVE`, else read-only →
    ///   `BEGIN DEFERRED`, else (read-write) → `BEGIN IMMEDIATE`.
    pub fn begin_transaction_sql(self, isolation: Isolation, access: AccessMode) -> TxBegin {
        match self {
            Self::Postgres => TxBegin {
                pre: None,
                begin: Some(format!(
                    "BEGIN ISOLATION LEVEL {} {}",
                    isolation.sql_level(),
                    access.sql_mode()
                )),
            },
            Self::MariaDb | Self::MySql => TxBegin {
                pre: Some(format!(
                    "SET TRANSACTION ISOLATION LEVEL {}, {}",
                    isolation.sql_level(),
                    access.sql_mode()
                )),
                begin: None,
            },
            Self::Sqlite => {
                let mode = if isolation == Isolation::Serializable {
                    "EXCLUSIVE"
                } else if access == AccessMode::ReadOnly {
                    "DEFERRED"
                } else {
                    "IMMEDIATE"
                };
                TxBegin {
                    pre: None,
                    begin: Some(format!("BEGIN {mode}")),
                }
            }
        }
    }

    /// The row-locking clause a `get|list … for update` query appends after `ORDER BY`/`LIMIT`
    /// (transactions.md). Postgres and the MySQL/MariaDB family take `FOR UPDATE`. SQLite has no
    /// row-level lock, so it returns the empty string (a no-op): its transaction locks the whole
    /// database (`BEGIN IMMEDIATE`/`EXCLUSIVE`) and already serializes writers, so the lock intent
    /// is honored at the transaction boundary rather than per row. Routed through this seam so the
    /// spelling can never drift from the compile target.
    pub fn for_update_clause(self) -> &'static str {
        match self {
            Self::Postgres | Self::MariaDb | Self::MySql => "FOR UPDATE",
            Self::Sqlite => "",
        }
    }
}

/// A transaction isolation level, a parameter on every transaction entry point. The
/// levels are the SQL-standard set the three targets share; SQLite (which has no standard
/// levels) maps them onto its `BEGIN` locking modes ([`Dialect::begin_transaction_sql`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Isolation {
    /// Each statement sees rows committed before it began — the default, and the weakest
    /// level all three engines agree on.
    #[default]
    ReadCommitted,
    /// Every read in the transaction sees the same snapshot taken at first read.
    RepeatableRead,
    /// The transaction runs as if transactions were serial; the engine may abort it with a
    /// serialization failure the caller retries (the optimistic-concurrency payoff).
    Serializable,
}

impl Isolation {
    /// The SQL-standard level name (`READ COMMITTED`, …) — Postgres/MySQL/MariaDB spelling.
    pub fn sql_level(self) -> &'static str {
        match self {
            Self::ReadCommitted => "READ COMMITTED",
            Self::RepeatableRead => "REPEATABLE READ",
            Self::Serializable => "SERIALIZABLE",
        }
    }
}

/// Whether a transaction may write. `ReadOnly` lets the engine reject writes and optimize;
/// on SQLite it selects the lighter `BEGIN DEFERRED` lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccessMode {
    /// The default: reads and writes.
    #[default]
    ReadWrite,
    /// Reads only — a write is a database error.
    ReadOnly,
}

impl AccessMode {
    /// The SQL access-mode clause (`READ WRITE` / `READ ONLY`).
    pub fn sql_mode(self) -> &'static str {
        match self {
            Self::ReadWrite => "READ WRITE",
            Self::ReadOnly => "READ ONLY",
        }
    }
}

/// The per-dialect SQL that opens a transaction ([`Dialect::begin_transaction_sql`]): an
/// optional statement to run on the connection *before* opening the transaction, and an
/// optional custom `BEGIN …` (`None` = the driver's default `BEGIN`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxBegin {
    pub pre: Option<String>,
    pub begin: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{AccessMode, Dialect, Isolation};

    #[test]
    fn for_update_clause_per_dialect() {
        // Postgres + MySQL/MariaDB family lock rows with FOR UPDATE; SQLite has no
        // row-level lock, so the clause is empty (its transaction locks the whole database).
        assert_eq!(Dialect::Postgres.for_update_clause(), "FOR UPDATE");
        assert_eq!(Dialect::MariaDb.for_update_clause(), "FOR UPDATE");
        assert_eq!(Dialect::MySql.for_update_clause(), "FOR UPDATE");
        assert_eq!(Dialect::Sqlite.for_update_clause(), "");
    }

    #[test]
    fn begin_transaction_sql_per_dialect() {
        // Postgres: one BEGIN ISOLATION LEVEL … with the access mode.
        let pg =
            Dialect::Postgres.begin_transaction_sql(Isolation::Serializable, AccessMode::ReadWrite);
        assert_eq!(pg.pre, None);
        assert_eq!(
            pg.begin.as_deref(),
            Some("BEGIN ISOLATION LEVEL SERIALIZABLE READ WRITE")
        );
        let pg_ro = Dialect::Postgres
            .begin_transaction_sql(Isolation::RepeatableRead, AccessMode::ReadOnly);
        assert_eq!(
            pg_ro.begin.as_deref(),
            Some("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
        );

        // MySQL/MariaDB: a SET TRANSACTION pre-statement, then the default BEGIN.
        let maria =
            Dialect::MariaDb.begin_transaction_sql(Isolation::ReadCommitted, AccessMode::ReadWrite);
        assert_eq!(
            maria.pre.as_deref(),
            Some("SET TRANSACTION ISOLATION LEVEL READ COMMITTED, READ WRITE")
        );
        assert_eq!(maria.begin, None);
        assert_eq!(
            Dialect::MySql
                .begin_transaction_sql(Isolation::Serializable, AccessMode::ReadOnly)
                .pre
                .as_deref(),
            Some("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE, READ ONLY")
        );

        // SQLite: the locking mode of BEGIN carries the intent.
        assert_eq!(
            Dialect::Sqlite
                .begin_transaction_sql(Isolation::Serializable, AccessMode::ReadWrite)
                .begin
                .as_deref(),
            Some("BEGIN EXCLUSIVE")
        );
        assert_eq!(
            Dialect::Sqlite
                .begin_transaction_sql(Isolation::ReadCommitted, AccessMode::ReadOnly)
                .begin
                .as_deref(),
            Some("BEGIN DEFERRED")
        );
        assert_eq!(
            Dialect::Sqlite
                .begin_transaction_sql(Isolation::ReadCommitted, AccessMode::ReadWrite)
                .begin
                .as_deref(),
            Some("BEGIN IMMEDIATE")
        );
    }
}

#[cfg(test)]
mod dialect_tests {
    use super::Dialect;

    #[test]
    fn parse_maps_names_and_defaults_to_mariadb() {
        assert_eq!(Dialect::parse("mariadb"), Dialect::MariaDb);
        assert_eq!(Dialect::parse("mysql"), Dialect::MySql);
        assert_eq!(Dialect::parse("sqlite"), Dialect::Sqlite);
        assert_eq!(Dialect::parse("postgres"), Dialect::Postgres);
        assert_eq!(Dialect::parse("postgresql"), Dialect::Postgres);
        // an unknown value is not a schema error — fall back to the documented default.
        assert_eq!(Dialect::parse("nope"), Dialect::MariaDb);
    }

    #[test]
    fn mysql_shares_the_mariadb_family_spellings() {
        assert!(Dialect::MySql.is_mysql_family());
        assert!(Dialect::MariaDb.is_mysql_family());
        assert!(!Dialect::Postgres.is_mysql_family());
        assert!(!Dialect::Sqlite.is_mysql_family());
        assert_eq!(Dialect::MySql.name(), "mysql");
        assert_eq!(Dialect::MySql.quote("order"), "`order`");
        assert_eq!(Dialect::MySql.bool_lit(true), "TRUE");
        assert_eq!(Dialect::MySql.json_object_fn(), "JSON_OBJECT");
    }

    #[test]
    fn quote_style_and_bool_literal_per_dialect() {
        // Backtick vs. double-quote, with the escape char doubled.
        assert_eq!(Dialect::MariaDb.quote("order"), "`order`");
        assert_eq!(Dialect::Sqlite.quote("order"), "`order`");
        assert_eq!(Dialect::Postgres.quote("order"), "\"order\"");
        assert_eq!(Dialect::Postgres.quote("a\"b"), "\"a\"\"b\"");
        assert_eq!(Dialect::MariaDb.qcol("order", "id"), "`order`.`id`");
        assert_eq!(Dialect::Postgres.qcol("order", "id"), "\"order\".\"id\"");
        // bool literal: keyword on MariaDB/Postgres, integer on SQLite.
        assert_eq!(Dialect::MariaDb.bool_lit(true), "TRUE");
        assert_eq!(Dialect::Postgres.bool_lit(false), "FALSE");
        assert_eq!(Dialect::Sqlite.bool_lit(true), "1");
    }
}
