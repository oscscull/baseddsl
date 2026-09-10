/// The one per-driver bring-your-own transaction constructor, for the compile-target
/// dialect. Each names that driver's concrete `sqlx::Transaction<DB>` (the types don't
/// unify across drivers, so there is one per driver) and is `#[cfg]`-gated on the
/// consumer's matching driver feature — a consumer forwards it to `based-runtime/<driver>`
/// (e.g. `postgres = ["based-runtime/postgres"]`), which is what makes
/// `based_runtime::AdoptedPg` and `based_runtime::sqlx` present. The MySQL/MariaDB family
/// shares one driver (`sqlx::MySql`), so both dialects emit `adopt_mariadb`.
pub(crate) fn adopt_constructor(dialect: crate::Dialect) -> String {
    use crate::Dialect::{MariaDb, MySql, Postgres, Sqlite};
    let (feature, suffix, sqlx_db, adapter, driver_name) = match dialect {
        Postgres => ("postgres", "postgres", "Postgres", "AdoptedPg", "Postgres"),
        Sqlite => ("sqlite", "sqlite", "Sqlite", "AdoptedSqlite", "SQLite"),
        MariaDb | MySql => (
            "mariadb",
            "mariadb",
            "MySql",
            "AdoptedMaria",
            "MariaDB/MySQL",
        ),
    };
    format!(
        r#"
/// Adopt a **caller-owned** open {driver_name} transaction — the bring-your-own (`adopt`)
/// rung of the transaction seam. Route this client's callables (including `for update`
/// locking reads — the adopted client is `TxBound`) through a transaction the caller already
/// opened on its own `sqlx` pool, so the baseddsl work commits atomically with the caller's
/// own raw writes on that transaction.
///
/// **`adopt` never begins, commits, or rolls back** — the caller owns the boundary; dropping
/// the returned client leaves the transaction untouched, and the caller commits (or rolls
/// back) it itself. There is no auto-retry (`transaction_retrying` needs an engine-owned
/// boundary). Feature-gated so a build without the {driver_name} driver simply omits it;
/// forward the feature to `based-runtime/{feature}`.
#[cfg(feature = "{feature}")]
pub fn adopt_{suffix}<'a>(
    engine: &based_runtime::Engine,
    tx: &'a mut based_runtime::sqlx::Transaction<'_, based_runtime::sqlx::{sqlx_db}>,
) -> Client<based_runtime::AdoptedTransport<based_runtime::{adapter}<'a>>> {{
    Client {{
        transport: based_runtime::AdoptedTransport::new(
            engine.clone(),
            based_runtime::{adapter}::new(tx),
        ),
    }}
}}
"#
    )
}

/// The bring-your-own transaction (`adopt`) `Transport` impl over
/// `based_runtime::AdoptedTransport<D>` — the same generated client, run on a
/// **caller-owned** transaction the caller adopted from its own driver. Mirrors the
/// `TxTransport` impl (it runs on the held transaction via `dispatch`, committing
/// nothing); the difference is who owns the boundary — here the caller does, so `adopt`
/// never begins/commits/rolls back. Generic over the driver adapter `D: DbRead`, so one
/// impl covers every driver. Keyed/stream doors splice in exactly as the other
/// transports' do.
pub(crate) const ADOPTED_TRANSPORT_HEAD: &str = r#"
// ---------- transactions: bring-your-own (`adopt`) ----------

/// A `Transport` bound to a **caller-owned** transaction adopted from the caller's own
/// driver: every callable runs on that transaction (committing nothing — the caller owns the
/// boundary). Generic over the per-driver adapter `D`, so one impl serves every driver.
impl<D: based_runtime::DbRead> Transport for based_runtime::AdoptedTransport<D> {
    async fn call<I, C, O>(&self, route: &str, input: &I, ctx: &C) -> Result<O, ClientError>
    where
        I: Serialize + Sync,
        C: Serialize + Sync,
        O: serde::de::DeserializeOwned,
    {
        let args = serde_json::to_value(input).map_err(ClientError::decode)?;
        // `&()` → JSON `null`; the engine treats a non-object context as empty.
        let ctx = serde_json::to_value(ctx)
            .map(|v| if v.is_object() { v } else { serde_json::json!({}) })
            .map_err(ClientError::decode)?;
        let resp = self.dispatch(route, args, ctx).await;
        if resp.status == 200 {
            serde_json::from_value(resp.body).map_err(ClientError::decode)
        } else {
            let code = resp.body["error"]["code"].as_str().unwrap_or("error");
            let message = resp.body["error"]["message"].as_str().unwrap_or("call failed");
            Err(ClientError::api(resp.status, code, message))
        }
    }
"#;

/// The adopted transport's keyed door (schema with a mutation): a keyed mutation on an
/// adopted transaction runs on it directly — the caller's transaction is the atomicity
/// boundary, so the idempotency key is not separately consulted.
pub(crate) const ADOPTED_TRANSPORT_KEYED_CALL: &str = r#"
    /// A keyed mutation on an adopted transaction: the caller's transaction is the boundary,
    /// so the write runs on it directly and the key is not separately consulted.
    async fn call_with_key<I, C, O>(
        &self,
        route: &str,
        input: &I,
        ctx: &C,
        _key: &str,
    ) -> Result<O, ClientError>
    where
        I: Serialize + Sync,
        C: Serialize + Sync,
        O: serde::de::DeserializeOwned,
    {
        self.call(route, input, ctx).await
    }
"#;

/// The adopted transport's streaming door (schema with a `-> stream` query): rejected —
/// a stream borrows the connection for its whole life, which the adopted transaction
/// holds — rather than silently run outside the transaction.
pub(crate) const ADOPTED_TRANSPORT_STREAM_CALL: &str = r#"
    /// Streaming on an adopted transaction is not supported (a stream holds the connection
    /// for its whole life, which the transaction owns): rejected loudly.
    async fn call_stream<I, C, O>(
        &self,
        _route: &str,
        _input: &I,
        _ctx: &C,
    ) -> Result<RowStream<O>, ClientError>
    where
        I: Serialize + Sync,
        C: Serialize + Sync,
        O: serde::de::DeserializeOwned + Send + 'static,
    {
        Err(ClientError::api(
            500,
            "unsupported",
            "streaming on an adopted transaction is not supported",
        ))
    }
"#;

/// Closes the adopted `Transport` impl.
pub(crate) const ADOPTED_TRANSPORT_TAIL: &str = "}\n";

/// An adopted client carries `TxBound` too, so `for update` locking reads work through a
/// caller-owned transaction (the lock is held to the caller's boundary). Emitted with the
/// locking surface (only when the schema has a `for update` query).
pub(crate) const ADOPTED_TXBOUND_IMPL: &str = r#"
impl<D: based_runtime::DbRead> TxBound for based_runtime::AdoptedTransport<D> {}
"#;
