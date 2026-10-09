use std::{
    net::TcpListener,
    process::{Child, Command},
    time::{Duration, Instant},
};

pub struct Service {
    child: Child,
    url: String,
}

impl Service {
    pub fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let reservation = TcpListener::bind("127.0.0.1:0")?;
        let address = reservation.local_addr()?;
        drop(reservation);
        let child = Command::new(std::env::var("BASED").unwrap_or_else(|_| "based".into()))
            .args([
                "serve",
                "--listen",
                &address.to_string(),
                "--database-url",
                &std::env::var("DATABASE_URL").unwrap_or_else(|_| "local.db".into()),
                "--idempotency-store",
                "memory",
            ])
            .spawn()?;
        let mut service = Self {
            child,
            url: format!("http://{address}"),
        };
        service.ready()?;
        Ok(service)
    }

    pub fn url(&self) -> String {
        self.url.clone()
    }

    fn ready(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(1))
            .build()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.child.try_wait()?.is_some() {
                return Err("Based exited before becoming ready".into());
            }
            if client
                .get(format!("{}/readyz", self.url))
                .send()
                .is_ok_and(|response| response.status().is_success())
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err("Based did not become ready".into())
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
