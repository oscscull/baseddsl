//! Workload definitions and the generated embedded client invocation.
use crate::{client, fixture, Error};
use based_runtime::Engine;
use serde::Serialize;

#[derive(Clone, Copy, Debug)]
pub enum Case {
    Flat(i64),
    Nested(i64),
    Page,
    Bulk(usize),
}

pub const CASES: [Case; 7] = [
    Case::Flat(8),
    Case::Flat(1024),
    Case::Nested(1),
    Case::Nested(128),
    Case::Page,
    Case::Bulk(8),
    Case::Bulk(512),
];

#[derive(Serialize)]
#[serde(untagged)]
pub enum Output {
    Items(Vec<client::ItemRow>),
    Owners(Vec<client::OwnerRow>),
    Page(client::Page<client::ItemRow>),
    Ack(()),
}

impl Case {
    pub fn request(self) -> (&'static str, serde_json::Value) {
        match self {
            Self::Flat(size) => ("/q/flat", serde_json::json!({"size": size})),
            Self::Nested(size) => ("/q/nested", serde_json::json!({"size": size})),
            Self::Page => ("/q/paged", serde_json::json!({"offset": 32})),
            Self::Bulk(size) => (
                "/m/bulk",
                serde_json::to_value(fixture::bulk(size)).unwrap(),
            ),
        }
    }

    pub fn is_write(self) -> bool {
        matches!(self, Self::Bulk(_))
    }

    pub async fn embedded(self, engine: &Engine) -> Result<Output, Error> {
        let api = client::embedded(engine);
        Ok(match self {
            Self::Flat(size) => Output::Items(api.flat(client::FlatInput { size }, ()).await?),
            Self::Nested(size) => {
                Output::Owners(api.nested(client::NestedInput { size }, ()).await?)
            }
            Self::Page => Output::Page(
                api.paged(client::PagedInput { offset: Some(32) }, ())
                    .await?,
            ),
            Self::Bulk(size) => {
                api.bulk(fixture::bulk(size), ()).await?;
                Output::Ack(())
            }
        })
    }
}
