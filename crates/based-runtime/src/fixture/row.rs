use based_codegen::fixture::{FieldValue, FixtureModel};

/// The fields a fixture row sets, sorted. Each must be a settable field of the model, and
/// a relation must carry its target's key object (or `null`).
pub(super) fn keys(
    fixture: &FixtureModel<'_>,
    row: &serde_json::Value,
) -> Result<Vec<String>, String> {
    let model = &fixture.model.name;
    let object = row
        .as_object()
        .ok_or_else(|| format!("a `{model}` fixture row must be a JSON object"))?;
    let mut keys = Vec::with_capacity(object.len());
    for (key, value) in object {
        let field = fixture
            .field(key)
            .ok_or_else(|| format!("`{model}` has no field `{key}` a fixture can set"))?;
        if matches!(field.value, FieldValue::Link { .. }) && !(value.is_object() || value.is_null())
        {
            return Err(format!(
                "`{model}.{key}` is a relation: pass the target's key object, e.g. {{\"id\": …}}"
            ));
        }
        keys.push(key.clone());
    }
    keys.sort();
    Ok(keys)
}
