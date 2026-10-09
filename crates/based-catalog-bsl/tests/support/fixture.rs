//! Independent catalog facts, authored without the emitter or generated BSL.
use based_catalog::*;

pub fn column(
    name: &str,
    position: u32,
    declaration: &str,
    family: TypeFamily,
    nullable: bool,
) -> Column {
    Column {
        name: name.into(),
        position,
        native_type: NativeType::declared(declaration, family),
        nullable,
        default: None,
        collation: None,
        generation: ValueGeneration::None,
    }
}

pub fn table(namespace: &str, name: &str, columns: Vec<Column>, primary: &[&str]) -> Table {
    Table {
        id: TableId::new(namespace, name),
        kind: TableKind::Table,
        columns,
        primary_key: (!primary.is_empty()).then(|| Key {
            name: None,
            columns: primary.iter().map(|name| (*name).into()).collect(),
            deferral: Deferral::NotDeferrable,
        }),
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

pub fn foreign(local: &str, target: &TableId, remote: &str) -> ForeignKey {
    ForeignKey {
        name: None,
        columns: vec![local.into()],
        target: target.clone(),
        target_columns: vec![remote.into()],
        on_delete: ReferentialAction::SetNull,
        on_update: ReferentialAction::Cascade,
        match_mode: MatchMode::Simple,
        deferral: Deferral::NotDeferrable,
    }
}

pub fn catalog(dialect: CatalogDialect) -> Catalog {
    let namespace = if dialect == CatalogDialect::Sqlite {
        "main"
    } else {
        "app"
    };
    let integer = if dialect == CatalogDialect::Sqlite {
        "INTEGER"
    } else {
        "bigint"
    };
    let text = if dialect == CatalogDialect::MariaDb {
        "varchar(255)"
    } else {
        "text"
    };
    let mut parent = table(
        namespace,
        "odd\" table",
        vec![
            column("odd id", 0, integer, TypeFamily::Integer, false),
            column("label", 1, text, TypeFamily::Text, true),
        ],
        &["odd id"],
    );
    parent.columns[0].generation = match dialect {
        CatalogDialect::Sqlite => ValueGeneration::SqliteRowId {
            autoincrement: true,
        },
        CatalogDialect::MariaDb => ValueGeneration::AutoIncrement,
        CatalogDialect::Postgres => ValueGeneration::Identity { always: true },
    };
    parent.columns[1].default = Some(
        if dialect == CatalogDialect::Postgres {
            "'it''s retained'::text"
        } else {
            "'it''s retained'"
        }
        .into(),
    );
    let mut child = table(
        namespace,
        "child",
        vec![
            column("part z", 0, integer, TypeFamily::Integer, false),
            column("part a", 1, integer, TypeFamily::Integer, false),
            column("parent ref", 2, integer, TypeFamily::Integer, true),
        ],
        &["part z", "part a"],
    );
    child
        .foreign_keys
        .push(foreign("parent ref", &parent.id, "odd id"));
    child.unique_keys.push(Key {
        name: None,
        columns: vec!["part a".into(), "part z".into()],
        deferral: Deferral::NotDeferrable,
    });
    child.indexes.push(Index {
        name: "existing_parent_lookup".into(),
        origin: IndexOrigin::Explicit,
        unique: false,
        parts: vec![IndexPart {
            target: IndexTarget::Column("parent ref".into()),
            direction: IndexDirection::Ascending,
            prefix_length: None,
            collation: None,
        }],
        included_columns: Vec::new(),
        method: None,
        predicate: None,
        native_definition: None,
        valid: true,
    });
    let (a, b) = cycle(namespace, text);
    let keyless = table(
        namespace,
        "keyless",
        vec![
            column("id", 0, integer, TypeFamily::Integer, true),
            column("owner_id", 1, integer, TypeFamily::Integer, true),
            column("created_at", 2, text, TypeFamily::Text, true),
        ],
        &[],
    );
    Catalog {
        source: CatalogSource {
            dialect,
            server_version: "independently authored catalog fixture".into(),
            database: "fixture".into(),
        },
        tables: vec![parent, child, a, b, keyless],
    }
}

fn cycle(namespace: &str, text: &str) -> (Table, Table) {
    let mut a = table(
        namespace,
        "cycle_a",
        vec![
            column("code", 0, text, TypeFamily::Text, false),
            column("next", 1, text, TypeFamily::Text, true),
        ],
        &["code"],
    );
    let mut b = table(namespace, "cycle_b", a.columns.clone(), &["code"]);
    a.foreign_keys.push(foreign("next", &b.id, "code"));
    b.foreign_keys.push(foreign("next", &a.id, "code"));
    b.columns
        .push(column("previous", 2, text, TypeFamily::Text, true));
    b.foreign_keys.push(foreign("previous", &b.id, "code"));
    (a, b)
}

pub fn discovery(catalog: Catalog) -> (Discovery, Selection) {
    let selection = Selection::new(catalog.tables.iter().map(|table| table.id.clone())).unwrap();
    (
        Discovery::checked(catalog, &selection, Vec::new()),
        selection,
    )
}

pub fn manifest(dialect: CatalogDialect, foreign_keys: &str) -> based_manifest::Manifest {
    let dialect = match dialect {
        CatalogDialect::MariaDb => "mariadb",
        CatalogDialect::Postgres => "postgres",
        CatalogDialect::Sqlite => "sqlite",
    };
    toml::from_str(&format!(
        "dialect = {dialect:?}\nroot = 'models'\n[schema]\nforeign_keys = {foreign_keys:?}\n"
    ))
    .unwrap()
}
