#[allow(dead_code)]
mod client {
    include!("../generated/client.rs");
}

use based_runtime::id::UuidGen;
use based_runtime::{Compiled, Engine};
use futures_util::StreamExt;
use serde_json::json;

#[tokio::main]
async fn main() {
    let compiled = Compiled::load(std::path::Path::new(".")).unwrap();
    let backend = backend();
    prepare(&compiled, &backend).await;
    let engine = Engine::new(compiled, backend, UuidGen);
    let api = client::embedded(&engine);
    let (owner, first, second) = seed(&api).await;
    verify_reads(&api, owner, &first).await;
    verify_json_strings(&api).await;
    verify_lists(&api).await;
    verify_pages(&api).await;
    verify_stream(&api).await;
    verify_delete(&api, second).await;
}

fn payload() -> serde_json::Value {
    json!({"nested": [null, {"active": true}], "price": "19.90"})
}

async fn seed<T: client::Transport>(
    api: &client::Client<T>,
) -> (client::OwnerRow, client::ItemRow, client::ItemRow) {
    let owner = api
        .create_owner(client::CreateOwnerInput { name: "Ada".into() }, ())
        .await
        .unwrap();
    assert_eq!(owner.name, "Ada");
    assert!(owner.items.is_empty());
    let data = payload();
    let wide = "12345678901234567890123456789.123456789";
    let fraction = "0.00000000000000000000000000000000000001";
    let first = api
        .create_item(
            client::CreateItemInput {
                owner: Some(client::Id::from_raw(owner.id.as_str())),
                name: "A".into(),
                payload: Some(data.clone()),
                wide: wide.parse().unwrap(),
                fraction: fraction.parse().unwrap(),
            },
            (),
        )
        .await
        .unwrap();
    assert_eq!(first.payload, Some(data.clone()));
    assert_eq!(first.wide.to_string(), wide);
    assert_eq!(first.fraction.to_string(), fraction);
    let second = api
        .create_item(
            client::CreateItemInput {
                owner: None,
                name: "B".into(),
                payload: None,
                wide: format!("-{wide}").parse().unwrap(),
                fraction: format!("-{fraction}").parse().unwrap(),
            },
            (),
        )
        .await
        .unwrap();
    assert!(second.payload.is_none());
    assert_eq!(second.wide.to_string(), format!("-{wide}"));
    assert_eq!(second.fraction.to_string(), format!("-{fraction}"));
    (owner, first, second)
}

async fn verify_reads<T: client::Transport>(
    api: &client::Client<T>,
    owner: client::OwnerRow,
    first: &client::ItemRow,
) {
    let data = payload();
    let read = api
        .item_by_id(
            client::ItemByIdInput {
                id: first.id.clone(),
            },
            (),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.payload, Some(data.clone()));
    assert!(api
        .item_by_id(
            client::ItemByIdInput {
                id: client::Id::from_raw("00000000-0000-4000-8000-000000000099")
            },
            ()
        )
        .await
        .unwrap()
        .is_none());
    let nested = api
        .owner_by_id(client::OwnerByIdInput { id: owner.id }, ())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(nested.items.len(), 1);
    assert_eq!(nested.items[0].payload, Some(data));
}

async fn verify_json_strings<T: client::Transport>(api: &client::Client<T>) {
    for text in ["true", "123", "eyJzZWNyZXQiOnRydWV9"] {
        let expected = json!(text);
        let item = api
            .create_item(
                client::CreateItemInput {
                    owner: None,
                    name: "Scalar".into(),
                    payload: Some(expected.clone()),
                    wide: "0".parse().unwrap(),
                    fraction: "0".parse().unwrap(),
                },
                (),
            )
            .await
            .unwrap();
        assert_eq!(item.payload, Some(expected.clone()));
        let read = api
            .item_by_id(
                client::ItemByIdInput {
                    id: item.id.clone(),
                },
                (),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read.payload, Some(expected));
        api.remove_item(client::RemoveItemInput { id: item.id }, ())
            .await
            .unwrap();
    }
}

async fn verify_lists<T: client::Transport>(api: &client::Client<T>) {
    for names in [vec![], vec!["B"], vec!["A", "B"]] {
        let rows = api
            .find_items(
                client::FindItemsInput {
                    names: names.iter().map(|s| (*s).into()).collect(),
                },
                (),
            )
            .await
            .unwrap();
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            names
        );
    }
}

async fn verify_pages<T: client::Transport>(api: &client::Client<T>) {
    let page = api
        .item_page(client::ItemPageInput { offset: Some(0) }, ())
        .await
        .unwrap();
    assert_eq!(page.total, Some(2));
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].name, "A");
    let page = api
        .item_page(client::ItemPageInput { offset: Some(1) }, ())
        .await
        .unwrap();
    assert_eq!(page.rows[0].name, "B");
}

async fn verify_stream<T: client::Transport>(api: &client::Client<T>) {
    let mut stream = api
        .export_items(client::ExportItemsInput {}, ())
        .await
        .unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap().name, "A");
    assert_eq!(stream.next().await.unwrap().unwrap().name, "B");
    assert!(stream.next().await.is_none());
    drop(stream);
}

async fn verify_delete<T: client::Transport>(api: &client::Client<T>, second: client::ItemRow) {
    api.remove_item(
        client::RemoveItemInput {
            id: second.id.clone(),
        },
        (),
    )
    .await
    .unwrap();
    assert!(api
        .item_by_id(client::ItemByIdInput { id: second.id }, ())
        .await
        .unwrap()
        .is_none());
}

#[cfg(feature = "sqlite")]
fn backend() -> based_runtime::SqliteBackend {
    based_runtime::SqliteBackend::open(&std::env::var("DATABASE_URL").unwrap()).unwrap()
}
#[cfg(feature = "mariadb")]
fn backend() -> based_runtime::driver::ShardRouter {
    based_runtime::driver::ShardRouter::single(
        &std::env::var("DATABASE_URL").unwrap(),
        Default::default(),
    )
    .unwrap()
}
#[cfg(feature = "postgres")]
fn backend() -> based_runtime::PgRouter {
    based_runtime::PgRouter::single(&std::env::var("DATABASE_URL").unwrap(), Default::default())
        .unwrap()
}

async fn prepare(compiled: &Compiled, backend: &dyn based_runtime::Backend) {
    let mut db = backend.checkout("").await.unwrap();
    for name in ["item", "owner"] {
        db.execute(
            &format!("DROP TABLE IF EXISTS {}", compiled.dialect.quote(name)),
            &[],
        )
        .await
        .unwrap();
    }
    let ddl = based_codegen::sql::ddl(&compiled.schema, compiled.dialect);
    for statement in ddl.split(';').filter(|s| !s.trim().is_empty()) {
        db.execute(statement, &[]).await.unwrap();
    }
}
