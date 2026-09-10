use super::*;

/// A real Rust enum for an `enum` decl. A string enum serde-renames each variant to
/// its wire string (`#[serde(rename = "PAID")] Paid`), so it (de)serializes as that
/// string. An int enum carries explicit discriminants and a hand-rolled Serialize /
/// Deserialize over `i64` — no `serde_repr` dependency; an unknown discriminant decodes
/// to a serde error.
pub(super) fn render_enum(e: &based_sema::REnum) -> String {
    use based_sema::EnumValue;
    match e.kind {
        based_sema::EnumKind::Str => {
            let mut s = format!(
                    "#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]\npub enum {} {{\n",
                    e.name
                );
            for v in &e.variants {
                let wire = match &v.value {
                    EnumValue::Str(w) => w.as_str(),
                    EnumValue::Int(_) => v.name.as_str(),
                };
                s.push_str(&format!(
                    "    #[serde(rename = \"{wire}\")]\n    {},\n",
                    pascal(&v.name)
                ));
            }
            s.push_str("}\n");
            s
        }
        based_sema::EnumKind::Int => render_int_enum(e),
    }
}

/// An int enum: explicit discriminants + a manual serde impl (de)serializing as the
/// integer, with an unknown value surfaced as a decode error.
fn render_int_enum(e: &based_sema::REnum) -> String {
    use based_sema::EnumValue;
    let ints: Vec<(String, i64)> = e
        .variants
        .iter()
        .map(|v| {
            let n = match &v.value {
                EnumValue::Int(n) => *n,
                EnumValue::Str(_) => 0,
            };
            (pascal(&v.name), n)
        })
        .collect();
    let name = &e.name;
    let mut s = format!("#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub enum {name} {{\n");
    for (variant, n) in &ints {
        s.push_str(&format!("    {variant} = {n},\n"));
    }
    s.push_str("}\n");
    s.push_str(&format!(
            "impl serde::Serialize for {name} {{\n    \
             fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {{\n        \
             s.serialize_i64(*self as i64)\n    }}\n}}\n"
        ));
    s.push_str(&format!(
            "impl<'de> serde::Deserialize<'de> for {name} {{\n    \
             fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {{\n        \
             let n = i64::deserialize(d)?;\n        match n {{\n"
        ));
    for (variant, n) in &ints {
        s.push_str(&format!("            {n} => Ok({name}::{variant}),\n"));
    }
    s.push_str(&format!(
        "            other => Err(serde::de::Error::custom(format!(\
             \"invalid {name} value: {{other}}\"))),\n        }}\n    }}\n}}\n"
    ));
    s
}
