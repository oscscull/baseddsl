use crate::command::{self, Environment};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use sqlx::{Connection, SqliteConnection};
use std::{fs, path::Path};

pub async fn sqlite(based: &Path) -> Result<()> {
    let based = based.canonicalize()?;
    let scratch = tempfile::tempdir()?;
    let app = scratch.path().join("app");
    project(&app, "sqlite")?;
    let path = app.join("original.db");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete);
    let mut db = SqliteConnection::connect_with(&options).await?;
    sqlx::raw_sql(include_str!(
        "../../crates/based-cli/tests/import_support/fixture.sql"
    ))
    .execute(&mut db)
    .await?;
    db.close().await?;
    let before = fs::read(&path)?;
    let env = Environment::from([
        ("DATABASE_URL".into(), path.display().to_string()),
        ("BASED_DATABASE_URL".into(), path.display().to_string()),
    ]);
    verify(&based, &app, "sqlite", "main", &env)?;
    ensure!(
        fs::read(path)? == before,
        "import/typed read changed original database"
    );
    Ok(())
}

pub fn server(based: &Path, app: &Path, dialect: &str) -> Result<()> {
    ensure!(
        ["mariadb", "postgres"].contains(&dialect),
        "unknown server dialect"
    );
    let source = std::env::var(format!("TEST_{}_URL", dialect.to_uppercase()))?;
    let env = Environment::from([
        (
            "BASED_DATABASE_URL".into(),
            connection(&source, dialect, "metadata", "fixture_metadata_only")?,
        ),
        (
            "DATABASE_URL".into(),
            connection(&source, dialect, "consumer", "fixture_select_only")?,
        ),
    ]);
    verify(
        &based.canonicalize()?,
        app,
        dialect,
        "based_import_fixture",
        &env,
    )
}

fn connection(source: &str, dialect: &str, role: &str, password: &str) -> Result<String> {
    // Preserve the supplied TLS query byte-for-byte; URL normalization can change CA paths.
    let (scheme, rest) = source
        .split_once("://")
        .ok_or_else(|| anyhow::anyhow!("invalid fixture URL"))?;
    let boundary = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = rest[..boundary].rsplit('@').next().unwrap();
    let mut suffix = rest[boundary..].to_string();
    if dialect == "mariadb" {
        let query = suffix
            .find(['?', '#'])
            .map(|index| &suffix[index..])
            .unwrap_or("");
        suffix = format!("/based_import_fixture{query}");
    }
    Ok(format!(
        "{scheme}://based_import_{role}:{password}@{authority}{suffix}"
    ))
}

fn project(app: &Path, dialect: &str) -> Result<()> {
    fs::create_dir_all(app.join("models"))?;
    fs::write(app.join("based.toml"), format!("dialect='{dialect}'\nroot='models'\n[generate]\nclient='src/based_client.rs'\nclient_mode='embedded'\n"))?;
    Ok(())
}

fn verify(
    based: &Path,
    app: &Path,
    dialect: &str,
    namespace: &str,
    environment: &Environment,
) -> Result<()> {
    let first = format!("{namespace}.legacy_account");
    let second = format!("{namespace}.legacy_entry");
    let result = command::run(
        based,
        &["import", "--table", &first, "--table", &second, "--json"],
        app,
        environment,
    )?;
    let report: Value = serde_json::from_str(&result)?;
    ensure!(
        report["status"] == "imported"
            && report["written"]
                .as_array()
                .is_some_and(|files| files.len() == 2),
        "import report: {report}"
    );
    ensure!(
        !app.join("migrations").exists(),
        "import adopted migrations"
    );
    fs::create_dir_all(app.join("src"))?;
    let fixture = command::root().join("ci/fixtures/import-consumer");
    fs::copy(fixture.join("reads.bsl"), app.join("models/reads.bsl"))?;
    fs::copy(fixture.join("main.rs"), app.join("src/main.rs"))?;
    fs::copy(
        fixture.join(format!("{dialect}.rs")),
        app.join("src/database.rs"),
    )?;
    let commit = command::git(&["rev-parse", "HEAD"])?;
    fs::write(app.join("Cargo.toml"), manifest(dialect, &commit))?;
    command::run(based, &["check"], app, environment)?;
    command::run(based, &["gen", "client", "--embedded"], app, environment)?;
    command::run(command::cargo(), &["fmt"], app, environment)?;
    let scratch = tempfile::tempdir()?;
    let mut env = crate::source::mirror(scratch.path())?;
    env.extend(environment.clone());
    let output = command::run(command::cargo(), &["run"], app, &env)?;
    let line = output
        .lines()
        .find_map(|line| line.strip_prefix("imported read: "))
        .ok_or_else(|| anyhow::anyhow!("missing typed read"))?;
    let rows: Value = serde_json::from_str(line)?;
    ensure!(
        rows == json!([{"heading":"retained original row", "parent_code":{"label":"retained account"}}]),
        "typed read changed fixture: {rows}"
    );
    command::run(
        command::cargo(),
        &["clippy", "--", "-D", "warnings"],
        app,
        &env,
    )?;
    ensure!(
        fs::read_to_string(app.join("Cargo.lock"))?.contains(&format!("#{commit}")),
        "consumer not pinned to exact Git source"
    );
    ensure!(
        !app.join("build.rs").exists() && !app.join("migrations").exists(),
        "import acquired generation/migration lifecycle"
    );
    Ok(())
}

fn manifest(dialect: &str, commit: &str) -> String {
    let driver = if dialect == "mariadb" {
        "mysql"
    } else {
        dialect
    };
    let tls = if dialect == "sqlite" {
        ""
    } else {
        ", \"tls-rustls\""
    };
    let sqlx_tls = if dialect == "sqlite" {
        ""
    } else {
        ", \"tls-rustls-ring-webpki\""
    };
    format!(
        r#"[package]
name = "based-import-consumer"
version = "0.1.0"
edition = "2021"
rust-version = "1.94"
[workspace]
[dependencies]
based-runtime = {{ git = "https://github.com/oscscull/baseddsl.git", rev = "{commit}", features = ["{dialect}", "id-gen"{tls}] }}
sqlx = {{ version = "0.9", default-features = false, features = ["runtime-tokio", "{driver}"{sqlx_tls}] }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
"#
    )
}
