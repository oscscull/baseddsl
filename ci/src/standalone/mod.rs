//! Run the standalone tutorial in a clean external project with the given binary.
mod call;
mod process;
mod tutorial;

use crate::command::{self, Environment};
use anyhow::{ensure, Context, Result};
use call::{curl, Ports};
use process::Process;
use serde_json::Value;
use std::path::Path;

pub fn verify(based: &Path) -> Result<()> {
    tutorial::require_documented(&command::root().join("book/src/standalone.md"))?;
    let based = based.canonicalize()?;
    let scratch = tempfile::tempdir()?;
    let env = path_with(&based)?;
    let help = run(&["based", "serve", "--help"], scratch.path(), &env)?;
    ensure!(
        help.contains("X-Based-*") && help.contains("deployment"),
        "serve --help omits the trust requirement"
    );
    let printed = run(tutorial::INIT, scratch.path(), &env)?;
    ensure!(
        printed.contains("node edge/edge.mjs") && !printed.contains("cargo"),
        "standalone next steps need Rust: {printed}"
    );
    let app = scratch.path().join("items");
    ensure!(
        !app.join("Cargo.toml").exists(),
        "standalone starter is a Rust project"
    );
    run(tutorial::MIGRATE, &app, &env)?;
    let ports = Ports {
        based: process::free_port()?,
        edge: process::free_port()?,
    };
    ensure!(ports.based != ports.edge, "port reservation collided");
    let mut service = serve(tutorial::SERVE_FIRST, &ports, &app, &env)?;
    let edge_env = Environment::from([
        ("EDGE_PORT".into(), ports.edge.to_string()),
        (
            "BASED_URL".into(),
            format!("http://127.0.0.1:{}", ports.based),
        ),
    ]);
    let mut edge = Process::start(&ports.apply(tutorial::EDGE), &app, &edge_env)?;
    edge.listening(ports.edge)?;

    let created = expect(curl(tutorial::CREATE, &ports, &app)?, 200)?;
    let id = created["id"]
        .as_str()
        .context("created item has no id")?
        .to_owned();
    ensure!(
        expect(curl(tutorial::CREATE, &ports, &app)?, 200)? == created,
        "keyed retry did not replay"
    );
    ensure!(
        expect(curl(tutorial::FORGED, &ports, &app)?, 200)? == Value::Array(vec![]),
        "forged X-Based-Context crossed the edge"
    );
    expect(curl(tutorial::ANONYMOUS, &ports, &app)?, 401)?;
    let direct = curl(
        &["curl", "-s", "127.0.0.1:9090/q/items", "-d", "{}"],
        &ports,
        &app,
    )?;
    ensure!(direct.0 >= 400, "missing context did not fail: {direct:?}");

    drop(service);
    service = serve(tutorial::SERVE, &ports, &app, &env)?;
    ensure!(
        expect(curl(tutorial::CREATE, &ports, &app)?, 200)? == created,
        "replay did not survive restart"
    );
    let reuse = expect(curl(tutorial::REUSE, &ports, &app)?, 422)?;
    ensure!(
        reuse.to_string().contains("idempotency_key_reuse"),
        "changed payload reused key: {reuse}"
    );
    let rows = expect(curl(tutorial::LIST, &ports, &app)?, 200)?;
    ensure!(
        rows.as_array().map(Vec::len) == Some(1) && rows[0]["id"] == id.as_str(),
        "retries duplicated the write: {rows}"
    );

    drop(service);
    evolve(&app, &env)?;
    let _service = serve(tutorial::SERVE, &ports, &app, &env)?;
    let rows = expect(curl(tutorial::LIST, &ports, &app)?, 200)?;
    ensure!(
        rows[0]["id"] == id.as_str() && rows[0]["note"].is_null(),
        "schema change lost data or field: {rows}"
    );
    Ok(())
}

fn evolve(app: &Path, env: &Environment) -> Result<()> {
    let schema = app.join("schema/item.bsl");
    crate::files::replace(&schema, "  name: text\n", "  name: text\n  note: text?\n")?;
    crate::files::replace(&schema, "  name\n  parent", "  name\n  note\n  parent")?;
    for args in tutorial::EVOLVE {
        run(args, app, env)?;
    }
    run(tutorial::MIGRATE, app, env)?;
    Ok(())
}

fn serve(args: &[&str], ports: &Ports, app: &Path, env: &Environment) -> Result<Process> {
    let mut service = Process::start(&ports.apply(args), app, env)?;
    service.listening(ports.based)?;
    Ok(service)
}

fn run(args: &[&str], cwd: &Path, env: &Environment) -> Result<String> {
    command::run(args[0], &args[1..], cwd, env)
}

fn expect((status, body): (u16, Value), wanted: u16) -> Result<Value> {
    ensure!(
        status == wanted,
        "expected HTTP {wanted}, got {status}: {body}"
    );
    Ok(body)
}

/// Resolve `based` to the binary under test, never a preinstalled one.
fn path_with(based: &Path) -> Result<Environment> {
    let directory = based.parent().context("binary has no directory")?;
    ensure!(
        based.file_stem().is_some_and(|stem| stem == "based"),
        "binary must be named based"
    );
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(directory.to_path_buf()).chain(std::env::split_paths(&inherited)),
    )?;
    Ok(Environment::from([(
        "PATH".into(),
        path.to_string_lossy().into_owned(),
    )]))
}
