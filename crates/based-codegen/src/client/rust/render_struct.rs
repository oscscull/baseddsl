use super::*;

/// A `#[derive(...)] pub struct Name { pub field: Type, … }` block. An empty body
/// renders as a unit-like struct (a callable with no params posts `{}`).
/// An input struct: like [`render_struct`], but an `Option<…>` field is omitted
/// from the wire when `None` (`skip_serializing_if`) so the engine applies the
/// param's declared default — an explicit JSON `null` would suppress it. The
/// `default` twin keeps such a body deserializable with the field absent.
pub(super) fn render_input_struct(name: &str, fields: &[(String, String)]) -> String {
    let mut s = format!("#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct {name}");
    if fields.is_empty() {
        s.push_str(";\n");
        return s;
    }
    s.push_str(" {\n");
    for (f, ty) in fields {
        if ty.starts_with("Option<") {
            s.push_str("    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n");
        }
        s.push_str(&format!("    pub {}: {ty},\n", field_ident(f)));
    }
    s.push_str("}\n");
    s
}

pub(super) fn render_struct(name: &str, fields: &[(String, String)]) -> String {
    let mut s = format!("#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct {name}");
    if fields.is_empty() {
        s.push_str(";\n");
        return s;
    }
    s.push_str(" {\n");
    for (f, ty) in fields {
        if let Some(de) = bool_deserialize_with(ty) {
            s.push_str(&format!("    #[serde(deserialize_with = \"{de}\")]\n"));
        }
        s.push_str(&format!("    pub {}: {ty},\n", field_ident(f)));
    }
    s.push_str("}\n");
    s
}

/// The lenient bool deserializer for a `bool`-typed read field, keyed by its full type
/// string (the closed vocabulary [`wrap`] produces for a `Primitive::Bool`). A boolean
/// column rides the wire as a real JSON bool on Postgres but as `0`/`1` on MySQL and
/// SQLite — where the type is lost at the value level (a stored bool reads back as an
/// integer), so the runtime cannot re-type it. The generated client knows the field is
/// boolean, so it reconciles: accept either form, land a Rust `bool`. `None` for any
/// non-bool type (which keeps its plain `#[derive(Deserialize)]`).
fn bool_deserialize_with(ty: &str) -> Option<&'static str> {
    match ty {
        "bool" => Some("de_bool"),
        "Option<bool>" => Some("de_bool_opt"),
        "Vec<bool>" => Some("de_bool_vec"),
        "Option<Vec<bool>>" => Some("de_bool_opt_vec"),
        _ => None,
    }
}
