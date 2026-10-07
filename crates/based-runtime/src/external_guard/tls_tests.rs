//! Local TLS server: certificate trust and hostname verification remain mandatory.
use super::*;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

struct Fixture {
    directory: std::path::PathBuf,
    child: std::process::Child,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test]
async fn tls_rejects_untrusted_ca_and_wrong_hostname_and_accepts_trusted_localhost() {
    let directory = std::env::temp_dir().join(format!(
        "based-guard-tls-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ci/tls/certificates.sh");
    assert!(Command::new("bash")
        .arg(script)
        .arg(&directory)
        .status()
        .unwrap()
        .success());
    eprintln!("TLS fixture: certificates ready");
    let python = r#"
import socket, ssl, sys
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(sys.argv[1] + '/server.crt', sys.argv[1] + '/server.key')
sock = socket.socket()
sock.bind(('127.0.0.1', 0)); sock.listen()
print(sock.getsockname()[1], flush=True)
while True:
    raw, _ = sock.accept()
    raw.settimeout(5)
    try:
        with ctx.wrap_socket(raw, server_side=True) as conn:
            request = b''
            while b'\r\n\r\n' not in request:
                chunk = conn.recv(4096)
                if not chunk: raise EOFError()
                request += chunk
            headers, payload = request.split(b'\r\n\r\n', 1)
            length = int(next(line.split(b':', 1)[1] for line in headers.lower().split(b'\r\n') if line.startswith(b'content-length:')))
            while len(payload) < length:
                chunk = conn.recv(4096)
                if not chunk: raise EOFError()
                payload += chunk
            body = b'{"version":1,"verdict":"allow"}'
            conn.sendall(b'HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: ' + str(len(body)).encode() + b'\r\n\r\n' + body)
    except (ssl.SSLError, OSError, EOFError): raw.close()
"#;
    let mut child = Command::new("python3")
        .args(["-u", "-c", python])
        .arg(&directory)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut port = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut port)
        .unwrap();
    eprintln!("TLS fixture: server ready");
    let fixture = Fixture { directory, child };
    let url = format!("https://localhost:{}", port.trim());
    let guard = HttpGuard::new(&url, "test", Duration::from_secs(2), false).unwrap();
    let req = || GuardRequest {
        callable: "make_item".into(),
        args: serde_json::json!({}),
        ctx: serde_json::json!({}),
        engine: None,
    };
    assert_eq!(
        guard.check("permission", req()).await,
        GuardVerdict::deny("Guard check unavailable")
    );
    let pem = std::fs::read(fixture.directory.join("ca.crt")).unwrap();
    let guard = guard.with_ca_pem(&pem).unwrap();
    assert_eq!(guard.check("permission", req()).await, GuardVerdict::Allow);
    let guard = HttpGuard::new(
        &format!("https://127.0.0.1:{}", port.trim()),
        "test",
        Duration::from_secs(2),
        false,
    )
    .unwrap()
    .with_ca_pem(&pem)
    .unwrap();
    assert_eq!(
        guard.check("permission", req()).await,
        GuardVerdict::deny("Guard check unavailable")
    );
}
