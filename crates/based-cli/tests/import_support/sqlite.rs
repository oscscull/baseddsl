//! Independently authored database and a real CLI subprocess, without migration application.
use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

pub struct Fixture {
    _directory: tempfile::TempDir,
    pub root: PathBuf,
    pub before: Vec<u8>,
}

impl Fixture {
    pub async fn create() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("models")).unwrap();
        std::fs::write(
            root.join("based.toml"),
            "dialect = 'sqlite'\nroot = 'models'\n[generate]\nclient = 'src/based_client.rs'\n",
        )
        .unwrap();
        std::fs::write(root.join(".env"), "BASED_DATABASE_URL=legacy.db\n").unwrap();
        let mut fixture = Self {
            _directory: directory,
            root,
            before: Vec::new(),
        };
        fixture.add_schema(include_str!("fixture.sql")).await;
        fixture
    }

    pub fn run(&self, cwd: &Path, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_based"))
            .current_dir(cwd)
            .args(arguments)
            .env_remove("BASED_DATABASE_URL")
            .env_remove("DATABASE_URL")
            .output()
            .unwrap()
    }

    pub async fn add_schema(&mut self, sql: &str) {
        let mut database = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(self.root.join("legacy.db"))
                .create_if_missing(true),
        )
        .await
        .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&mut database)
            .await
            .unwrap();
        database.close().await.unwrap();
        self.before = std::fs::read(self.root.join("legacy.db")).unwrap();
    }

    pub fn import(&self, extra: &[&str]) -> Output {
        let mut arguments = vec![
            "import",
            "--table",
            "main.legacy_account",
            "--table",
            "main.legacy_entry",
            "--json",
        ];
        arguments.extend_from_slice(extra);
        self.run(&self.root, &arguments)
    }

    pub fn unchanged(&self) {
        assert_eq!(
            self.before,
            std::fs::read(self.root.join("legacy.db")).unwrap()
        );
        assert!(!self.root.join("migrations").exists());
        assert!(!self.root.join("src/based_client.rs").exists());
    }
}

pub fn report(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error}: {output:?}"))
}
