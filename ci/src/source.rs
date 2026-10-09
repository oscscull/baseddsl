use crate::command::{self, Environment};
use anyhow::Result;
use std::path::Path;

pub fn mirror(directory: &Path) -> Result<Environment> {
    let mirror = directory.join("source.git");
    let commit = command::git(&["rev-parse", "HEAD"])?;
    let empty = Environment::new();
    command::run(
        "git",
        &[
            "init",
            "--bare",
            "--initial-branch=release-source",
            mirror.to_str().unwrap(),
        ],
        directory,
        &empty,
    )?;
    command::run(
        "git",
        &[
            "--git-dir",
            mirror.to_str().unwrap(),
            "fetch",
            "--no-tags",
            command::root().to_str().unwrap(),
            &commit,
        ],
        directory,
        &empty,
    )?;
    command::run(
        "git",
        &[
            "--git-dir",
            mirror.to_str().unwrap(),
            "update-ref",
            "refs/heads/release-source",
            &commit,
        ],
        directory,
        &empty,
    )?;
    let source = url::Url::from_directory_path(&mirror)
        .map_err(|_| anyhow::anyhow!("invalid Git mirror path"))?;
    Ok(Environment::from([
        ("CARGO_NET_GIT_FETCH_WITH_CLI".into(), "true".into()),
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        ("GIT_CONFIG_KEY_0".into(), format!("url.{source}.insteadOf")),
        (
            "GIT_CONFIG_VALUE_0".into(),
            "https://github.com/oscscull/baseddsl.git".into(),
        ),
        (
            "CARGO_TARGET_DIR".into(),
            directory.join("target").display().to_string(),
        ),
    ]))
}
