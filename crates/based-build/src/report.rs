//! Report a failed generation to Cargo as build errors, one line per directive.
use crate::{Error, Generation};

pub(super) fn report(result: Result<Generation, Error>) -> Option<Generation> {
    match result {
        Ok(generation) => Some(generation),
        Err(error) => {
            for line in error.to_string().lines() {
                println!("cargo::error={line}");
            }
            None
        }
    }
}
