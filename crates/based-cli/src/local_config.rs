//! Resolve live-command connection configuration without mutating process env.
use crate::error::CliError;
use based_codegen::Dialect;
use std::collections::HashMap;
use std::path::Path;

pub fn shard_urls(
    root: &Path,
    dialect: Dialect,
    explicit: Vec<String>,
) -> Result<Vec<String>, CliError> {
    let urls = connection_values(root, explicit)?;
    if urls.is_empty() || urls.iter().any(|url| url.trim().is_empty()) {
        return Err(CliError::usage(
            "no database url: pass --database-url <url> or set BASED_DATABASE_URL / DATABASE_URL in the environment or project .env",
        ));
    }
    if dialect == Dialect::Sqlite && urls.iter().any(|url| url.contains("://")) {
        return Err(CliError::usage(
            "sqlite connection must be a file path or :memory:, not a server URL (value redacted)",
        ));
    }
    Ok(urls
        .into_iter()
        .map(|url| {
            if dialect != Dialect::Sqlite || url == ":memory:" || Path::new(&url).is_absolute() {
                return url;
            }
            root.join(url).to_string_lossy().into_owned()
        })
        .collect())
}

fn connection_values(root: &Path, explicit: Vec<String>) -> Result<Vec<String>, CliError> {
    if !explicit.is_empty() {
        return Ok(explicit);
    }
    let process = std::env::var("BASED_DATABASE_URL").or_else(|_| std::env::var("DATABASE_URL"));
    if let Ok(value) = process {
        return Ok(split(&value));
    }
    let local = read_dotenv(root)?;
    Ok(local
        .get("BASED_DATABASE_URL")
        .or_else(|| local.get("DATABASE_URL"))
        .map_or_else(Vec::new, |value| split(value)))
}

fn split(value: &str) -> Vec<String> {
    value.split(',').map(|url| url.trim().to_owned()).collect()
}

fn read_dotenv(root: &Path) -> Result<HashMap<String, String>, CliError> {
    let path = root.join(".env");
    let entries = match dotenvy::from_path_iter(&path) {
        Ok(entries) => entries,
        Err(dotenvy::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(HashMap::new())
        }
        Err(_) => {
            return Err(CliError::usage(format!(
                "cannot read project config {}; check file permissions",
                path.display()
            )))
        }
    };
    entries.collect::<Result<HashMap<_, _>, _>>().map_err(|_| {
        CliError::usage(format!(
            "invalid project config {}; expected dotenv assignments (values redacted)",
            path.display()
        ))
    })
}
