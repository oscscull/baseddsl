//! Resolve an enum column's bare variant to its wire literal.

use super::super::*;

impl<'a> Select<'a> {
    /// The enum a dotted path terminates on, when the terminal column is enum-typed
    /// (read-only, no join materialized). Lets the caller render a variant RHS as its
    /// wire value.
    fn terminal_enum(&self, path: &Path, model: &RModel) -> Option<&REnum> {
        let mut cur = model;
        let n = path.segments.len();
        for (i, seg) in path.segments.iter().enumerate() {
            let mem = cur.member(&seg.node)?;
            let last = i + 1 == n;
            match &mem.kind {
                MemberKind::Scalar {
                    enum_name: Some(name),
                    ..
                } if last => return self.schema.enum_(name),
                MemberKind::Scalar { .. } => return None,
                MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => {
                    if last {
                        return None;
                    }
                    cur = self.schema.model(target)?;
                }
            }
        }
        None
    }

    /// If `path` names an enum column and `value` is a bare single-segment variant,
    /// its wire value literal (`'paid'` or `2`); else `None` to fall back to value lowering.
    pub(crate) fn enum_variant_lit(
        &self,
        model: &RModel,
        path: &Path,
        value: &Value,
    ) -> Option<String> {
        let en = self.terminal_enum(path, model)?;
        variant_lit(self.dialect, en, value)
    }

    /// If assigning enum column `col_field` a bare single-segment variant, its wire value
    /// literal; else `None`.
    pub(super) fn enum_assign_lit(
        &self,
        model: &RModel,
        col_field: &str,
        value: &Value,
    ) -> Option<String> {
        match model.member(col_field).map(|m| &m.kind) {
            Some(MemberKind::Scalar {
                enum_name: Some(name),
                ..
            }) => {
                let en = self.schema.enum_(name)?;
                variant_lit(self.dialect, en, value)
            }
            _ => None,
        }
    }
}
