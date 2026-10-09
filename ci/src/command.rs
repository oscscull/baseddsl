use anyhow::{ensure, Context, Result};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Output},
};

pub type Environment = BTreeMap<String, String>;

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
pub fn based() -> PathBuf {
    std::env::var_os("BASED_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            root().join(format!(
                "target/debug/based{}",
                std::env::consts::EXE_SUFFIX
            ))
        })
}
pub fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

pub fn output(
    program: impl AsRef<OsStr>,
    args: &[&str],
    cwd: &Path,
    env: &Environment,
) -> Result<Output> {
    Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env_remove("DATABASE_URL")
        .env_remove("BASED_DATABASE_URL")
        .envs(env)
        .output()
        .context("start verification command")
}

pub fn run(
    program: impl AsRef<OsStr>,
    args: &[&str],
    cwd: &Path,
    env: &Environment,
) -> Result<String> {
    let result = output(program, args, cwd, env)?;
    ensure!(
        result.status.success(),
        "command {:?} failed:\n{}\n{}",
        args,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(String::from_utf8(result.stdout)?)
}

pub fn fail(
    program: impl AsRef<OsStr>,
    args: &[&str],
    cwd: &Path,
    env: &Environment,
) -> Result<String> {
    let result = output(program, args, cwd, env)?;
    ensure!(
        !result.status.success(),
        "command unexpectedly succeeded: {args:?}"
    );
    Ok(format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    ))
}

pub fn git(args: &[&str]) -> Result<String> {
    Ok(run("git", args, &root(), &Environment::new())?
        .trim()
        .into())
}
