//! `based check` and `based fmt`: parse + typecheck the project, or rewrite its
//! `.bsl` files in the canonical layout.

use crate::error::{io_at, CliError};
use crate::project::{discover_project, load_checked};
use crate::render;
use std::path::Path;

pub fn cmd_check(root: &Path) -> Result<(), CliError> {
    let (_project, schema, _decls, _sources, warnings) = load_checked(root)?;
    let n = schema.models.len();
    if warnings > 0 {
        println!("ok with warnings: {warnings} warning(s) across {n} model(s)");
    } else {
        println!("ok: {n} model(s) checked clean");
    }
    Ok(())
}

/// `based fmt [--check]`: rewrite every discovered `.bsl` file in the canonical layout.
/// Without `--check` it writes each changed file in place; with `--check` it writes
/// nothing and exits nonzero if any file is not already formatted. A file that doesn't
/// parse can't be formatted — its diagnostics are framed rustc-style and the run fails.
pub fn cmd_fmt(root: &Path, check: bool) -> Result<(), CliError> {
    let project = discover_project(root)?;

    let mut changed = 0usize;
    let mut unparsed = 0usize;
    for f in &project.files {
        let src = std::fs::read_to_string(&f.path).map_err(|e| io_at("reading", &f.path, e))?;
        match based_fmt::format_source(&src) {
            Ok(formatted) => {
                if formatted == src {
                    continue;
                }
                changed += 1;
                if check {
                    eprintln!("would reformat {}", f.path.display());
                } else {
                    std::fs::write(&f.path, &formatted)
                        .map_err(|e| io_at("writing", &f.path, e))?;
                    eprintln!("formatted {}", f.path.display());
                }
            }
            Err(diags) => {
                unparsed += 1;
                render::render(&diags, &[(f.path.clone(), src)]);
            }
        }
    }

    if unparsed > 0 {
        return Err(CliError::summary(
            false,
            format!("fmt failed: {unparsed} file(s) with parse errors (see above)"),
        ));
    }
    if check && changed > 0 {
        return Err(CliError::summary(
            false,
            format!("{changed} file(s) not formatted — run `based fmt`"),
        ));
    }
    let n = project.files.len();
    if changed == 0 {
        println!("ok: {n} file(s) already formatted");
    } else {
        println!("reformatted {changed} of {n} file(s)");
    }
    Ok(())
}
