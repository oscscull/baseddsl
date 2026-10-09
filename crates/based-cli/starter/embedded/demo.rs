//! Demonstrate creating related items and reading the host-owned projection.
use crate::{client, lookup, session::Session};

pub async fn run(
    engine: &based_runtime::Engine,
    session: Session,
) -> Result<(), Box<dyn std::error::Error>> {
    let api = client::embedded(engine);
    let created = create_pair(&api, &session).await?;
    let rows = api
        .items(
            client::ItemsInput {},
            client::ItemsCtx {
                owner: session.owner(),
            },
        )
        .await?;
    println!("created: {}", serde_json::to_string(&created)?);
    println!("read: {}", serde_json::to_string(&rows)?);
    lookup::show(&api, created.id, Session::other_demo()).await?;
    Ok(())
}

async fn create_pair(
    api: &client::Client<client::Embedded<'_>>,
    session: &Session,
) -> Result<client::ItemView, client::ClientError> {
    let parent = api
        .create_item(
            client::CreateItemInput {
                name: "Parent".into(),
                parent: None,
            },
            client::CreateItemCtx {
                owner: session.owner(),
            },
        )
        .await?;
    let created = api
        .create_item(
            client::CreateItemInput {
                name: "Hello Based".into(),
                parent: Some(parent.id),
            },
            client::CreateItemCtx {
                owner: session.owner(),
            },
        )
        .await?;
    Ok(created)
}
