//! Seeding test data with the generated fixture builders: typed rows, no raw SQL.

use based_runtime::id::UuidGen;
use based_runtime::{apply_migrations, load_migrations, ApplyOpts, Compiled, Engine};
use based_runtime::{SqliteBackend, TxOptions};
use std::path::Path;

#[allow(dead_code)]
mod client {
    include!(concat!(env!("OUT_DIR"), "/client.rs"));
}

use client::fixtures;

/// A fresh in-memory database at the latest migration.
async fn engine() -> Engine {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let compiled = Compiled::load(root).expect("schema loads");
    let backend = SqliteBackend::in_memory().expect("in-memory database");
    let migrations = load_migrations(root, compiled.dialect).expect("migrations load");
    apply_migrations(&backend, compiled.dialect, &migrations, &ApplyOpts::default())
        .await
        .expect("migrations apply");
    Engine::new(compiled, backend, UuidGen)
}

#[tokio::test]
async fn fixtures_seed_typed_rows() -> Result<(), Box<dyn std::error::Error>> {
    let engine = engine().await;
    let api = client::embedded(&engine);

    // Required columns get placeholders; set only what the test cares about.
    let acme = fixtures::org().name("Acme").insert(&api).await?;
    assert_eq!(acme.name, "Acme");
    assert!(acme.slug.starts_with("slug-"), "{}", acme.slug);
    let buyer = fixtures::user().insert(&api).await?;

    // Required relations are arguments: a row or an id names the parent.
    let first = fixtures::order(&acme, buyer.id.clone())
        .insert(&api)
        .await?;
    assert_eq!(first.org, acme.id);
    assert_eq!(first.status, "pending", "unset columns keep their default");

    // Fixtures run inside a transaction too.
    let (org, user) = (acme.clone(), buyer.clone());
    client::transaction(&engine, TxOptions::new(), |tx| async move {
        fixtures::order(&org, &user)
            .status("paid")
            .insert(&tx)
            .await?;
        Ok(())
    })
    .await?;

    // The app's own queries see the seeded rows.
    let mine = api
        .my_orders(
            client::MyOrdersInput,
            client::MyOrdersCtx {
                org: acme.id.clone(),
            },
        )
        .await?;
    assert_eq!(mine.len(), 2);
    Ok(())
}
