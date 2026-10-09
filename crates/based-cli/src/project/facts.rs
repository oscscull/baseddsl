//! Present compiler-derived facts in the requested terminal format.
use super::load_checked;
use crate::{error::CliError, render};
use std::path::Path;

/// `based facts`: surface the engine-derived facts — the inferred
/// inverse pairings and join-key indexes an editor would show as hints.
pub fn cmd_facts(root: &Path, json: bool) -> Result<(), CliError> {
    let (_project, schema, decls, sources, _warnings) = load_checked(root)?;
    let facts = based_facts::facts(&schema, &decls);
    if json {
        print!("{}", render::facts_json(&facts, &sources));
    } else if facts.is_empty() {
        println!("no derived facts");
    } else {
        print!("{}", render::facts_text(&facts, &sources));
    }
    Ok(())
}
