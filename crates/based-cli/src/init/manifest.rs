//! Configure generation for the chosen starter mode.
use super::options::{Mode, Options};

pub fn render(options: &Options) -> String {
    let generate = match options.mode {
        Mode::Embedded => "client_mode = \"embedded\"",
        Mode::Standalone => "sql = \"generated/schema.sql\"\nopenapi = \"generated/openapi.json\"",
    };
    format!(
        "dialect = \"{}\"\nroot = \"schema\"\n\n[generate]\n{generate}\n",
        options.dialect.name()
    )
}
