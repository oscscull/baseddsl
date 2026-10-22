use anyhow::{anyhow, Result};
use std::fs;

/// The `## VERSION` section of the book's release notes, with a link to the upgrade steps.
pub fn for_version(version: &str) -> Result<String> {
    let notes = fs::read_to_string(crate::command::root().join("book/src/release-notes.md"))?;
    let body = section(&notes, version)
        .ok_or_else(|| anyhow!("book/src/release-notes.md has no `## {version}` section"))?;
    Ok(format!(
        "{body}\nUpgrade steps: https://github.com/oscscull/baseddsl/blob/v{version}/book/src/upgrading.md\n"
    ))
}

fn section(notes: &str, version: &str) -> Option<String> {
    let heading = format!("## {version}");
    let mut lines = notes.lines().skip_while(|line| line.trim_end() != heading);
    let first = lines.next()?;
    let rest = lines.take_while(|line| !line.starts_with("## "));
    let body = std::iter::once(first).chain(rest).collect::<Vec<_>>();
    Some(format!("{}\n", body.join("\n").trim_end()))
}

#[cfg(test)]
mod tests {
    use super::section;

    const NOTES: &str =
        "# Release notes\n\n## Unreleased\n\n- next\n\n## 0.1.2\n\n- two\n\n## 0.1.1\n\n- one\n";

    #[test]
    fn takes_only_the_requested_version() {
        assert_eq!(section(NOTES, "0.1.2").unwrap(), "## 0.1.2\n\n- two\n");
        assert_eq!(section(NOTES, "0.1.1").unwrap(), "## 0.1.1\n\n- one\n");
    }

    #[test]
    fn rejects_a_missing_or_prefix_version() {
        assert!(section(NOTES, "0.1.3").is_none());
        assert!(section(NOTES, "0.1").is_none());
    }

    #[test]
    fn current_workspace_version_has_notes() {
        let manifest: toml::Value = toml::from_str(
            &std::fs::read_to_string(crate::command::root().join("Cargo.toml")).unwrap(),
        )
        .unwrap();
        let version = manifest["workspace"]["package"]["version"]
            .as_str()
            .unwrap();
        super::for_version(version).unwrap();
    }
}
