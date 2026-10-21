//! The error for a callable name the project does not declare.
use crate::error::CliError;

/// Names the project's callables, the closest ones first when any resemble `name`.
pub fn unknown_callable(name: &str, names: &[&str]) -> CliError {
    if names.is_empty() {
        return CliError::usage(format!(
            "no query or mutation named `{name}`; the project declares none"
        ));
    }
    let wanted = name.to_lowercase();
    let near: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| {
            let n = n.to_lowercase();
            n.contains(&wanted) || wanted.contains(&n)
        })
        .collect();
    let (heading, list) = match near.is_empty() {
        true => ("callables", names.to_vec()),
        false => ("did you mean", near),
    };
    CliError::usage(format!(
        "no query or mutation named `{name}`; {heading}: {}",
        list.join(", ")
    ))
}
