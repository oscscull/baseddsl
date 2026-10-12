use based_codegen::fixture::{FieldValue, FixtureModel};

/// The one mutation a fixture schema declares.
pub(super) const MUTATION: &str = "based_fixture";
/// Its row parameter.
pub(super) const PARAM: &str = "fixture";
const SHAPE: &str = "BasedFixtureInput";

/// The BSL for a fixture op: an input shape naming exactly `keys`, and an unscoped
/// structured create of the model that reads the whole row back.
pub(super) fn source(fixture: &FixtureModel<'_>, keys: &[String]) -> String {
    let model = &fixture.model.name;
    let mut body = String::new();
    for key in keys {
        if let Some(FieldValue::Link { target, .. }) = fixture.field(key).map(|f| &f.value) {
            let parts = target.pk_field_names().join(", ");
            body.push_str(&format!("  {key} {{ {parts} }}\n"));
        } else {
            body.push_str(&format!("  {key}\n"));
        }
    }
    format!(
        "shape {SHAPE} from {model} {{\n{body}}}\n\
         mutation {MUTATION}({PARAM}: {SHAPE}) -> {model} unscoped(\"fixture\") {{\n  \
         create {model} from ${PARAM};\n}}\n"
    )
}
