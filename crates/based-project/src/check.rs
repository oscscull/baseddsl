//! Check the project language and manifest-specific target conventions.
use crate::{Error, Report};
use based_ast::Decl;
use based_codegen::Dialect;
use based_manifest::Project;
use based_sema::CheckedSchema;

pub(super) fn schema(
    project: &Project,
    declarations: &mut [Decl],
    report: &mut Report,
) -> Result<CheckedSchema, Error> {
    report
        .diagnostics
        .extend(based_sema::expand_spreads(declarations));
    report.ensure_clean()?;
    let (mut schema, diagnostics) = based_sema::check(declarations);
    report.diagnostics.extend(diagnostics);
    let dialect = Dialect::parse(&project.manifest.dialect);
    report
        .diagnostics
        .extend(based_sema::check_target(&schema, dialect.name()));
    report
        .diagnostics
        .extend(based_codegen::ordered_nest_diagnostics(
            &schema,
            declarations,
            dialect,
        ));
    report.diagnostics.extend(based_sema::check_foreign_keys(
        &schema,
        based_sema::ForeignKeys::parse(&project.manifest.schema.foreign_keys),
    ));
    report.ensure_clean()?;
    based_sema::resolve_pk_default(
        &mut schema,
        based_sema::PkStrategy::parse(&project.manifest.schema.id),
    );
    Ok(schema)
}
