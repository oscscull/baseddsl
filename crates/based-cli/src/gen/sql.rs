//! SQL artifact rendering, separate from destinations and file publication.
use based_ast::Decl;
use based_codegen::Dialect;
use based_sema::CheckedSchema;

pub(super) fn render(
    schema: &CheckedSchema,
    decls: &[Decl],
    dialect: Dialect,
    fks: based_sema::ForeignKeys,
) -> String {
    let mut sql = based_codegen::sql::ddl_with(schema, dialect, fks);
    if !schema.queries.is_empty() {
        sql.push_str(
            "\n\n-- ============================== queries ==============================\n",
        );
        sql.push_str(&based_codegen::sql::dml::dml(schema, decls, dialect));
    }
    if !schema.mutations.is_empty() {
        sql.push_str(
            "\n\n-- ============================= mutations =============================\n",
        );
        sql.push_str(&based_codegen::sql::mutations::mutations(
            schema, decls, dialect,
        ));
    }
    sql
}
