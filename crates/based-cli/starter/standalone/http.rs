use reqwest::blocking::Client;
use serde_json::Value;
use std::time::Duration;

pub struct Api {
    client: Client,
    url: String,
}

impl Api {
    pub fn new(url: String) -> Result<Self, reqwest::Error> {
        Ok(Self {
            url,
            client: Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()?,
        })
    }

    pub fn call(&self, route: &str, payload: Value) -> Result<Value, reqwest::Error> {
        self.client
            .post(format!("{}/{route}", self.url))
            .header(
                "X-Based-Context",
                r#"{"owner":"00000000-0000-4000-8000-000000000001"}"#,
            )
            .json(&payload)
            .send()?
            .error_for_status()?
            .json()
    }
}
