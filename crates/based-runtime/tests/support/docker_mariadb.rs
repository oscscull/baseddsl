//! Docker-backed ephemeral MariaDB harness for real-DB integration tests (D35).
//!
//! This brings up a throwaway MariaDB server in a container, waits for it to accept
//! connections, and tears it down when the test ends — so a live integration suite can run
//! the *verbatim* `based gen sql` output against a genuine server instead of a `MockDb`.
//! It shells out to the `docker` CLI directly (OrbStack provides the daemon locally); a
//! thin guard rather than a heavy testcontainers dependency, matching principle 7's "reuse
//! the hardened external tool" without pulling an async runtime into a sync codebase.
//!
//! **No daemon ⇒ skip, never fail.** [`MariaDbContainer::start`] returns `None` when the
//! Docker daemon is unreachable (`docker info` fails) or the image/run/readiness steps do
//! not complete, logging a clear reason. The suite treats `None` as "skip this test", so
//! `cargo test --workspace --all-features` stays green on a machine with no Docker — the
//! real-DB proof runs *when infra is present* and is simply absent otherwise (it never
//! turns a missing daemon into a red build).
//!
//! **CI-provided server ⇒ use it, don't spin one (D64).** When `TEST_MARIADB_URL` is set,
//! [`MariaDbContainer::start`] connects to *that* server (a CI service container, a shared
//! dev DB, …) instead of launching its own container — after the same readiness-wait, so the
//! suite never races a still-booting server. `Drop` then leaves the external server alone.
//! This is what lets the portable `make ci-live-mariadb` target run the live suite against a
//! GitHub Actions `services:` container while a laptop with a Docker daemon keeps the
//! self-spun behaviour with no env set. Because an external server *persists* across tests,
//! every suite helper resets its tables before creating them (idempotent, re-runnable).

use std::process::Command;
use std::time::{Duration, Instant};

/// A pinned MariaDB image. Pinned (not `latest`) so the suite tests a known server version
/// and a CI cache stays warm; 11.4 is an LTS release with the native `UUID`/`DATETIME`
/// types the generated MariaDB DDL emits.
const IMAGE: &str = "mariadb:11.4";

/// The in-container root password + database the harness provisions. The server is
/// ephemeral and bound to loopback only, so a fixed throwaway password is fine.
const ROOT_PASSWORD: &str = "based_test_pw";
const DATABASE: &str = "based_test";

/// The env var that points the suite at an externally-provided server (a CI service
/// container). When set, the harness connects to it instead of spinning its own container.
const URL_ENV: &str = "TEST_MARIADB_URL";

/// How long to wait for the freshly-started server to accept a real connection before
/// giving up (MariaDB's first boot initializes the data dir + creates the database).
const READY_TIMEOUT: Duration = Duration::from_secs(90);

/// A live MariaDB the suite runs against. Either a **self-spun** ephemeral container (owned:
/// [`Drop`] force-`docker rm`s it, so a panicking test still cleans up) or an **external**
/// server named by `TEST_MARIADB_URL` (unowned: `Drop` leaves it alone). Hand [`url`] to the
/// driver in both cases.
pub struct MariaDbContainer {
    kind: Kind,
}

enum Kind {
    /// A container this process launched and must remove.
    Spun { id: String, port: u16 },
    /// A server provided by the environment (`TEST_MARIADB_URL`); not ours to tear down.
    External { url: String },
}

impl MariaDbContainer {
    /// Connect to a live MariaDB and wait until it accepts connections.
    ///
    /// Prefers an externally-provided server (`TEST_MARIADB_URL`, e.g. a CI service
    /// container); otherwise spins an ephemeral container via Docker. Returns `None` (after
    /// logging why) when neither is reachable/ready — the caller skips the test rather than
    /// failing it.
    pub fn start() -> Option<MariaDbContainer> {
        // CI-provided server takes precedence: connect to it after the readiness-wait.
        if let Ok(url) = std::env::var(URL_ENV) {
            let url = url.trim().to_string();
            if !url.is_empty() {
                eprintln!("[docker-mariadb] using external {URL_ENV}={url}");
                if !wait_ready(&url) {
                    eprintln!(
                        "[docker-mariadb] SKIP: external server at {URL_ENV} not ready within {}s",
                        READY_TIMEOUT.as_secs()
                    );
                    return None;
                }
                return Some(MariaDbContainer {
                    kind: Kind::External { url },
                });
            }
        }

        if !docker_available() {
            eprintln!(
                "[docker-mariadb] SKIP: Docker daemon not reachable (`docker info` failed). \
                 The live MariaDB suite needs a running daemon (OrbStack/Docker Desktop); \
                 skipping cleanly so the build stays green."
            );
            return None;
        }

        // Let Docker pick a free host port (`-p 0:3306`) so parallel test binaries never
        // collide on a fixed port. We read the mapped port back after the container starts.
        let run = Command::new("docker")
            .args([
                "run",
                "--detach",
                "--rm",
                "--publish",
                "0:3306",
                "--env",
                &format!("MARIADB_ROOT_PASSWORD={ROOT_PASSWORD}"),
                "--env",
                &format!("MARIADB_DATABASE={DATABASE}"),
                // Bind loopback only; force UTF-8 so text/uuid round-trip as expected.
                IMAGE,
                "--bind-address=0.0.0.0",
                "--character-set-server=utf8mb4",
            ])
            .output()
            .ok()?;
        if !run.status.success() {
            eprintln!(
                "[docker-mariadb] SKIP: `docker run` failed: {}",
                String::from_utf8_lossy(&run.stderr).trim()
            );
            return None;
        }
        let id = String::from_utf8_lossy(&run.stdout).trim().to_string();

        let port = match mapped_port(&id) {
            Some(p) => p,
            None => {
                eprintln!("[docker-mariadb] SKIP: could not read the mapped host port");
                remove(&id);
                return None;
            }
        };
        let container = MariaDbContainer {
            kind: Kind::Spun { id, port },
        };

        if !wait_ready(&container.url()) {
            eprintln!(
                "[docker-mariadb] SKIP: MariaDB did not become ready within {}s",
                READY_TIMEOUT.as_secs()
            );
            // `container` drops here, removing the container.
            return None;
        }
        if let Kind::Spun { id, port } = &container.kind {
            eprintln!(
                "[docker-mariadb] ready: {} on 127.0.0.1:{}",
                &id[..12.min(id.len())],
                port
            );
        }
        Some(container)
    }

    /// A `mysql://` URL the `mariadb` driver connects to: the external URL as-provided, or
    /// (for a self-spun container) root user + provisioned database on the mapped loopback port.
    pub fn url(&self) -> String {
        match &self.kind {
            Kind::Spun { port, .. } => {
                format!("mysql://root:{ROOT_PASSWORD}@127.0.0.1:{port}/{DATABASE}")
            }
            Kind::External { url } => url.clone(),
        }
    }
}

/// Poll a real connection until the server answers `SELECT 1` or the timeout elapses. A fresh
/// (or still-booting CI) MariaDB rejects connections for a few seconds while it initializes;
/// we retry rather than sleeping a fixed amount so a ready server starts the suite promptly.
/// This is the portable readiness-wait — the same poll for a self-spun container and a CI
/// service container, so `based migrate apply` / the live suite never races a booting DB.
fn wait_ready(url: &str) -> bool {
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(pool) = mysql::Pool::new(url) {
            if let Ok(mut conn) = pool.get_conn() {
                use mysql::prelude::Queryable;
                if conn.query_drop("SELECT 1").is_ok() {
                    return true;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

impl Drop for MariaDbContainer {
    fn drop(&mut self) {
        // Force-remove a self-spun container (best effort — a failed teardown must not mask a
        // test result). An external server (`TEST_MARIADB_URL`) is not ours to remove.
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
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Read the host port Docker mapped to the container's 3306 (`docker port <id> 3306/tcp`
/// prints `0.0.0.0:49153` etc.; we take the port after the last colon).
fn mapped_port(id: &str) -> Option<u16> {
    let out = Command::new("docker")
        .args(["port", id, "3306/tcp"])
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
