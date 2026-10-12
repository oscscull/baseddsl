use based_ast::{Decl, FileId};
use based_diagnostics::Severity;
use based_sema::{CheckedSchema, MemberKind};

use crate::load::Compiled;

/// A source id no project file uses, so fixture spans never alias a real file.
const FIXTURE_FILE: FileId = FileId(u32::MAX);

/// Compile a fixture op's BSL against the served schema's models, enums and scopes. Errors
/// (e.g. a row missing a required column) come back as the checker's messages.
pub(super) fn compile(base: &Compiled, source: &str) -> Result<Compiled, String> {
    let parsed = based_parser::parse_file(source, FIXTURE_FILE)
        .map_err(|diagnostics| messages(&diagnostics))?;
    let mut decls: Vec<Decl> = base
        .decls
        .iter()
        .filter(|d| matches!(d, Decl::Model(_) | Decl::Enum(_) | Decl::Scope(_)))
        .cloned()
        .collect();
    decls.extend(parsed.decls);
    let (mut schema, diagnostics) = based_sema::check(&decls);
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Err(messages(&diagnostics));
    }
    inherit_column_types(&mut schema, &base.schema);
    Ok(Compiled::from_checked(schema, decls, base.dialect))
}

/// Carry over column types the project resolved after checking (the `Id` strategy).
fn inherit_column_types(schema: &mut CheckedSchema, base: &CheckedSchema) {
    for model in &mut schema.models {
        let Some(base_model) = base.model(&model.name) else {
            continue;
        };
        for member in &mut model.members {
            let Some(MemberKind::Scalar { ty: base_ty, .. }) =
                base_model.member(&member.name).map(|m| &m.kind)
            else {
                continue;
            };
            if let MemberKind::Scalar { ty, .. } = &mut member.kind {
                *ty = *base_ty;
            }
        }
    }
}

fn messages(diagnostics: &[based_diagnostics::Diagnostic]) -> String {
    diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}
