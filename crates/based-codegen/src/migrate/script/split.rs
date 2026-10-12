//! Split a legacy raw-SQL `down.mig` into statements.

/// The `;`-separated statements of a raw SQL script, dropping `--`/`#` comment lines and
/// blank fragments.
pub(crate) fn split_sql(script: &str) -> Vec<String> {
    script
        .split(';')
        .map(|frag| {
            frag.lines()
                .filter(|l| {
                    let t = l.trim_start();
                    !t.starts_with("--") && !t.starts_with('#')
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_sql_drops_comments_and_blanks() {
        let script = "-- reverse\nDROP TABLE `widget`;\n# another\nDROP INDEX `x`;\n";
        assert_eq!(
            split_sql(script),
            vec![
                "DROP TABLE `widget`".to_string(),
                "DROP INDEX `x`".to_string()
            ]
        );
    }
}
