//! Describe local configuration without creating or embedding credentials.
use super::options::Dialect;

pub fn example(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Sqlite => "# Local learning database; no credentials. CLI commands read .env if you copy this.\nDATABASE_URL=local.db\n",
        Dialect::Mariadb => "# Supply your own existing database. Do not commit credentials.\n# Export DATABASE_URL for both the CLI and consumer/demo.\n# DATABASE_URL=mysql://USER:PASSWORD@HOST:3306/DATABASE\n",
        Dialect::Postgres => "# Supply your own existing database. Do not commit credentials.\n# Export DATABASE_URL for both the CLI and consumer/demo.\n# DATABASE_URL=postgres://USER:PASSWORD@HOST:5432/DATABASE\n",
    }
}
