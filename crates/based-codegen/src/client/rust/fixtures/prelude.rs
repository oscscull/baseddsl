/// Opens the fixtures module: the transport seam, the row-reference trait and the
/// private helpers every builder shares.
pub(super) const FIXTURES_HEAD: &str = r#"
// ---------- test fixtures ----------

/// Typed test fixtures: one builder per model. Required columns get placeholders,
/// required relations are constructor arguments, and everything else stays unset until a
/// setter fills it. `insert` writes the row through an engine-backed client, bypassing
/// scopes and guards, and returns it as stored.
pub mod fixtures {
    use super::*;

    /// A client transport that can insert fixtures: the in-process ones over an engine.
    #[allow(async_fn_in_trait)]
    pub trait FixtureTransport {
        async fn insert_fixture(
            &self,
            model: &str,
            row: serde_json::Value,
        ) -> based_runtime::WireResponse;
    }

    impl FixtureTransport for Embedded<'_> {
        async fn insert_fixture(
            &self,
            model: &str,
            row: serde_json::Value,
        ) -> based_runtime::WireResponse {
            self.engine.insert_fixture(model, row).await
        }
    }

    impl FixtureTransport for based_runtime::TxTransport {
        async fn insert_fixture(
            &self,
            model: &str,
            row: serde_json::Value,
        ) -> based_runtime::WireResponse {
            based_runtime::TxTransport::insert_fixture(self, model, row).await
        }
    }

    impl<D: based_runtime::DbRead> FixtureTransport for based_runtime::AdoptedTransport<D> {
        async fn insert_fixture(
            &self,
            model: &str,
            row: serde_json::Value,
        ) -> based_runtime::WireResponse {
            based_runtime::AdoptedTransport::insert_fixture(self, model, row).await
        }
    }

    /// Names one row of a model: its key, or a fixture row of that model.
    pub trait RowRef<K> {
        fn key(&self) -> K;
    }

    impl<E> RowRef<Id<E>> for Id<E> {
        fn key(&self) -> Id<E> {
            self.clone()
        }
    }

    impl<K, T: RowRef<K> + ?Sized> RowRef<K> for &T {
        fn key(&self) -> K {
            (**self).key()
        }
    }

    #[allow(dead_code)]
    mod support {
        use super::{Client, ClientError, FixtureTransport};
        use std::sync::atomic::{AtomicU64, Ordering};

        type Row = serde_json::Map<String, serde_json::Value>;

        static NEXT: AtomicU64 = AtomicU64::new(1);

        /// The next placeholder number: one per fixture, unique within this process.
        pub(super) fn next_n() -> u64 {
            NEXT.fetch_add(1, Ordering::Relaxed)
        }

        pub(super) fn put(row: &mut Row, field: &str, value: &impl serde::Serialize) {
            let value = serde_json::to_value(value).expect("fixture values serialize to JSON");
            row.insert(field.to_string(), value);
        }

        /// A relation to a single-column key: `{ "<key field>": key }`.
        pub(super) fn link(key_field: &str, key: &impl serde::Serialize) -> serde_json::Value {
            let mut object = Row::new();
            put(&mut object, key_field, key);
            serde_json::Value::Object(object)
        }

        /// A relation to a composite key, already an object of its parts.
        pub(super) fn link_parts(key: &impl serde::Serialize) -> serde_json::Value {
            serde_json::to_value(key).expect("fixture keys serialize to JSON")
        }

        const BASE_SECONDS: u64 = 946_684_800;

        /// 2000-01-01T00:00:00Z plus `n` seconds.
        pub(super) fn timestamp(n: u64) -> String {
            let seconds = BASE_SECONDS + n;
            let (y, m, d) = civil(seconds / 86_400);
            let s = seconds % 86_400;
            format!(
                "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
                s / 3600,
                s / 60 % 60,
                s % 60
            )
        }

        /// 2000-01-01 plus `n` days.
        pub(super) fn date(n: u64) -> String {
            let (y, m, d) = civil(BASE_SECONDS / 86_400 + n);
            format!("{y:04}-{m:02}-{d:02}")
        }

        /// Midnight plus `n` seconds, wrapping at a day.
        pub(super) fn time(n: u64) -> String {
            let s = n % 86_400;
            format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
        }

        pub(super) fn uuid(n: u64) -> String {
            format!("00000000-0000-4000-8000-{n:012x}")
        }

        pub(super) fn ulid(n: u64) -> String {
            format!("{n:026}")
        }

        /// The civil date `days` after 1970-01-01.
        fn civil(days: u64) -> (u64, u64, u64) {
            let z = days + 719_468;
            let era = z / 146_097;
            let doe = z % 146_097;
            let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let d = doy - (153 * mp + 2) / 5 + 1;
            let m = if mp < 10 { mp + 3 } else { mp - 9 };
            let y = yoe + era * 400 + u64::from(m <= 2);
            (y, m, d)
        }

        pub(super) async fn insert<T, R>(
            client: &Client<T>,
            model: &str,
            row: Row,
        ) -> Result<R, ClientError>
        where
            T: FixtureTransport,
            R: serde::de::DeserializeOwned,
        {
            let resp = client
                .transport
                .insert_fixture(model, serde_json::Value::Object(row))
                .await;
            if resp.status == 200 {
                serde_json::from_value(resp.body).map_err(ClientError::decode)
            } else {
                let code = resp.body["error"]["code"].as_str().unwrap_or("error");
                let message = resp.body["error"]["message"].as_str().unwrap_or("fixture insert failed");
                Err(ClientError::api(resp.status, code, message))
            }
        }
    }
"#;

/// Closes the fixtures module.
pub(super) const FIXTURES_TAIL: &str = "}\n";

/// A composite key names its own row.
pub(super) fn composite_self_ref(model: &str) -> String {
    format!(
        "\n    impl RowRef<{model}Id> for {model}Id {{\n        fn key(&self) -> {model}Id {{\n            self.clone()\n        }}\n    }}\n"
    )
}
