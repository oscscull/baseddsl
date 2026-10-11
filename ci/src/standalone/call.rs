//! Run a tutorial curl command against the gate's ports and decode the reply.
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

pub struct Ports {
    pub based: u16,
    pub edge: u16,
}

impl Ports {
    pub fn apply(&self, args: &[&str]) -> Vec<String> {
        args.iter()
            .map(|arg| {
                arg.replace("localhost:8080", &format!("127.0.0.1:{}", self.edge))
                    .replace("127.0.0.1:9090", &format!("127.0.0.1:{}", self.based))
            })
            .collect()
    }
}

pub fn curl(args: &[&str], ports: &Ports, cwd: &Path) -> Result<(u16, Value)> {
    let mut args = ports.apply(args);
    args.extend(["-w".into(), "\n%{http_code}".into()]);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = crate::command::run(args[0], &args[1..], cwd, &Default::default())?;
    let (body, status) = output
        .trim_end()
        .rsplit_once('\n')
        .with_context(|| format!("no status in {output}"))?;
    let body = serde_json::from_str(body).unwrap_or_else(|_| Value::String(body.into()));
    Ok((status.parse()?, body))
}
