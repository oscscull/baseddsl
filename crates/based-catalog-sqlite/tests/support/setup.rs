use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};
use std::path::PathBuf;

pub struct Fixture {
    pub path: PathBuf,
    pub before: Vec<u8>,
    _directory: tempfile::TempDir,
}

impl Fixture {
    pub async fn create() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.db");
        let mut admin = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(include_str!("../fixture.sql")))
            .execute(&mut admin)
            .await
            .unwrap();
        admin.close().await.unwrap();
        let before = std::fs::read(&path).unwrap();
        Self {
            path,
            before,
            _directory: directory,
        }
    }

    pub fn assert_unchanged(&self) {
        assert_eq!(self.before, std::fs::read(&self.path).unwrap());
        let files: Vec<_> = std::fs::read_dir(self.path.parent().unwrap())
            .unwrap()
            .collect();
        assert_eq!(files.len(), 1, "rollback-journal fixture created sidecars");
    }
}
