//! Configure generated destinations for the chosen starter mode.
use super::options::{Mode, Options};

pub fn render(options: &Options) -> String {
    let destinations = match options.mode {
        Mode::Embedded => "client = \"generated/client.rs\"\nclient_mode = \"embedded\"",
        Mode::Standalone => "openapi = \"generated/openapi.json\"",
    };
    format!("dialect = \"{}\"\nroot = \"schema\"\n\n[generate]\nsql = \"generated/schema.sql\"\n{destinations}\n", options.dialect.name())
}
