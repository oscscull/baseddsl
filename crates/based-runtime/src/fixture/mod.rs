//! Typed test fixtures, engine side: insert one row of any model through the structured
//! `create M from $row` path, without exposing a route.
//!
//! Each distinct set of fields a fixture sets compiles once into a private one-mutation
//! schema (an input shape naming exactly those fields, returning the whole model), so
//! unset columns take their database default, and id minting, coercion and
//! `@created`/`@updated` stamping stay the create path's own. Fixtures bypass scopes and
//! guards. The compiled ops live beside the served schema, never in its dispatch tables,
//! so no `/q/` or `/m/` request can reach them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use based_codegen::fixture::fixture_model;

use crate::load::Compiled;
use crate::plan::Request;
use crate::serve::WireResponse;

mod compile;
mod insert;
mod row;
mod source;

/// A model and the sorted fields a fixture row sets.
type OpKey = (String, Vec<String>);

/// The fixture ops compiled so far.
#[derive(Default)]
pub(crate) struct Fixtures {
    compiled: Mutex<HashMap<OpKey, Arc<Compiled>>>,
}

/// A fixture insert ready to run: its private schema and the request for its one mutation.
pub(crate) struct Prepared {
    pub compiled: Arc<Compiled>,
    pub request: Request,
}

/// Validate a fixture row for `model` and find (or compile) the op that inserts it.
pub(crate) fn prepare(
    base: &Compiled,
    model: &str,
    row: serde_json::Value,
) -> Result<Prepared, WireResponse> {
    let Some(rmodel) = base.schema.model(model) else {
        return Err(WireResponse::error(
            404,
            "unknown_fixture",
            format!("no model `{model}` to insert a fixture of"),
        ));
    };
    let fixture = fixture_model(&base.schema, rmodel).map_err(|why| {
        WireResponse::error(
            422,
            "unsupported_fixture",
            format!("`{model}` cannot be a fixture: {why}"),
        )
    })?;
    let keys = row::keys(&fixture, &row)
        .map_err(|message| WireResponse::error(400, "bad_fixture", message))?;
    let compiled = base
        .fixtures
        .get_or_compile(model, keys, |keys| {
            compile::compile(base, &source::source(&fixture, keys))
        })
        .map_err(|message| WireResponse::error(400, "bad_fixture", message))?;
    Ok(Prepared {
        compiled,
        request: Request::new(source::MUTATION, args(row), serde_json::json!({})),
    })
}

/// The op's arguments: the row under its parameter name.
fn args(row: serde_json::Value) -> serde_json::Value {
    let mut args = serde_json::Map::new();
    args.insert(source::PARAM.to_string(), row);
    serde_json::Value::Object(args)
}

impl Fixtures {
    fn get_or_compile(
        &self,
        model: &str,
        keys: Vec<String>,
        compile: impl FnOnce(&[String]) -> Result<Compiled, String>,
    ) -> Result<Arc<Compiled>, String> {
        let mut cache = self
            .compiled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = (model.to_string(), keys);
        if let Some(found) = cache.get(&key) {
            return Ok(found.clone());
        }
        let compiled = Arc::new(compile(&key.1)?);
        cache.insert(key, compiled.clone());
        Ok(compiled)
    }
}
