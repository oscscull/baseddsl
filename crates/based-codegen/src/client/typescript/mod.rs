//! The TypeScript test-fixture module (`based gen fixtures --lang ts`): the same builders
//! as the Rust fixtures, inserting through a `based fixtures` child process.

use crate::fixture::{fixture_models, FieldValue, FixtureField, FixtureModel, Placeholder, Role};
use based_sema::{CheckedSchema, MemberKind, RMember, RModel};

mod builder;
mod helpers;
mod names;
mod placeholder;
mod prelude;
mod row;
mod types;

use builder::*;
use helpers::*;
use names::*;
use placeholder::*;
use prelude::*;
use row::*;
use types::*;

/// Render the whole TypeScript fixtures module.
pub fn typescript_fixtures(schema: &CheckedSchema) -> String {
    let fixtures = fixture_models(schema);
    let mut out = String::from(HEADER);
    for e in &schema.enums {
        out.push_str(&render_enum(e));
    }
    for model in schema.models.iter().filter(|m| m.is_composite_key()) {
        out.push_str(&render_composite_id(schema, model));
    }
    for model in schema
        .models
        .iter()
        .filter(|m| !m.pk_field_names().is_empty())
    {
        out.push_str(&render_ref(schema, model));
    }
    for fixture in &fixtures {
        out.push_str(&render_row(schema, fixture.model));
        out.push_str(&render_builder(schema, fixture));
    }
    out.push_str(&render_helpers(&fixtures));
    out.push_str(CONNECTION);
    out
}
