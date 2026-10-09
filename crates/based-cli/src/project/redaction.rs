//! Redact credentials before connection diagnostics are displayed.
/// Redact a database URL's password for logging (`mysql://user:pw@host` → `mysql://user@host`).
pub fn redact(url: &str) -> String {
    match (url.find("://"), url.find('@')) {
        (Some(s), Some(at)) if at > s => {
            let scheme = &url[..s + 3];
            let creds = &url[s + 3..at];
            let user = creds.split(':').next().unwrap_or(creds);
            format!("{scheme}{user}@{}", &url[at + 1..])
        }
        _ => url.to_string(),
    }
}
