//! `based explain`: print the statements a callable executes, as the driver receives them.
pub mod options;
mod render;
mod unknown;

use crate::{dialect_flag, error::CliError, project::load_checked, project_root};
use based_runtime::{explain, Compiled};
use options::ExplainOptions;

pub fn cmd_explain(options: ExplainOptions) -> Result<(), CliError> {
    let flag = options.dialect.clone();
    let (root, callable) = options.root_and_callable();
    let (project, schema, decls, _sources, _warnings) =
        load_checked(&project_root::resolve(root.as_deref())?)?;
    let dialect = dialect_flag::resolve(flag.as_deref(), &project.manifest.dialect)?;
    let compiled = Compiled::from_checked(schema, decls, dialect);
    let explained = match callable {
        Some(name) => {
            let found = explain::explain(&compiled, &name);
            if found.is_empty() {
                let names = explain::callable_names(&compiled);
                return Err(unknown::unknown_callable(&name, &names));
            }
            found
        }
        None => explain::explain_all(&compiled),
    };
    let text: Vec<String> = explained
        .iter()
        .map(|e| render::callable(e, dialect))
        .collect();
    print!("{}", text.join("\n"));
    Ok(())
}
