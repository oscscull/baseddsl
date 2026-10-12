/// Opens the fixtures module: the insert seam, row references, the placeholder counter
/// and the private helpers every builder shares.
pub(super) const FIXTURES_HEAD: &str = r#"
// ---------- test fixtures ----------

/// Typed test fixtures: one builder per model. Required columns get placeholders,
/// required relations are constructor arguments, and everything else stays unset until a
/// setter fills it. `insert` writes the row, bypassing scopes and guards, and returns it
/// as stored.
pub mod fixtures {
    use super::*;

    /// Something a fixture row can be inserted through: an engine-backed client, or a
    /// [`Connection`] to a `based fixtures` process.
    #[allow(async_fn_in_trait)]
    pub trait FixtureTransport {
        async fn insert_fixture(
            &self,
            model: &str,
            row: serde_json::Value,
            shard: &str,
        ) -> Result<serde_json::Value, ClientError>;
    }

    impl<T: FixtureTransport> FixtureTransport for Client<T> {
        async fn insert_fixture(
            &self,
            model: &str,
            row: serde_json::Value,
            shard: &str,
        ) -> Result<serde_json::Value, ClientError> {
            self.transport.insert_fixture(model, row, shard).await
        }
    }

    /// Names one row of a model: its key, or a fixture row of that model.
    pub trait RowRef<K> {
        fn key(&self) -> K;
    }

    impl<E> RowRef<Id<E>> for Id<E> {
        fn key(&self) -> Self {
            self.clone()
        }
    }

    impl<K, T: RowRef<K> + ?Sized> RowRef<K> for &T {
        fn key(&self) -> K {
            (**self).key()
        }
    }

    /// The placeholder numbers: a sequence starting at a random offset, so separate
    /// processes sharing a database never collide on unique columns.
    pub struct Counter {
        base: u64,
        taken: std::sync::atomic::AtomicU64,
    }

    impl Counter {
        pub fn new() -> Self {
            use std::hash::{BuildHasher, Hasher};
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u32(std::process::id());
            if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                hasher.write_u128(now.as_nanos());
            }
            // Below 2^52, so the numbers survive a JSON round trip exactly.
            Self {
                base: (hasher.finish() >> 32) << 20,
                taken: std::sync::atomic::AtomicU64::new(0),
            }
        }

        /// The next number in the sequence.
        pub fn take(&self) -> u64 {
            self.base + self.taken.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        }
    }

    impl Default for Counter {
        fn default() -> Self {
            Self::new()
        }
    }

    #[allow(dead_code)]
    mod support {
        use super::{ClientError, Counter, FixtureTransport};

        type Row = serde_json::Map<String, serde_json::Value>;

        static COUNTER: std::sync::OnceLock<Counter> = std::sync::OnceLock::new();

        /// The next placeholder number: one per fixture.
        pub(super) fn next_n() -> u64 {
            COUNTER.get_or_init(Counter::new).take()
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

        /// 2000-01-01T00:00:00Z plus up to about 31 years of seconds.
        pub(super) fn timestamp(n: u64) -> String {
            let seconds = BASE_SECONDS + n % 1_000_000_000;
            let (y, m, d) = civil(seconds / 86_400);
            let s = seconds % 86_400;
            format!(
                "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
                s / 3600,
                s / 60 % 60,
                s % 60
            )
        }

        /// 2000-01-01 plus up to 100 years of days.
        pub(super) fn date(n: u64) -> String {
            let (y, m, d) = civil(BASE_SECONDS / 86_400 + n % 36_500);
            format!("{y:04}-{m:02}-{d:02}")
        }

        /// A time of day.
        pub(super) fn time(n: u64) -> String {
            let s = n % 86_400;
            format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
        }

        pub(super) fn uuid(n: u64) -> String {
            format!(
                "{:08x}-0000-4000-8000-{:012x}",
                n >> 48,
                n & 0xffff_ffff_ffff
            )
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

        pub(super) async fn insert<R: serde::de::DeserializeOwned>(
            to: &impl FixtureTransport,
            model: &str,
            row: Row,
            shard: &str,
        ) -> Result<R, ClientError> {
            let stored = to
                .insert_fixture(model, serde_json::Value::Object(row), shard)
                .await?;
            serde_json::from_value(stored).map_err(ClientError::decode)
        }

        /// A wire-shaped reply (`status` + body) as a stored row or a client error.
        pub(super) fn reply(status: u16, body: serde_json::Value) -> Result<serde_json::Value, ClientError> {
            if status == 200 {
                return Ok(body);
            }
            let code = body["error"]["code"].as_str().unwrap_or("error");
            let message = body["error"]["message"].as_str().unwrap_or("fixture insert failed");
            Err(ClientError::api(status, code, message))
        }
    }

    /// Where a `based fixtures` process runs; [`connect`] starts it.
    #[derive(Debug, Clone)]
    pub struct Connect {
        database_url: String,
        cwd: Option<std::path::PathBuf>,
        binary: Option<std::path::PathBuf>,
    }

    impl Connect {
        /// Insert into the database at `database_url`.
        pub fn new(database_url: impl Into<String>) -> Self {
            Self {
                database_url: database_url.into(),
                cwd: None,
                binary: None,
            }
        }

        /// The project directory holding `based.toml` (default: the current directory).
        #[must_use]
        pub fn cwd(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
            self.cwd = Some(dir.into());
            self
        }

        /// The `based` executable (default: `BASED_BIN`, then `based` on the PATH).
        #[must_use]
        pub fn binary(mut self, path: impl Into<std::path::PathBuf>) -> Self {
            self.binary = Some(path.into());
            self
        }
    }

    /// Start a `based fixtures` process. It reads requests on stdin and opens no socket;
    /// dropping the [`Connection`] ends it.
    pub fn connect(options: Connect) -> Result<Connection, ClientError> {
        let binary = options
            .binary
            .or_else(|| std::env::var_os("BASED_BIN").map(std::path::PathBuf::from))
            .unwrap_or_else(|| std::path::PathBuf::from("based"));
        let mut command = std::process::Command::new(binary);
        command
            .args(["fixtures", "--database-url", &options.database_url])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());
        if let Some(cwd) = &options.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn().map_err(ClientError::transport)?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().map(std::io::BufReader::new);
        match (stdin, stdout) {
            (Some(stdin), Some(stdout)) => Ok(Connection {
                io: std::sync::Mutex::new(ProcessIo {
                    child,
                    stdin: Some(stdin),
                    stdout,
                }),
            }),
            _ => Err(ClientError::transport("based fixtures has no stdio pipes")),
        }
    }

    /// A running `based fixtures` process; inserts are answered in order.
    pub struct Connection {
        io: std::sync::Mutex<ProcessIo>,
    }

    struct ProcessIo {
        child: std::process::Child,
        stdin: Option<std::process::ChildStdin>,
        stdout: std::io::BufReader<std::process::ChildStdout>,
    }

    impl FixtureTransport for Connection {
        async fn insert_fixture(
            &self,
            model: &str,
            row: serde_json::Value,
            shard: &str,
        ) -> Result<serde_json::Value, ClientError> {
            let request = serde_json::json!({ "model": model, "row": row, "shard": shard });
            let mut io = self
                .io
                .lock()
                .map_err(|_| ClientError::transport("the fixtures connection is poisoned"))?;
            let io = &mut *io;
            let stdin = io
                .stdin
                .as_mut()
                .ok_or_else(|| ClientError::transport("the fixtures connection is closed"))?;
            std::io::Write::write_all(stdin, format!("{request}\n").as_bytes())
                .and_then(|()| std::io::Write::flush(stdin))
                .map_err(ClientError::transport)?;
            let mut line = String::new();
            let read = std::io::BufRead::read_line(&mut io.stdout, &mut line)
                .map_err(ClientError::transport)?;
            if read == 0 {
                return Err(ClientError::transport("based fixtures exited"));
            }
            let reply: serde_json::Value = serde_json::from_str(&line).map_err(ClientError::decode)?;
            match reply.get("row") {
                Some(row) => Ok(row.clone()),
                None => {
                    let status = reply["status"].as_u64().and_then(|s| u16::try_from(s).ok());
                    support::reply(status.unwrap_or(400), reply)
                }
            }
        }
    }

    impl Drop for Connection {
        fn drop(&mut self) {
            if let Ok(io) = self.io.get_mut() {
                drop(io.stdin.take());
                let _ = io.child.wait();
            }
        }
    }
"#;

/// The engine-backed transports, emitted with the embedded bridge.
pub(super) const FIXTURES_EMBEDDED: &str = r#"
    impl FixtureTransport for Embedded<'_> {
        async fn insert_fixture(
            &self,
            model: &str,
            row: serde_json::Value,
            shard: &str,
        ) -> Result<serde_json::Value, ClientError> {
            let resp = self.engine.insert_fixture(model, row, shard).await;
            support::reply(resp.status, resp.body)
        }
    }

    /// A transaction's connection is already chosen, so the shard key does not apply.
    impl FixtureTransport for based_runtime::TxTransport {
        async fn insert_fixture(
            &self,
            model: &str,
            row: serde_json::Value,
            _shard: &str,
        ) -> Result<serde_json::Value, ClientError> {
            let resp = based_runtime::TxTransport::insert_fixture(self, model, row).await;
            support::reply(resp.status, resp.body)
        }
    }

    /// An adopted transaction's connection is already chosen, so the shard key does not
    /// apply.
    impl<D: based_runtime::DbRead> FixtureTransport for based_runtime::AdoptedTransport<D> {
        async fn insert_fixture(
            &self,
            model: &str,
            row: serde_json::Value,
            _shard: &str,
        ) -> Result<serde_json::Value, ClientError> {
            let resp = based_runtime::AdoptedTransport::insert_fixture(self, model, row).await;
            support::reply(resp.status, resp.body)
        }
    }
"#;

/// Closes the fixtures module.
pub(super) const FIXTURES_TAIL: &str = "}\n";

/// A composite key names its own row.
pub(super) fn composite_self_ref(model: &str) -> String {
    format!(
        "\n    impl RowRef<{model}Id> for {model}Id {{\n        fn key(&self) -> Self {{\n            self.clone()\n        }}\n    }}\n"
    )
}
