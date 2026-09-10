use super::*;

/// Every scope-name reference identifier across the AST, with its span — the sites a name
/// *points at* a `scope` decl: `@scope Name[, …]` on a model, `scoped Name[, …]` on a
/// query or mutation. Used for go-to-definition into the `scope` decl.
pub(crate) fn collect_scope_refs(decls: &[Decl]) -> Vec<&Ident> {
    let mut out = Vec::new();
    for d in decls {
        match d {
            Decl::Model(m) => {
                for r in &m.scopes {
                    out.extend(r.names.iter());
                }
            }
            Decl::Query(q) => {
                if let Some(s) = &q.scoped {
                    out.extend(s.names.iter());
                }
            }
            Decl::Mutation(m) => {
                if let Some(s) = &m.scoped {
                    out.extend(s.names.iter());
                }
            }
            _ => {}
        }
    }
    out
}
