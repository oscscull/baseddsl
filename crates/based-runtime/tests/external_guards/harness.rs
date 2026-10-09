//! Controlled callback socket fixture, including authentication and response failures.
use based_runtime::Guards;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
    task::JoinHandle,
};

pub const ALLOW: &str = r#"{"version":1,"verdict":"allow"}"#;
pub const DENY: &str = r#"{"version":1,"verdict":"deny","message":"Permission denied"}"#;

pub fn response(status: &str, headers: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
        body.len()
    )
    .into_bytes()
}

pub fn json_response(body: &str) -> Vec<u8> {
    response("200 OK", "Content-Type: application/json\r\n", body)
}

pub struct Callback {
    pub endpoint: String,
    pub reply: Arc<Mutex<Vec<u8>>>,
    pub received: mpsc::UnboundedReceiver<Vec<u8>>,
    task: JoinHandle<()>,
}

impl Drop for Callback {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Callback {
    pub async fn start(reply: Vec<u8>, delay: Duration) -> Self {
        Self::start_with_auth(reply, delay, None).await
    }

    pub async fn authenticated(secret: &'static str) -> Self {
        Self::start_with_auth(json_response(ALLOW), Duration::ZERO, Some(secret)).await
    }

    async fn start_with_auth(
        reply: Vec<u8>,
        delay: Duration,
        secret: Option<&'static str>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/guard", listener.local_addr().unwrap());
        let reply = Arc::new(Mutex::new(reply));
        let outgoing = Arc::clone(&reply);
        let (tx, received) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut bytes = Vec::new();
                let mut chunk = [0; 4096];
                loop {
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if complete(&bytes) {
                        break;
                    }
                }
                let authenticated = secret.is_none_or(|secret| has_bearer(&bytes, secret));
                let _ = tx.send(bytes);
                tokio::time::sleep(delay).await;
                let reply = match authenticated {
                    true => outgoing.lock().unwrap().clone(),
                    false => response("401 Unauthorized", "", ""),
                };
                let _ = socket.write_all(&reply).await;
            }
        });
        Self {
            endpoint,
            reply,
            received,
            task,
        }
    }

    pub fn guards(&self, secret: &str, deadline_ms: u64) -> Guards {
        based_runtime::external_guard::HttpGuard::new(
            "can_create".into(),
            &self.endpoint,
            secret,
            Duration::from_millis(deadline_ms),
            true,
        )
        .unwrap()
        .register(Guards::new())
    }
}

fn complete(bytes: &[u8]) -> bool {
    let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&bytes[..end]);
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap();
    bytes.len() >= end + 4 + length
}

fn has_bearer(request: &[u8], secret: &str) -> bool {
    let headers = String::from_utf8_lossy(request);
    headers
        .lines()
        .take_while(|line| !line.is_empty())
        .any(|line| {
            let Some((name, value)) = line.split_once(':') else {
                return false;
            };
            name.eq_ignore_ascii_case("authorization") && value.trim() == format!("Bearer {secret}")
        })
}
