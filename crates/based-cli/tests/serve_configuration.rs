//! Standalone startup uses the same ancestor manifest and selected .env as migrations.
#[path = "support/project.rs"]
mod support;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};
use support::{success, Project};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn serve_from_child_uses_project_local_connection() {
    let project = Project::new();
    project.write(".env", "DATABASE_URL=service.db\n");
    success(project.run("", &["migrate", "gen"]));
    success(project.run("", &["migrate", "apply"]));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let listen = address.to_string();
    let mut server = Server(
        project
            .command("src/nested", &["serve", "--listen", &listen])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut connection = loop {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "server failed before readiness"
        );
        if let Ok(connection) = TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
            break connection;
        }
        assert!(Instant::now() < deadline, "server failed to listen");
        std::thread::sleep(Duration::from_millis(20));
    };
    connection
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    connection.write_all(b"POST /q/items HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
    let mut response = String::new();
    connection.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(!project.0.join("src/nested/service.db").exists());
}
