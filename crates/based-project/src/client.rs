//! Translate manifest client options into the existing deterministic emitter.
use based_ast::Decl;
use based_codegen::{
    client::{ClientOptions, ClientTarget},
    Dialect,
};
use based_manifest::{ClientMode, Project};
use based_sema::CheckedSchema;

pub fn render_client(
    project: &Project,
    schema: &CheckedSchema,
    declarations: &[Decl],
    mode: ClientMode,
) -> String {
    // Discovery validates the client target; currently only Rust is supported.
    let embedded = mode == ClientMode::Embedded;
    based_codegen::client::client_with(
        schema,
        declarations,
        ClientTarget::Rust,
        ClientOptions {
            embedded,
            dialect: embedded.then(|| Dialect::parse(&project.manifest.dialect)),
        },
    )
}
