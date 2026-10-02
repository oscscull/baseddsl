//! A committed generated client exercises exact decimals through the real SQLite
//! engine. The fixture includes both 38-digit boundary shapes and a nullable field.

#![cfg(feature = "sqlite")]

#[allow(dead_code, clippy::use_self)]
#[path = "support/decimal_client.rs"]
mod client;

use based_codegen::{sql, Dialect};
use based_runtime::{Compiled, Engine, SeqIdGen, SqliteBackend};

#[cfg(feature = "docker-tests")]
#[path = "support/docker_mariadb.rs"]
mod docker_mariadb;
#[cfg(feature = "docker-tests")]
#[path = "support/docker_postgres.rs"]
mod docker_postgres;

#[tokio::test]
async fn generated_client_round_trips_decimal_domain_through_sqlite() {
    let root =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/decimal-client");
    let compiled = Compiled::load(&root).expect("decimal schema compiles");
    let backend = SqliteBackend::in_memory().expect("in-memory database");
    backend
        .execute_batch(&sql::ddl(&compiled.schema, Dialect::Sqlite))
        .await
        .expect("generated DDL executes");
    let engine = Engine::new(compiled, backend, SeqIdGen::default());
    round_trip(&client::embedded(&engine)).await;
}

async fn round_trip<T: client::Transport>(api: &client::Client<T>) {
    for (wide, fraction) in [
        (
            "12345678901234567890123456789.123456789",
            "0.00000000000000000000000000000000000001",
        ),
        (
            "-12345678901234567890123456789.123456789",
            "-0.00000000000000000000000000000000000001",
        ),
        ("0.000000000", "0.00000000000000000000000000000000000000"),
        ("19.900000000", "0.25000000000000000000000000000000000000"),
    ] {
        let created = api
            .create_measure(
                client::CreateMeasureInput {
                    wide: wide.parse().unwrap(),
                    fraction: fraction.parse().unwrap(),
                },
                (),
            )
            .await
            .expect("typed create");
        assert_eq!(created.wide.to_string(), wide);
        assert_eq!(created.fraction.to_string(), fraction);
        assert!(created.optional.is_none());

        let read = api
            .measure_by_id(client::MeasureByIdInput { id: created.id }, ())
            .await
            .expect("typed read")
            .expect("row exists");
        assert_eq!(read.wide.to_string(), wide);
        assert_eq!(read.fraction.to_string(), fraction);
        assert!(read.optional.is_none());
    }
}

#[cfg(feature = "docker-tests")]
fn compiled_for(dialect: Dialect) -> Compiled {
    use based_ast::FileId;
    use based_parser::parse_file;
    use based_sema::check;

    let source = include_str!("fixtures/decimal-client/schema/measure.bsl");
    let file = parse_file(source, FileId(0)).expect("decimal fixture parses");
    let (schema, diagnostics) = check(&file.decls);
    assert!(
        !diagnostics
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error),
        "decimal fixture checks: {diagnostics:?}"
    );
    Compiled::from_checked(schema, file.decls, dialect)
}

#[cfg(feature = "docker-tests")]
#[tokio::test]
async fn generated_client_round_trips_decimal_domain_through_mariadb() {
    use based_runtime::driver::{PoolConfig, ShardRouter};
    use based_runtime::id::UuidGen;

    let Some(server) = docker_mariadb::MariaDbContainer::start().await else {
        return;
    };
    let compiled = compiled_for(Dialect::MariaDb);
    server.exec_batch("DROP TABLE IF EXISTS `measure`;").await;
    server
        .exec_batch(&sql::ddl(&compiled.schema, Dialect::MariaDb))
        .await;
    let router = ShardRouter::single(&server.url(), PoolConfig::default()).unwrap();
    let engine = Engine::new(compiled, router, UuidGen);
    round_trip(&client::embedded(&engine)).await;
}

#[cfg(feature = "docker-tests")]
#[tokio::test]
async fn generated_client_round_trips_decimal_domain_through_postgres() {
    use based_runtime::id::UuidGen;
    use based_runtime::shard::PoolConfig;
    use based_runtime::PgRouter;

    let Some(server) = docker_postgres::PostgresContainer::start().await else {
        return;
    };
    let compiled = compiled_for(Dialect::Postgres);
    server.exec_batch("DROP TABLE IF EXISTS \"measure\";").await;
    server
        .exec_batch(&sql::ddl(&compiled.schema, Dialect::Postgres))
        .await;
    let router = PgRouter::single(&server.url(), PoolConfig::default()).unwrap();
    let engine = Engine::new(compiled, router, UuidGen);
    round_trip(&client::embedded(&engine)).await;
}
