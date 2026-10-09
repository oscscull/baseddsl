//! Handle the optional scoped lookup and ordinary client failures explicitly.
use crate::{client, session::Session};

pub async fn show(
    api: &client::Client<client::Embedded<'_>>,
    id: client::Id<client::entity::Item>,
    session: Session,
) -> Result<(), client::ClientError> {
    let result = api
        .item_by_id(
            client::ItemByIdInput { id },
            client::ItemByIdCtx {
                owner: session.owner(),
            },
        )
        .await;
    match result {
        Ok(Some(item)) => println!("lookup: {}", item.name),
        Ok(None) => println!("lookup: not found in this owner's context"),
        Err(error) => {
            eprintln!(
                "lookup failed: {:?} [{}] status {:?}",
                error.kind(),
                error.code(),
                error.status()
            );
            return Err(error);
        }
    }
    Ok(())
}
