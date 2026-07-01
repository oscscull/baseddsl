//! based-codegen — turns a [`CheckedSchema`] into target artifacts.
//!
//! SQL **DDL** ([`sql::ddl`]): `CheckedSchema` -> `CREATE TABLE` (M2). SQL **DML**:
//! queries -> parameterized `SELECT`s ([`sql::dml`]) and mutations ->
//! INSERT/UPDATE/DELETE ([`sql::mutations`]) (M3, read + write). The typed **client**
//! ([`client`]): `CheckedSchema` -> a Rust client module (M4). Each is a module
//! reading the same resolved IR.
//!
//! The compiler seed is `based_sema::CheckedSchema`. Codegen never re-derives
//! resolution facts (table names, FK columns, soft-delete mode) — those live on the
//! IR. It only picks physical representations (SQL types, index names) per dialect.

pub mod client;
pub mod sql;

/// The SQL compile target (manifest `dialect`). MariaDB is first and only for now
/// (D5); the enum exists so `sql::ddl` can branch when a second dialect lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    MariaDb,
}

impl Dialect {
    /// Parse the manifest `dialect` string. Unknown values fall back to MariaDB
    /// (the documented default) rather than failing — dialect selection is not a
    /// schema error.
    pub fn parse(s: &str) -> Dialect {
        match s {
            "mariadb" | "mysql" => Dialect::MariaDb,
            _ => Dialect::MariaDb,
        }
    }
}
