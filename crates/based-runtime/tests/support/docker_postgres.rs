//! Docker-backed ephemeral Postgres harness for real-DB integration tests (A3/D38).
//!
//! The twin of [`docker_mariadb`](super::docker_mariadb): it brings up a throwaway Postgres
//! server in a container, waits for it to accept connections, and tears it down when the test
//! ends — so `tests/postgres_integration.rs` can run the *verbatim* Postgres-lowered
//! (`$n`-bound, D29) `based gen sql` output against a genuine server instead of a `MockDb`.
//! It shells out to the `docker` CLI directly (a thin guard, not a heavy testcontainers
//! dependency — principle 7, and no async runtime pulled into the sync codebase).
//!
//! **No daemon ⇒ skip, never fail.** [`PostgresContainer::start`] returns `None` when the
//! Docker daemon is unreachable or the image/run/readiness steps do not complete, logging a
//! clear reason. The suite treats `None` as "skip this test", so `cargo test --workspace
//! --all-features` stays green on a machine with no Docker — the real-DB proof runs *when
//! infra is present* and is simply absent otherwise (never a red build for want of infra).

use std::process::Command;
use std::time::{Duration, Instant};

use based_runtime::pg_connect;

/// A pinned Postgres image. Pinned (not `latest`) so the suite tests a known server version
/// and a CI cache stays warm; 16 is a current stable major with the native `uuid` /
/// `timestamptz` / `jsonb` types the generated Postgres DDL emits (D29).
const IMAGE: &str = "postgres:16";

/// The in-container superuser password + database the harness provisions. The server is
/// ephemeral and bound to loopback only, so a fixed throwaway password is fine.
const PASSWORD: &str = "based_test_pw";
const USER: &str = "postgres";
const DATABASE: &str = "based_test";

/// How long to wait for the freshly-started server to accept a real connection before
/// giving up (Postgres's first boot initializes the data dir + creates the database).
const READY_TIMEOUT: Duration = Duration::from_secs(90);

/// A running ephemeral Postgres container. Owns the container's lifetime: [`Drop`] removes
/// it (force `docker rm`), so a panicking test still cleans up. Hand [`url`] to the driver.
pub struct PostgresContainer {
    id: String,
    port: u16,
}

impl PostgresContainer {
    /// Start an ephemeral Postgres and wait until it accepts connections.
    ///
    /// Returns `None` (after logging why) when Docker is unreachable or the container never
    /// becomes ready — the caller skips the test rather than failing it. On success the
    /// returned container is live and its [`url`] connects to an empty `based_test` database.
    pub fn start() -> Option<PostgresContainer> {
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

        let port = match mapped_port(&id) {
            Some(p) => p,
            None => {
                eprintln!("[docker-postgres] SKIP: could not read the mapped host port");
                remove(&id);
                return None;
            }
        };
        let container = PostgresContainer { id, port };

        if !container.wait_ready() {
            eprintln!(
                "[docker-postgres] SKIP: Postgres did not become ready within {}s",
                READY_TIMEOUT.as_secs()
            );
            // `container` drops here, removing the container.
            return None;
        }
        eprintln!(
            "[docker-postgres] ready: {} on 127.0.0.1:{}",
            &container.id[..12.min(container.id.len())],
            container.port
        );
        Some(container)
    }

    /// A `postgres://` URL the `postgres` driver connects to (superuser, the provisioned
    /// database, on the mapped loopback port).
    pub fn url(&self) -> String {
        format!(
            "postgres://{USER}:{PASSWORD}@127.0.0.1:{}/{DATABASE}",
            self.port
        )
    }

    /// Poll a real connection until the server answers `SELECT 1` or the timeout elapses. A
    /// fresh Postgres rejects connections for a moment while it initializes; we retry rather
    /// than sleeping a fixed amount so a fast machine starts the suite promptly.
    fn wait_ready(&self) -> bool {
        let deadline = Instant::now() + READY_TIMEOUT;
        let url = self.url();
        while Instant::now() < deadline {
            if let Ok(mut client) = pg_connect(&url) {
                if client.simple_query("SELECT 1").is_ok() {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        false
    }
}

impl Drop for PostgresContainer {
    fn drop(&mut self) {
        // Force-remove the container (started `--rm`, but a stopped-but-not-removed or a
        // still-running container both get cleaned up here). Best effort — a failed teardown
        // must not mask a test result.
        remove(&self.id);
    }
}

/// Is the Docker daemon reachable? `docker info` exits non-zero (fast) when the daemon is
/// down, so it is a cheap, reliable probe that avoids a slow `docker run` timeout on a
/// machine with the CLI installed but no running daemon.
fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
