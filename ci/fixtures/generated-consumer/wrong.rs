#[allow(dead_code)]
mod client {
    include!("../generated/client.rs");
}
fn main() {
    let owner: client::Id<client::entity::Owner> = client::Id::from_raw("owner");
    let _input = client::ItemByIdInput { id: owner };
}
