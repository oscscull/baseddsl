//! Shared physical fixtures for catalog invariants, independent of dialect SQL adapters.
use based_catalog::*;

pub fn column(name: &str, position: u32) -> Column {
    Column {
        name: name.into(),
        position,
        native_type: NativeType::declared("BIGINT", TypeFamily::Integer),
        nullable: false,
        default: None,
        collation: None,
        generation: ValueGeneration::None,
    }
}

pub fn key(columns: &[&str]) -> Key {
    Key {
        name: None,
        columns: columns.iter().map(|name| (*name).to_owned()).collect(),
        deferral: Deferral::NotDeferrable,
    }
}

pub fn table(name: &str) -> Table {
    Table {
        id: TableId::new("public", name),
        kind: TableKind::Table,
        columns: vec![column("seq", 2), column("owner", 1)],
        primary_key: Some(key(&["owner", "seq"])),
        unique_keys: Vec::new(),
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        checks: Vec::new(),
        native_definition: None,
        engine: None,
        without_rowid: false,
        strict: false,
    }
}

pub fn relation(target: &str) -> ForeignKey {
    ForeignKey {
        name: Some("declared_relation".into()),
        columns: vec!["owner".into(), "seq".into()],
        target: TableId::new("public", target),
        target_columns: vec!["owner".into(), "seq".into()],
        on_delete: ReferentialAction::Restrict,
        on_update: ReferentialAction::Cascade,
        match_mode: MatchMode::Simple,
        deferral: Deferral::NotDeferrable,
    }
}

pub fn catalog(tables: Vec<Table>) -> Catalog {
    Catalog {
        source: CatalogSource {
            dialect: CatalogDialect::Postgres,
            server_version: "16 fixture".into(),
            database: "catalog_fixture".into(),
        },
        tables,
    }
}

pub fn selection(names: &[&str]) -> Selection {
    Selection::new(names.iter().map(|name| TableId::new("public", *name))).unwrap()
}
