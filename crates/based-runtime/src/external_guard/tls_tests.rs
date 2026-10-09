//! Controlled TLS server proves trust-chain and hostname enforcement in the adapter.
use super::*;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::rustls::{
    self,
    pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer},
};

struct TlsServer {
    directory: PathBuf,
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TlsServer {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

impl TlsServer {
    async fn start() -> Self {
        let directory =
            std::env::temp_dir().join(format!("based-guard-tls-{}", uuid::Uuid::new_v4()));
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ci/tls/certificates.sh");
        assert!(std::process::Command::new("bash")
            .arg(script)
            .arg(&directory)
            .status()
            .unwrap()
            .success());
        let certificates = CertificateDer::pem_file_iter(directory.join("server.crt"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let key = PrivateKeyDer::from_pem_file(directory.join("server.key")).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certificates, key)
        .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let Ok(mut tls) = acceptor.accept(socket).await else {
                    continue;
                };
                let mut bytes = [0; 4096];
                let _ = tls.read(&mut bytes).await;
                let body = r#"{"version":1,"verdict":"allow"}"#;
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = tls.write_all(response.as_bytes()).await;
                let _ = tls.shutdown().await;
            }
        });
        Self {
            directory,
            port,
            task,
        }
    }

    fn callback(&self, host: &str, trust_ca: bool) -> HttpGuard {
        let mut callback = HttpGuard::new(
            "can_create".into(),
            &format!("https://{host}:{}/guard", self.port),
            "secret",
            Duration::from_secs(2),
            false,
        )
        .unwrap();
        // Keep trust verification deterministic: use the fixture CA or a separate
        // untrusted CA, without relying on the machine's system trust service.
        let ca_file = match trust_ca {
            true => "ca.crt",
            false => "untrusted.crt",
        };
        let ca = std::fs::read(self.directory.join(ca_file)).unwrap();
        callback.client = reqwest::Client::builder()
            .no_proxy()
            .resolve("localhost", ([127, 0, 0, 1], self.port).into())
            .tls_certs_only([reqwest::Certificate::from_pem(&ca).unwrap()])
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .unwrap();
        callback
    }
}

fn request() -> GuardRequest {
    GuardRequest {
        callable: "create_item".into(),
        args: serde_json::json!({}),
        ctx: serde_json::json!({}),
        engine: None,
    }
}

#[tokio::test]
async fn tls_requires_trusted_certificate_and_matching_hostname() {
    let server = TlsServer::start().await;
    assert_eq!(
        server.callback("localhost", true).check(request()).await,
        GuardVerdict::Allow
    );
    for callback in [
        server.callback("localhost", false),
        server.callback("127.0.0.1", true),
    ] {
        assert_eq!(
            callback.check(request()).await,
            GuardVerdict::deny("Guard check unavailable")
        );
    }
}
