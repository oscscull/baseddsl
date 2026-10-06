//! HTTP-level helper for isolated standalone CLI processes.
use crate::support::Project;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

pub struct Server {
    child: Child,
    pub address: SocketAddr,
}

impl Server {
    pub fn start(project: &Project, options: &[&str]) -> Self {
        let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = reservation.local_addr().unwrap();
        drop(reservation);
        let listen = address.to_string();
        let mut args = vec!["serve", "--listen", &listen];
        args.extend_from_slice(options);
        let child = project
            .command("", &args)
            .env_remove("BASED_IDEMPOTENCY_STORE")
            .env_remove("BASED_INIT_IDEMPOTENCY_TABLE")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut server = Self { child, address };
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            assert!(
                server.child.try_wait().unwrap().is_none(),
                "standalone startup failed"
            );
            if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
                return server;
            }
            assert!(Instant::now() < deadline, "standalone startup timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn post(
    address: SocketAddr,
    path: &str,
    body: &str,
    context: &str,
    key: Option<&str>,
) -> (u16, serde_json::Value) {
    post_on_shard(address, path, body, context, key, None)
}

pub fn post_on_shard(
    address: SocketAddr,
    path: &str,
    body: &str,
    context: &str,
    key: Option<&str>,
    shard: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let key_header = key.map_or(String::new(), |key| format!("Idempotency-Key: {key}\r\n"));
    let shard_header = shard.map_or(String::new(), |shard| {
        format!("X-Based-Shard-Key: {shard}\r\n")
    });
    write!(socket, "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nX-Based-Context: {context}\r\n{key_header}{shard_header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    let status = response.split_whitespace().nth(1).unwrap().parse().unwrap();
    let payload = response.split_once("\r\n\r\n").unwrap().1;
    (status, serde_json::from_str(payload).unwrap())
}
