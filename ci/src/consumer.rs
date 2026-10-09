use crate::command::{self, Environment};
use anyhow::{ensure, Result};
use std::{fs, path::Path};

pub fn verify(dialect: &str) -> Result<()> {
    verify_with(dialect, &command::based(), None, &Environment::new())
}

pub fn verify_with(
    dialect: &str,
    based: &Path,
    commit: Option<&str>,
    environment: &Environment,
) -> Result<()> {
    ensure!(
        ["sqlite", "mariadb", "postgres"].contains(&dialect),
        "unknown dialect"
    );
    let root = command::root();
    for example in [
        "sqlite-quickstart",
        "mariadb-quickstart",
        "postgres-quickstart",
        "axum-helpdesk",
    ] {
        crate::client::check(based, &root.join("examples").join(example))?;
    }
    let scratch = tempfile::tempdir()?;
    let app = scratch.path();
    fs::create_dir(app.join("src"))?;
    fs::create_dir(app.join("generated"))?;
    let fixture = root.join("ci/fixtures/generated-consumer");
    fs::copy(fixture.join("schema.bsl"), app.join("schema.bsl"))?;
    fs::copy(fixture.join("main.rs"), app.join("src/main.rs"))?;
    fs::write(app.join("based.toml"), format!("dialect = '{dialect}'\n"))?;
    fs::write(app.join("Cargo.toml"), manifest(dialect, commit))?;
    command::run(
        based,
        &["gen", "client", "--embedded", "-o", "generated/client.rs"],
        app,
        &Environment::new(),
    )?;
    let database = match dialect {
        "sqlite" => app.join("database.db").display().to_string(),
        _ => std::env::var(format!("TEST_{}_URL", dialect.to_uppercase()))?,
    };
    let mut env = Environment::from([
        (
            "CARGO_TARGET_DIR".into(),
            root.join("target/consumer-contract").display().to_string(),
        ),
        ("DATABASE_URL".into(), database),
    ]);
    env.extend(environment.clone());
    command::run(
        command::cargo(),
        &[
            "run",
            "--features",
            dialect,
            "--bin",
            "based-consumer-contract",
        ],
        app,
        &env,
    )?;
    if let Some(commit) = commit {
        ensure!(
            fs::read_to_string(app.join("Cargo.lock"))?.contains(&format!("#{commit}")),
            "consumer resolved a different source commit"
        );
    }
    fs::copy(fixture.join("wrong.rs"), app.join("src/wrong.rs"))?;
    let manifest = app.join("Cargo.toml");
    let contents =
        fs::read_to_string(&manifest)? + "\n[[bin]]\nname='wrong'\npath='src/wrong.rs'\n";
    fs::write(manifest, contents)?;
    let error = command::fail(
        command::cargo(),
        &[
            "check",
            "--locked",
            "--offline",
            "--features",
            dialect,
            "--bin",
            "wrong",
        ],
        app,
        &env,
    )?;
    ensure!(
        error.contains("mismatched types") && error.contains("Owner") && error.contains("Item"),
        "wrong entity IDs were not rejected: {error}"
    );
    Ok(())
}

fn manifest(dialect: &str, commit: Option<&str>) -> String {
    let runtime = command::root().join("crates/based-runtime");
    let codegen = command::root().join("crates/based-codegen");
    let driver = if dialect == "mariadb" {
        "mysql"
    } else {
        dialect
    };
    let dependency = |path: &Path| match commit {
        Some(commit) => {
            format!("git = \"https://github.com/oscscull/baseddsl.git\", rev = \"{commit}\"")
        }
        None => format!("path = {path:?}"),
    };
    let runtime = dependency(&runtime);
    let codegen = dependency(&codegen);
    format!(
        r#"[package]
name = "based-consumer-contract"
version = "0.0.0"
edition = "2021"
[workspace]
[features]
sqlite = []
mariadb = []
postgres = []
[dependencies]
based-runtime = {{ {runtime}, features = ["{dialect}", "id-gen"] }}
based-codegen = {{ {codegen}, default-features = false }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
bigdecimal = "0.4"
futures-core = "0.3"
futures-util = {{ version = "0.3", default-features = false, features = ["std"] }}
async-trait = "0.1.92"
sqlx = {{ version = "0.9", default-features = false, features = ["runtime-tokio", "{driver}"] }}
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
"#
    )
}
