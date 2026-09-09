use super::*;

/// True when every field of a relation block names one of the target's key parts (a bare
/// column or a single-column rename) — an FK link to an existing row. An empty block is a
/// create (all-default/engine-filled columns), so it does not count as a link.
pub fn is_key_link(body: &[ShapeField], key_fields: &[String]) -> bool {
    !body.is_empty()
        && body.iter().all(|f| {
            let name = match f {
                ShapeField::Bare(id) => &id.node,
                ShapeField::Rename {
                    value: ShapeValue::Path(p),
                    ..
                } if p.segments.len() == 1 => &p.segments[0].node,
                _ => return false,
            };
            key_fields.iter().any(|k| k == name)
        })
}
