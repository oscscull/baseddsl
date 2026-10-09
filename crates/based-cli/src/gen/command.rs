//! Stable regeneration commands, expressed relative to the manifest directory.
use super::render::Kind;
use std::path::Path;

pub(super) fn regeneration(kind: Kind, path: Option<&Path>, embedded: bool) -> String {
    let target = match kind {
        Kind::Client => "client",
        Kind::Sql => "sql",
        Kind::OpenApi => "openapi",
    };
    let mut command = format!("based gen {target}");
    if let Some(path) = path {
        command.push_str(" --out=");
        command.push_str(&quote(&path.to_string_lossy()));
    }
    if matches!(kind, Kind::Client) {
        command.push_str(if embedded {
            " --mode embedded"
        } else {
            " --mode wire"
        });
    }
    command
}

fn quote(value: &str) -> String {
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "/._-".contains(character))
    {
        return value.into();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
