//! based-codegen — turns a [`CheckedSchema`] into target artifacts.
//!
//! SQL **DDL** ([`sql::ddl`]): `CheckedSchema` -> `CREATE TABLE` (M2). SQL **DML**:
//! queries -> parameterized `SELECT`s ([`sql::dml`]) and mutations ->
//! INSERT/UPDATE/DELETE ([`sql::mutations`]) (M3, read + write). The typed **client**
//! ([`client`]): `CheckedSchema` -> a Rust client module (M4). The **OpenAPI** spec
//! ([`openapi`]): `CheckedSchema` -> one OpenAPI document over the same wire, so
//! `openapi-generator` yields clients in any language (polyglot via one emitter, not
//! N — D23). Each is a module reading the same resolved IR.
//!
//! The compiler seed is `based_sema::CheckedSchema`. Codegen never re-derives
//! resolution facts (table names, FK columns, soft-delete mode) — those live on the
//! IR. It only picks physical representations (SQL types, index names) per dialect.

pub mod client;
pub mod openapi;
pub mod sql;

/// The SQL compile target (manifest `dialect`). MariaDB is the default (D5).
///
/// - `MariaDb` — the original target. MySQL maps here too (a MariaDB fork; the
///   emitted SQL — backtick idents, `DATETIME`, `MEMBER OF`, positional `?` — is
///   MySQL-8-compatible).
/// - `Sqlite` (D27/D28) — the infra-free backend + its DDL. Backtick idents, `IS
///   NULL`, `= TRUE`, positional `?` all run on SQLite too, so only DDL branches.
/// - `Postgres` (D29) — the standards-track target. It diverges the most: identifiers
///   are double-quoted (`"order"`), placeholders are `$1, $2, …` (not `?` — the one
///   coupling D21 flagged, fixed in the runtime scanner), JSON containment is `@>`
///   (not MySQL's `MEMBER OF`), and the multi-table UPDATE/DELETE forms use
///   `FROM`/`USING` rather than MySQL's `JOIN` clause. Its `Db`/`Backend` driver is
///   deferred to the live-DB slice (needs a real server to be meaningful, like
///   MariaDb's — see D29); this variant is the *codegen* + scanner half.
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
    pub fn parse(s: &str) -> Dialect {
        match s {
            "sqlite" => Dialect::Sqlite,
            "postgres" | "postgresql" => Dialect::Postgres,
            "mariadb" | "mysql" => Dialect::MariaDb,
            _ => Dialect::MariaDb,
        }
    }

    /// The dialect's manifest name — used in the generated-header comment so the
    /// emitted artifact records which target it was compiled for.
    pub fn name(self) -> &'static str {
        match self {
            Dialect::MariaDb => "mariadb",
            Dialect::Sqlite => "sqlite",
            Dialect::Postgres => "postgres",
        }
    }

    /// Quote a single SQL identifier for this dialect. MySQL/MariaDB and SQLite use
    /// backticks (`` `order` ``); Postgres uses ANSI double quotes (`"order"`). An
    /// embedded quote char is doubled, the standard escape in both quoting styles.
    /// This is the one difference that pervades the DML/mutation SQL, so it is routed
    /// through here rather than hardcoded at each `format!` site.
    pub fn quote(self, ident: &str) -> String {
        match self {
            Dialect::MariaDb | Dialect::Sqlite => {
                format!("`{}`", ident.replace('`', "``"))
            }
            Dialect::Postgres => format!("\"{}\"", ident.replace('"', "\"\"")),
        }
    }

    /// A `table`.`column` qualified reference, each part quoted for the dialect.
    pub fn qcol(self, table: &str, column: &str) -> String {
        format!("{}.{}", self.quote(table), self.quote(column))
    }

    /// The boolean literal spelling. MariaDB/Postgres have the `TRUE`/`FALSE`
    /// keywords; SQLite stores bools as integers (D27), so it is `1`/`0`.
    pub fn bool_lit(self, b: bool) -> &'static str {
        match self {
            Dialect::MariaDb | Dialect::Postgres => {
                if b {
                    "TRUE"
                } else {
                    "FALSE"
                }
            }
            Dialect::Sqlite => {
                if b {
                    "1"
                } else {
                    "0"
                }
            }
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
