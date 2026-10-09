//! Real SQLite schema and keyed mutation fixture.
use based_runtime::{Compiled, Guards, MemStore, SeqIdGen, SqliteBackend, WireResponse};
use serde_json::{json, Value};

pub struct Fixture {
    pub compiled: Compiled,
    pub backend: SqliteBackend,
    pub store: MemStore,
    ids: SeqIdGen,
}

impl Fixture {
    pub async fn new() -> Self {
        let source = r#"
            Item { id: Id, name: text }
            mutation create_item(name) -> Item guard can_create { create Item { name = $name }; }
            query items() -> Item[] { list Item; }
        "#;
        let file = based_parser::parse_file(source, based_ast::FileId(0)).unwrap();
        let (schema, diagnostics) = based_sema::check(&file.decls);
        assert!(!diagnostics
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error));
        let compiled = Compiled::from_checked(schema, file.decls, based_codegen::Dialect::Sqlite);
        let backend = SqliteBackend::in_memory().unwrap();
        backend
            .execute_batch(&based_codegen::sql::ddl(&compiled.schema, compiled.dialect))
            .await
            .unwrap();
        Self {
            compiled,
            backend,
            store: MemStore::new(),
            ids: SeqIdGen::default(),
        }
    }

    pub async fn create(&self, guards: &Guards, args: Value) -> WireResponse {
        based_runtime::dispatch(
            &self.compiled,
            &self.backend,
            "",
            &self.ids,
            &self.store,
            guards,
            None,
            "POST",
            "/m/create_item",
            args,
            json!({"user":"trusted-user"}),
            Some("same-key".into()),
        )
        .await
    }

    pub async fn rows(&self) -> Value {
        based_runtime::dispatch(
            &self.compiled,
            &self.backend,
            "",
            &self.ids,
            &self.store,
            &Guards::new(),
            None,
            "POST",
            "/q/items",
            json!({}),
            json!({}),
            None,
        )
        .await
        .body
    }
}
