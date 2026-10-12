//! Typed test fixtures: which columns a fixture builder sets, and how.
//!
//! One shared reading of each model serves the engine (which compiles a fixture insert)
//! and the Rust and TypeScript emitters (which generate the builders), so the rules are
//! identical on every side. Required scalars with no default get a placeholder; required
//! relations are constructor arguments; everything else is optional and, left unset,
//! takes its NULL or database default.

use based_sema::{CheckedSchema, RMember, RModel};

mod classify;
mod placeholder;

pub use placeholder::Placeholder;

/// A model that can be inserted as a fixture, with its settable fields in member order.
pub struct FixtureModel<'s> {
    pub model: &'s RModel,
    pub fields: Vec<FixtureField<'s>>,
}

/// One settable field of a fixture.
pub struct FixtureField<'s> {
    pub name: &'s str,
    pub role: Role,
    pub value: FieldValue<'s>,
}

/// How a fixture fills a field it was not given.
#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    /// Required with no default: the builder fills a placeholder.
    Placeholder(Placeholder),
    /// A required relation: a constructor argument, never invented.
    Argument,
    /// Unset means NULL, the database default, or an engine-managed value.
    Optional,
}

/// What a field holds.
pub enum FieldValue<'s> {
    /// A stored column.
    Scalar(&'s RMember),
    /// A to-one relation, written as the target row's key.
    Link { target: &'s RModel, optional: bool },
}

impl FixtureModel<'_> {
    pub fn field(&self, name: &str) -> Option<&FixtureField<'_>> {
        self.fields.iter().find(|f| f.name == name)
    }
}

/// Every model a fixture can insert, in schema order.
pub fn fixture_models(schema: &CheckedSchema) -> Vec<FixtureModel<'_>> {
    schema
        .models
        .iter()
        .filter_map(|m| fixture_model(schema, m).ok())
        .collect()
}

/// The fixture reading of one model, or why it cannot be a fixture.
pub fn fixture_model<'s>(
    schema: &'s CheckedSchema,
    model: &'s RModel,
) -> Result<FixtureModel<'s>, String> {
    let mut fields = Vec::new();
    for member in &model.members {
        if let Some(field) = classify::classify(schema, model, member)? {
            fields.push(field);
        }
    }
    classify::check_readback_key(model, &fields)?;
    Ok(FixtureModel { model, fields })
}
