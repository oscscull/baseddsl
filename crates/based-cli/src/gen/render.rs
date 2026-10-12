//! Generate one complete artifact from a checked compiler result; no filesystem writes.
use crate::{error::CliError, project::Loaded};
use based_artifacts::Format;
use based_codegen::Dialect;
use based_manifest::ClientMode;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Client,
    Sql,
    OpenApi,
    Fixtures,
}

impl Kind {
    pub(super) fn format(self) -> Format {
        match self {
            Self::Client => Format::Rust,
            Self::Sql => Format::Sql,
            Self::OpenApi => Format::OpenApi,
            Self::Fixtures => Format::TypeScript,
        }
    }
    pub(super) fn configured(self, loaded: &Loaded) -> Option<&str> {
        let config = &loaded.0.manifest.generate;
        match self {
            Self::Client => config.client.as_deref(),
            Self::Sql => config.sql.as_deref(),
            Self::OpenApi => config.openapi.as_deref(),
            Self::Fixtures => None,
        }
    }
}

pub(super) fn render(kind: Kind, loaded: &Loaded, mode: ClientMode) -> Result<String, CliError> {
    let (project, schema, decls, _, _) = loaded;
    let dialect = Dialect::parse(&project.manifest.dialect);
    match kind {
        Kind::Client => Ok(based_project::render_client(project, schema, decls, mode)),
        Kind::Sql => Ok(super::sql::render(
            schema,
            decls,
            dialect,
            based_sema::ForeignKeys::parse(&project.manifest.schema.foreign_keys),
        )),
        Kind::OpenApi => Ok(based_codegen::openapi::openapi(schema, decls)),
        Kind::Fixtures => Ok(based_codegen::client::typescript_fixtures(schema)),
    }
}
