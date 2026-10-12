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
    based_codegen::client::client_with(
        schema,
        declarations,
        ClientTarget::Rust,
        options(project, mode),
    )
}

/// The client `build.rs` generates: an embedded client also carries its test fixtures.
pub fn render_build_client(
    project: &Project,
    schema: &CheckedSchema,
    declarations: &[Decl],
    mode: ClientMode,
) -> String {
    based_codegen::client::client_with_fixtures(
        schema,
        declarations,
        ClientTarget::Rust,
        options(project, mode),
    )
}

fn options(project: &Project, mode: ClientMode) -> ClientOptions {
    let embedded = mode == ClientMode::Embedded;
    ClientOptions {
        embedded,
        dialect: embedded.then(|| Dialect::parse(&project.manifest.dialect)),
    }
}
