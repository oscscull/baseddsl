//! Docker-backed ephemeral Postgres harness for real-DB integration tests.
//!
//! The twin of [`docker_mariadb`](super::docker_mariadb): it brings up a throwaway Postgres
//! server in a container, waits for it to accept connections, and tears it down when the test
//! ends — so `tests/postgres_integration.rs` can run the *verbatim* Postgres-lowered
//! (`$n`-bound) `based gen sql` output against a genuine server instead of a `MockDb`.
//! It shells out to the `docker` CLI directly (a thin guard, not a heavy testcontainers
//! dependency); the readiness wait and setup helpers ride sqlx, the same executor the
//! runtime's drivers use.
//!
//! **No daemon ⇒ skip, never fail.** [`PostgresContainer::start`] returns `None` when the
//! Docker daemon is unreachable or the image/run/readiness steps do not complete, logging a
//! clear reason. The suite treats `None` as "skip this test", so `cargo test --workspace
//! --all-features` stays green on a machine with no Docker — the real-DB proof runs *when
//! infra is present* and is simply absent otherwise (never a red build for want of infra).
//!
//! **CI-provided server ⇒ use it, don't spin one.** When `TEST_POSTGRES_URL` is set,
//! [`PostgresContainer::start`] connects to *that* server (a CI service container, a shared
//! dev DB, …) instead of launching its own — after the same readiness-wait, so the suite
//! never races a still-booting server; `Drop` then leaves the external server alone. This is
//! what lets the portable `make ci-live-postgres` target run the live suite against a GitHub
//! Actions `services:` container while a laptop with a Docker daemon keeps the self-spun
//! behaviour with no env set. Because an external server *persists* across tests, every suite
//! helper resets the schema (`DROP SCHEMA public CASCADE`) before creating tables.

use std::process::Command;
use std::time::{Duration, Instant};

use sqlx::Connection;

/// A pinned Postgres image. Pinned (not `latest`) so the suite tests a known server version
/// and a CI cache stays warm; 16 is a current stable major with the native `uuid` /
/// `timestamptz` / `jsonb` types the generated Postgres DDL emits.
const IMAGE: &str = "postgres:16";

/// The in-container superuser password + database the harness provisions. The server is
/// ephemeral and bound to loopback only, so a fixed throwaway password is fine.
const PASSWORD: &str = "based_test_pw";
const USER: &str = "postgres";
const DATABASE: &str = "based_test";

/// The env var that points the suite at an externally-provided server (a CI service
/// container). When set, the harness connects to it instead of spinning its own container.
const URL_ENV: &str = "TEST_POSTGRES_URL";

/// How long to wait for the freshly-started server to accept a real connection before
/// giving up (Postgres's first boot initializes the data dir + creates the database).
const READY_TIMEOUT: Duration = Duration::from_secs(90);

/// Per-attempt connect cap. A socket that accepts but never completes the handshake (a
/// container that died while OrbStack still forwards its port) must not wedge a single
/// connect past the overall deadline — bounding each attempt keeps the poll honoring it.
const CONNECT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(5);

/// A live Postgres the suite runs against. Either a **self-spun** ephemeral container (owned:
/// [`Drop`] force-`docker rm`s it, so a panicking test still cleans up) or an **external**
/// server named by `TEST_POSTGRES_URL` (unowned: `Drop` leaves it alone). Hand [`url`] to the
/// driver in both cases.
pub struct PostgresContainer {
    kind: Kind,
}

enum Kind {
    /// A container this process launched and must remove.
    Spun { id: String, port: u16 },
    /// A server provided by the environment (`TEST_POSTGRES_URL`); not ours to tear down.
    External { url: String },
}

impl PostgresContainer {
    /// Connect to a live Postgres and wait until it accepts connections.
    ///
    /// Prefers an externally-provided server (`TEST_POSTGRES_URL`, e.g. a CI service
    /// container); otherwise spins an ephemeral container via Docker. Returns `None` (after
    /// logging why) when neither is reachable/ready — the caller skips rather than failing.
    pub async fn start() -> Option<Self> {
        // CI-provided server takes precedence: connect to it after the readiness-wait.
        if let Ok(url) = std::env::var(URL_ENV) {
            let url = url.trim().to_string();
            if !url.is_empty() {
                eprintln!("[docker-postgres] using external {URL_ENV}={url}");
                // An externally-provided server is expected to be up: if it never answers,
                // fail loudly (don't skip) so a dead CI/`make check` container is a red build
                // with a clear cause, not a green skip.
                assert!(
                    wait_ready(&url).await,
                    "[docker-postgres] DB at {url} not ready after {}s — is the container running?",
                    READY_TIMEOUT.as_secs()
                );
                return Some(Self {
                    kind: Kind::External { url },
                });
            }
        }

        if !docker_available() {
            eprintln!(
                "[docker-postgres] SKIP: Docker daemon not reachable (`docker info` failed). \
                 The live Postgres suite needs a running daemon (OrbStack/Docker Desktop); \
                 skipping cleanly so the build stays green."
            );
            return None;
        }

        // Let Docker pick a free host port (`-p 0:5432`) so parallel test binaries never
        // collide on a fixed port. We read the mapped port back after the container starts.
        let run = Command::new("docker")
            .args([
                "run",
                "--detach",
                "--rm",
                "--publish",
                "0:5432",
                "--env",
                &format!("POSTGRES_PASSWORD={PASSWORD}"),
                "--env",
                &format!("POSTGRES_DB={DATABASE}"),
                IMAGE,
            ])
            .output()
            .ok()?;
        if !run.status.success() {
            eprintln!(
                "[docker-postgres] SKIP: `docker run` failed: {}",
                String::from_utf8_lossy(&run.stderr).trim()
            );
            return None;
        }
        let id = String::from_utf8_lossy(&run.stdout).trim().to_string();

        let Some(port) = mapped_port(&id) else {
            eprintln!("[docker-postgres] SKIP: could not read the mapped host port");
            remove(&id);
            return None;
        };
        let container = Self {
            kind: Kind::Spun { id, port },
        };

        if !wait_ready(&container.url()).await {
            eprintln!(
                "[docker-postgres] SKIP: Postgres did not become ready within {}s",
                READY_TIMEOUT.as_secs()
            );
            // `container` drops here, removing the container.
            return None;
        }
        if let Kind::Spun { id, port } = &container.kind {
            eprintln!(
                "[docker-postgres] ready: {} on 127.0.0.1:{}",
                &id[..12.min(id.len())],
                port
            );
        }
        Some(container)
    }

    /// A `postgres://` URL the `postgres` driver connects to: the external URL as-provided, or
    /// (for a self-spun container) superuser + provisioned database on the mapped loopback port.
    pub fn url(&self) -> String {
        match &self.kind {
            Kind::Spun { port, .. } => {
                format!("postgres://{USER}:{PASSWORD}@127.0.0.1:{port}/{DATABASE}")
            }
            Kind::External { url } => url.clone(),
        }
    }

    /// Run a multi-statement setup script (schema reset / DDL / seed) against the live
    /// server on one one-shot connection. Panics on failure: setup SQL failing is a broken
    /// fixture, not a test outcome.
    #[allow(dead_code)] // not every includer runs setup SQL
    pub async fn exec_batch(&self, sql: &str) {
        let mut conn = sqlx::postgres::PgConnection::connect(&self.url())
            .await
            .expect("setup connection");
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&mut conn)
            .await
            .unwrap_or_else(|e| panic!("setup batch failed: {e}\n{sql}"));
    }
}

/// Poll a real connection until the server answers `SELECT 1` or the timeout elapses. A fresh
/// (or still-booting CI) Postgres rejects connections for a moment while it initializes; we
/// retry rather than sleeping a fixed amount so a ready server starts the suite promptly. This
/// is the portable readiness-wait — the same poll for a self-spun and a CI service container,
/// so `based migrate apply` / the live suite never races a booting DB.
/// Bounded: returns `false` once [`READY_TIMEOUT`] elapses (each attempt is itself capped so a
/// dead-but-still-forwarding socket can't hang it), so a dead DB is caught, not waited on forever.
async fn wait_ready(url: &str) -> bool {
    wait_ready_within(url, READY_TIMEOUT).await
}

async fn wait_ready_within(url: &str, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        let probe = tokio::time::timeout(CONNECT_ATTEMPT_TIMEOUT, async {
            let mut conn = sqlx::postgres::PgConnection::connect(url).await.ok()?;
            sqlx::raw_sql("SELECT 1").execute(&mut conn).await.ok()
        })
        .await;
        if matches!(probe, Ok(Some(_))) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

#[tokio::test]
async fn wait_ready_is_bounded_for_a_dead_server() {
    // Nothing listens here: the poll must give up at its (tiny) deadline, never spin forever.
    let start = Instant::now();
    let ready = wait_ready_within(
        "postgres://postgres:pw@127.0.0.1:1/nope",
        Duration::from_millis(300),
    )
    .await;
    assert!(!ready, "an unreachable server is never 'ready'");
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "wait_ready must honor its deadline, not hang"
    );
}

impl Drop for PostgresContainer {
    fn drop(&mut self) {
        // Force-remove a self-spun container (best effort — a failed teardown must not mask a
        // test result). An external server (`TEST_POSTGRES_URL`) is not ours to remove.
        if let Kind::Spun { id, .. } = &self.kind {
            remove(id);
        }
    }
}

/// Is the Docker daemon reachable? `docker info` exits non-zero (fast) when the daemon is
/// down, so it is a cheap, reliable probe that avoids a slow `docker run` timeout on a
/// machine with the CLI installed but no running daemon.
fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Read the host port Docker mapped to the container's 5432 (`docker port <id> 5432/tcp`
/// prints `0.0.0.0:49153` etc.; we take the port after the last colon).
fn mapped_port(id: &str) -> Option<u16> {
    let out = Command::new("docker")
        .args(["port", id, "5432/tcp"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Multiple lines (IPv4 + IPv6) may print; the first mapping's port suffices.
    text.lines()
        .next()
        .and_then(|line| line.rsplit(':').next())
        .and_then(|p| p.trim().parse().ok())
}

/// Force-remove a container by id (best effort).
fn remove(id: &str) {
    let _ = Command::new("docker").args(["rm", "--force", id]).output();
}
