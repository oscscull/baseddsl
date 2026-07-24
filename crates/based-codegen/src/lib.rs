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
/// - `MariaDb` — the original target. MySQL maps here too (a MariaDB fork; the
///   emitted SQL — backtick idents, `DATETIME`, `MEMBER OF`, positional `?` — is
///   MySQL-8-compatible).
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
            "mariadb" | "mysql" => Self::MariaDb,
            _ => Self::MariaDb,
        }
    }

    /// The dialect's manifest name — used in the generated-header comment so the
    /// emitted artifact records which target it was compiled for.
    pub fn name(self) -> &'static str {
        match self {
            Self::MariaDb => "mariadb",
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
        }
    }

    /// Quote a single SQL identifier for this dialect. MySQL/MariaDB and SQLite use
    /// backticks (`` `order` ``); Postgres uses ANSI double quotes (`"order"`). An
    /// embedded quote char is doubled, the standard escape in both quoting styles.
    /// This is the one difference that pervades the DML/mutation SQL, so it is routed
    /// through here rather than hardcoded at each `format!` site.
    pub fn quote(self, ident: &str) -> String {
        match self {
            Self::MariaDb | Self::Sqlite => {
                format!("`{}`", ident.replace('`', "``"))
            }
            Self::Postgres => format!("\"{}\"", ident.replace('"', "\"\"")),
        }
    }

    /// A `table`.`column` qualified reference, each part quoted for the dialect.
    pub fn qcol(self, table: &str, column: &str) -> String {
        format!("{}.{}", self.quote(table), self.quote(column))
    }

    /// The boolean literal spelling. MariaDB/Postgres have the `TRUE`/`FALSE`
    /// keywords; SQLite stores bools as integers, so it is `1`/`0`.
    pub fn bool_lit(self, b: bool) -> &'static str {
        match self {
            Self::MariaDb | Self::Postgres => {
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
            Self::MariaDb => "JSON_OBJECT",
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
            Self::MariaDb => format!("COALESCE(JSON_ARRAYAGG({ordered}), JSON_ARRAY())"),
            Self::Postgres => format!("COALESCE(json_agg({ordered}), '[]'::json)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Dialect;

    #[test]
    fn parse_maps_names_and_defaults_to_mariadb() {
        assert_eq!(Dialect::parse("mariadb"), Dialect::MariaDb);
        assert_eq!(Dialect::parse("mysql"), Dialect::MariaDb);
        assert_eq!(Dialect::parse("sqlite"), Dialect::Sqlite);
        assert_eq!(Dialect::parse("postgres"), Dialect::Postgres);
        assert_eq!(Dialect::parse("postgresql"), Dialect::Postgres);
        // an unknown value is not a schema error — fall back to the documented default.
        assert_eq!(Dialect::parse("nope"), Dialect::MariaDb);
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
