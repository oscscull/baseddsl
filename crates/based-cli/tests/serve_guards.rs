//! Real CLI requests prove callback failure cannot write or consume a key.
#[path = "support/project.rs"]
mod support;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use support::{success, Project};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn project(endpoint: &str) -> Project {
    let project = Project::new();
    project.write(
        "schema/item.bsl",
        r#"
        Item { id: Id name: text }
        mutation make_item(name) -> Item guard permission { create Item { name = $name }; }
        query items() -> Item[] { list Item; }
    "#,
    );
    project.write(".env", "DATABASE_URL=service.db\n");
    project.write(
        "guards.toml",
        &format!(
            r#"
        [[guards]]
        name = "permission"
        endpoint = "{endpoint}"
        secret_env = "BASED_TEST_GUARD_SECRET"
        allow_loopback_http = true
        timeout_ms = 1000
    "#
        ),
    );
    success(project.run("", &["migrate", "gen"]));
    success(project.run("", &["migrate", "apply"]));
    project
}

fn start(project: &Project) -> (Server, SocketAddr) {
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let mut server = Server(
        project
            .command(
                "",
                &[
                    "serve",
                    "--listen",
                    &address.to_string(),
                    "--guard-config",
                    "guards.toml",
                    "--init-idempotency-table",
                ],
            )
            .env("BASED_TEST_GUARD_SECRET", "test-only")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while TcpStream::connect(address).is_err() {
        assert!(server.0.try_wait().unwrap().is_none(), "startup failed");
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    (server, address)
}

fn post(address: SocketAddr, path: &str, body: &str) -> (u16, serde_json::Value) {
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(socket, "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nIdempotency-Key: same-key\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    (
        response.split_whitespace().nth(1).unwrap().parse().unwrap(),
        serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap(),
    )
}

#[test]
fn guards_recheck_replays_and_failures_leave_no_writes_or_claims() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let project = project(&format!("http://{}", listener.local_addr().unwrap()));
    let allowed = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let callback = spawn_callback(listener, allowed.clone(), calls.clone());
    let (_server, address) = start(&project);
    let body = r#"{"name":"sample"}"#;
    assert_eq!(post(address, "/m/make_item", body).0, 403);
    assert_eq!(post(address, "/q/items", "{}").1, serde_json::json!([]));
    allowed.store(true, Ordering::SeqCst);
    let committed = post(address, "/m/make_item", body);
    assert_eq!(committed.0, 200);
    allowed.store(false, Ordering::SeqCst);
    assert_eq!(post(address, "/m/make_item", body).0, 403);
    allowed.store(true, Ordering::SeqCst);
    assert_eq!(post(address, "/m/make_item", body), committed);
    callback.join().unwrap();
    let failure = post(address, "/m/make_item", body);
    assert_eq!(failure.0, 403);
    assert_eq!(failure.1["error"]["message"], "Guard check unavailable");
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!(
        post(address, "/q/items", "{}").1.as_array().unwrap().len(),
        1
    );
}

#[test]
fn startup_requires_complete_valid_operator_mappings_and_redacts_secrets() {
    let project = project("http://example.com/private-token");
    for options in [vec![], vec!["--guard-config", "guards.toml"]] {
        let mut args = vec!["serve"];
        args.extend(options);
        let output = project
            .command("", &args)
            .env("BASED_TEST_GUARD_SECRET", "private-token")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("permission"), "{error}");
        assert!(!error.contains("private-token"), "{error}");
    }
}

fn spawn_callback(
    listener: TcpListener,
    allowed: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if bytes.len() >= end + 4 + length {
                        assert!(headers.contains("authorization: bearer test-only"));
                        break;
                    }
                }
                assert!(count > 0);
            }
            calls.fetch_add(1, Ordering::SeqCst);
            let body = if allowed.load(Ordering::SeqCst) {
                r#"{"version":1,"verdict":"allow"}"#
            } else {
                r#"{"version":1,"verdict":"deny","message":"Permission denied"}"#
            };
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    })
}

#[test]
fn invalid_deadlines_and_duplicate_mappings_name_the_guard() {
    let project = project("http://127.0.0.1:9/check");
    let valid = std::fs::read_to_string(project.0.join("guards.toml")).unwrap();
    let mut cases = [-1, 0, 30_001]
        .map(|millis| valid.replace("timeout_ms = 1000", &format!("timeout_ms = {millis}")))
        .to_vec();
    cases.push(format!("{valid}\n{valid}"));
    cases.push(valid.replace("permission", "unknown_guard"));
    cases.push(format!("{valid}\nca_file = \"missing-ca.pem\"\n"));
    for config in cases {
        project.write("guards.toml", &config);
        let output = project
            .command("", &["serve", "--guard-config", "guards.toml"])
            .env("BASED_TEST_GUARD_SECRET", "test-only")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let message = String::from_utf8(output.stderr).unwrap();
        let name = if config.contains("unknown_guard") {
            "unknown_guard"
        } else {
            "permission"
        };
        assert!(message.contains(name), "{message}");
        assert!(!message.contains("test-only"), "{message}");
    }
}
