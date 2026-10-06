//! Two physical databases prove per-shard provisioning and real standalone driver wiring.
#![cfg(any(feature = "postgres", feature = "mariadb"))]
#[allow(dead_code)]
#[path = "support/server.rs"]
mod server;
// The common fixture exposes extra helpers used by the SQLite suite.
#[allow(dead_code)]
#[path = "support/project.rs"]
mod support;

use based_codegen::{sql, Dialect};
use based_runtime::shard::PoolConfig;
use based_runtime::{Backend, Compiled};
use server::{post_on_shard, Server};
use support::Project;

fn backend(url: &str, dialect: Dialect) -> Box<dyn Backend> {
    let pool = PoolConfig {
        min: 0,
        max: 2,
        ..PoolConfig::default()
    };
    match dialect {
        #[cfg(feature = "postgres")]
        Dialect::Postgres => Box::new(based_runtime::PgRouter::single(url, pool).unwrap()),
        #[cfg(feature = "mariadb")]
        Dialect::MariaDb => {
            Box::new(based_runtime::driver::ShardRouter::single(url, pool).unwrap())
        }
        _ => unreachable!("test only exercises enabled server drivers"),
    }
}

fn database_url(admin: &str, database: &str) -> String {
    let (base, query) = admin.split_once('?').unwrap_or((admin, ""));
    let prefix = base.rsplit_once('/').unwrap().0;
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{query}")
    };
    format!("{prefix}/{database}{suffix}")
}

fn project(dialect: &str) -> Project {
    let project = Project::new();
    project.write(
        "based.toml",
        &format!("dialect = \"{dialect}\"\nroot = \"schema\"\n"),
    );
    project.write(
        "schema/item.bsl",
        r#"
        Item { id: Id name: text owner: text }
        mutation make_item(name) -> Item { create Item { name = $name owner = $ctx.owner }; }
        query items() -> Item[] { list Item; }
    "#,
    );
    project
}

async fn prepare_databases(
    admin: &dyn Backend,
    urls: &[String],
    names: &[String],
    project: &Project,
    dialect: Dialect,
) {
    let compiled = Compiled::load(&project.0).unwrap();
    let mut db = admin.checkout("").await.unwrap();
    for name in names {
        db.execute(&format!("CREATE DATABASE {}", dialect.quote(name)), &[])
            .await
            .unwrap();
    }
    for url in urls {
        let backend = backend(url, dialect);
        let mut db = backend.checkout("").await.unwrap();
        db.execute(&sql::ddl(&compiled.schema, dialect), &[])
            .await
            .unwrap();
    }
}

fn check_http(project: &Project, urls: &[String]) {
    let options = [
        "--database-url",
        urls[0].as_str(),
        "--database-url",
        urls[1].as_str(),
        "--pool-min",
        "0",
    ];
    let mut initialize = options.to_vec();
    initialize.push("--init-idempotency-table");
    let first = Server::start(project, &initialize);
    // Startup checks both physical databases; this fails if only shard zero was initialized.
    let second = Server::start(project, &options);
    let shard = (0..100)
        .map(|n| format!("shard-{n}"))
        .find(|key| based_runtime::shard::fnv1a_64(key.as_bytes()) % 2 == 1)
        .unwrap();
    let body = r#"{"name":"alpha"}"#;
    let ctx = r#"{"owner":"a"}"#;
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            post_on_shard(
                first.address,
                "/m/make_item",
                body,
                ctx,
                Some("live-key"),
                Some(&shard),
            )
        });
        let b = scope.spawn(|| {
            post_on_shard(
                second.address,
                "/m/make_item",
                body,
                ctx,
                Some("live-key"),
                Some(&shard),
            )
        });
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_eq!(a.0, 200, "{a:?}");
    assert_eq!(b, a);
    drop(first);
    drop(second);
    let restarted = Server::start(project, &options);
    assert_eq!(
        post_on_shard(
            restarted.address,
            "/m/make_item",
            body,
            ctx,
            Some("live-key"),
            Some(&shard)
        ),
        a
    );
    let rows = post_on_shard(
        restarted.address,
        "/q/items",
        "{}",
        "{}",
        None,
        Some(&shard),
    );
    assert_eq!(rows.0, 200);
    assert_eq!(rows.1.as_array().unwrap().len(), 1);
}

async fn live(env: &str, dialect: Dialect, manifest_dialect: &str) {
    let Ok(url) = std::env::var(env) else {
        eprintln!("skip {env}; run make ci-standalone-store against disposable databases");
        return;
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let names = [
        format!("based_store_{stamp}_a"),
        format!("based_store_{stamp}_b"),
    ];
    let urls = names.each_ref().map(|name| database_url(&url, name));
    let project = project(manifest_dialect);
    let admin = backend(&url, dialect);
    prepare_databases(admin.as_ref(), &urls, &names, &project, dialect).await;
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check_http(&project, &urls)));
    let mut db = admin.checkout("").await.unwrap();
    let force = match dialect {
        Dialect::Postgres => " WITH (FORCE)",
        _ => "",
    };
    for name in &names {
        db.execute(
            &format!("DROP DATABASE {}{force}", dialect.quote(name)),
            &[],
        )
        .await
        .unwrap();
    }
    outcome.unwrap();
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_initializes_every_shard_and_replays_over_http() {
    live("TEST_POSTGRES_URL", Dialect::Postgres, "postgres").await;
}

#[cfg(feature = "mariadb")]
#[tokio::test]
async fn mariadb_initializes_every_shard_and_replays_over_http() {
    live("TEST_MARIADB_URL", Dialect::MariaDb, "mariadb").await;
}
