//! The Rust test-fixture builders: a `fixtures` module in every client, one builder per
//! model, inserted through an engine-backed client or a `based fixtures` process.

use super::*;
use crate::fixture::{fixture_models, FieldValue, FixtureField, FixtureModel, Placeholder, Role};
use based_sema::RMember;

mod builder;
mod placeholder;
mod prelude;
mod row;
mod setter;

use builder::*;
use placeholder::*;
use prelude::*;
use row::*;
use setter::*;

/// The `pub mod fixtures { … }` block appended to a client; `embedded` adds the
/// engine-backed transports.
pub(crate) fn render_fixtures(schema: &CheckedSchema, embedded: bool) -> String {
    let mut out = String::from(FIXTURES_HEAD);
    if embedded {
        out.push_str(FIXTURES_EMBEDDED);
    }
    for model in schema.models.iter().filter(|m| m.is_composite_key()) {
        out.push_str(&composite_self_ref(&model.name));
    }
    for fixture in fixture_models(schema) {
        out.push_str(&render_row(schema, fixture.model));
        out.push_str(&render_builder(schema, &fixture));
    }
    out.push_str(FIXTURES_TAIL);
    out
}

/// The key type that names one row of `model`.
fn key_type(schema: &CheckedSchema, model: &RModel) -> String {
    id_type(schema, &model.name)
}
