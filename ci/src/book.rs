//! Require a book page to show the commands and files a gate exercises, verbatim.
use anyhow::{ensure, Result};
use std::path::Path;

/// Fail when the page no longer shows a command the gate runs.
pub fn require_commands(page: &Path, commands: &[&[&str]]) -> Result<()> {
    let text = std::fs::read_to_string(page)?.replace("\\\n", " ");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for args in commands {
        let line = shell(args);
        ensure!(
            text.contains(&line),
            "{} no longer shows: {line}",
            page.display()
        );
    }
    Ok(())
}

/// Fail when the page no longer shows a file's full text.
pub fn require_files(page: &Path, files: &[&Path]) -> Result<()> {
    let text = std::fs::read_to_string(page)?;
    for file in files {
        ensure!(
            text.contains(&std::fs::read_to_string(file)?),
            "{} no longer shows {}",
            page.display(),
            file.display()
        );
    }
    Ok(())
}

fn shell(args: &[&str]) -> String {
    args.iter()
        .map(|arg| {
            let plain = arg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:=".contains(c));
            if plain {
                arg.to_string()
            } else {
                format!("'{arg}'")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
