//! Explicit artifact destinations and client mode, shared by CLI and optional builders.
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ClientMode {
    #[default]
    Wire,
    Embedded,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationConfig {
    pub client: Option<String>,
    pub sql: Option<String>,
    pub openapi: Option<String>,
    #[serde(default)]
    pub client_mode: ClientMode,
}
