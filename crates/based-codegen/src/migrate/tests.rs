//! Migration engine tests (round-trip, diff, per-dialect SQL, content hash).

use super::*;

fn col(name: &str, ty: &str, nullable: bool) -> ColumnSnap {
    ColumnSnap {
        name: name.to_string(),
        ty: ty.to_string(),
        nullable,
        default: None,
        unique: false,
        fk: None,
    }
}

fn table(name: &str, columns: Vec<ColumnSnap>) -> TableSnap {
    TableSnap {
        name: name.to_string(),
        soft_delete: None,
        created: None,
        updated: None,
        scope_alts: Vec::new(),
        sort: Vec::new(),
        columns,
        indexes: Vec::new(),
    }
}

fn scope_decl(name: &str, terms: &[(&str, &str, &str)]) -> ScopeDeclSnap {
    ScopeDeclSnap {
        name: name.to_string(),
        terms: terms
            .iter()
            .map(|(c, t, f)| ScopeTermSnap {
                column: c.to_string(),
                ty: t.to_string(),
                ctx_field: f.to_string(),
            })
            .collect(),
    }
}

#[test]
fn render_then_parse_round_trips_every_attribute() {
    let snap = Snapshot {
        scopes: vec![scope_decl("Tenant", &[("org", "Org", "org")])],
        tables: vec![TableSnap {
            name: "order".to_string(),
            soft_delete: Some(("deleted_at".to_string(), "timestamp".to_string())),
            created: Some("created_at".to_string()),
            updated: Some("updated_at".to_string()),
            scope_alts: vec![vec!["Tenant".to_string()]],
            sort: vec![("placed_at".to_string(), "desc".to_string())],
            columns: vec![
                ColumnSnap {
                    name: "status".to_string(),
                    ty: "text".to_string(),
                    nullable: false,
                    default: Some("\"pending\"".to_string()),
                    unique: false,
                    fk: None,
                },
                ColumnSnap {
                    name: "org_id".to_string(),
                    ty: "uuid".to_string(),
                    nullable: false,
                    default: None,
                    unique: false,
                    fk: Some("Org".to_string()),
                },
            ],
            indexes: vec![IndexSnap {
                name: "idx_order_status".to_string(),
                columns: vec!["status".to_string()],
                unique: false,
            }],
        }],

        renames: Vec::new(),
    };
    let text = snap.render();
    let parsed = Snapshot::parse(&text).expect("parse round-trip");
    assert_eq!(snap, parsed, "\n{text}");
}

#[test]
fn parse_tolerates_a_quoted_default_with_spaces() {
    let mut c = col("label", "text", false);
    c.default = Some("\"in progress\"".to_string());
    let snap = Snapshot {
        scopes: Vec::new(),
        tables: vec![table("job", vec![c])],

        renames: Vec::new(),
    };
    let parsed = Snapshot::parse(&snap.render()).expect("parse");
    assert_eq!(snap, parsed);
}

#[test]
fn init_diff_from_empty_is_a_full_create_set() {
    let now = Snapshot {
        scopes: Vec::new(),
        tables: vec![
            table("a", vec![col("x", "int", false)]),
            table("b", vec![col("y", "text", true)]),
        ],

        renames: Vec::new(),
    };
    let steps = diff_snapshots(&Snapshot::default(), &now);
    assert_eq!(steps.len(), 2);
    assert!(matches!(&steps[0], Step::CreateTable(t) if t.name == "a"));
    assert!(matches!(&steps[1], Step::CreateTable(t) if t.name == "b"));
}

#[test]
fn add_column_and_add_index_between_versions() {
    let prev = Snapshot {
        scopes: Vec::new(),
        tables: vec![table("product", vec![col("name", "text", false)])],

        renames: Vec::new(),
    };
    let mut now_t = table(
        "product",
        vec![col("name", "text", false), col("barcode", "text", true)],
    );
    now_t.indexes.push(IndexSnap {
        name: "idx_product_barcode".to_string(),
        columns: vec!["barcode".to_string()],
        unique: false,
    });
    let now = Snapshot {
        scopes: Vec::new(),
        tables: vec![now_t],

        renames: Vec::new(),
    };
    let steps = diff_snapshots(&prev, &now);
    assert_eq!(steps.len(), 2);
    assert!(matches!(&steps[0], Step::AddColumn { column, .. } if column.name == "barcode"));
    assert!(
        matches!(&steps[1], Step::AddIndex { index, .. } if index.name == "idx_product_barcode")
    );
    // Neither a nullable add nor a plain index is destructive.
    assert!(steps.iter().all(|s| !s.destructive()));
}

#[test]
fn dropping_a_column_and_a_table_is_destructive() {
    let prev = Snapshot {
        scopes: Vec::new(),
        tables: vec![
            table("keep", vec![col("a", "int", false), col("b", "int", false)]),
            table("gone", vec![col("c", "int", false)]),
        ],

        renames: Vec::new(),
    };
    let now = Snapshot {
        scopes: Vec::new(),
        tables: vec![table("keep", vec![col("a", "int", false)])],

        renames: Vec::new(),
    };
    let steps = diff_snapshots(&prev, &now);
    // drop column keep.b + drop table gone.
    let drops: Vec<_> = steps.iter().filter(|s| s.destructive()).collect();
    assert_eq!(drops.len(), 2);
    assert!(steps
        .iter()
        .any(|s| matches!(s, Step::DropColumn { column, .. } if column == "b")));
    assert!(steps
        .iter()
        .any(|s| matches!(s, Step::DropTable(n) if n == "gone")));
}

#[test]
fn narrowing_type_and_new_not_null_without_default_are_destructive() {
    let prev = Snapshot {
        scopes: Vec::new(),
        tables: vec![table("t", vec![col("v", "text", true)])],

        renames: Vec::new(),
    };
    // text -> int (narrowing) AND null -> not_null with no default.
    let now = Snapshot {
        scopes: Vec::new(),
        tables: vec![table("t", vec![col("v", "int", false)])],

        renames: Vec::new(),
    };
    let steps = diff_snapshots(&prev, &now);
    assert_eq!(steps.len(), 1);
    assert!(steps[0].destructive(), "{:?}", steps[0]);

    // The inverse — widening int -> text and relaxing not_null -> null — is safe.
    let prev2 = Snapshot {
        scopes: Vec::new(),
        tables: vec![table("t", vec![col("v", "int", false)])],

        renames: Vec::new(),
    };
    let now2 = Snapshot {
        scopes: Vec::new(),
        tables: vec![table("t", vec![col("v", "text", true)])],

        renames: Vec::new(),
    };
    let steps2 = diff_snapshots(&prev2, &now2);
    assert_eq!(steps2.len(), 1);
    assert!(!steps2[0].destructive(), "{:?}", steps2[0]);
}

#[test]
fn adding_a_unique_index_is_destructive_over_existing_data() {
    let prev = Snapshot {
        scopes: Vec::new(),
        tables: vec![table("t", vec![col("email", "text", false)])],

        renames: Vec::new(),
    };
    let mut now_t = table("t", vec![col("email", "text", false)]);
    now_t.indexes.push(IndexSnap {
        name: "uq_t_email".to_string(),
        columns: vec!["email".to_string()],
        unique: true,
    });
    let now = Snapshot {
        scopes: Vec::new(),
        tables: vec![now_t],

        renames: Vec::new(),
    };
    let steps = diff_snapshots(&prev, &now);
    assert_eq!(steps.len(), 1);
    assert!(matches!(&steps[0], Step::AddUnique { .. }));
    assert!(steps[0].destructive());
}

#[test]
fn no_changes_yields_no_steps() {
    let snap = Snapshot {
        scopes: Vec::new(),
        tables: vec![table("t", vec![col("a", "int", false)])],

        renames: Vec::new(),
    };
    assert!(diff_snapshots(&snap, &snap).is_empty());
}

/// A multi-alternative (OR) scope round-trips through render/parse and its addition,
/// term change, and a model joining a second alternative each surface as diff steps.
#[test]
fn multi_alternative_scope_serializes_parses_and_diffs() {
    // `Post` is scoped either by page OR by author — two stacked `@scope` decorators.
    let mut post = table("post", vec![col("body", "text", false)]);
    post.columns.push(col("page_id", "uuid", false));
    post.columns.push(col("author_id", "uuid", false));
    post.scope_alts = vec![vec!["Author".to_string()], vec!["Page".to_string()]];
    let now = Snapshot {
        scopes: vec![
            scope_decl("Author", &[("author", "User", "user")]),
            scope_decl("Page", &[("page", "Page", "page")]),
        ],
        tables: vec![post.clone()],

        renames: Vec::new(),
    };

    // Round-trip: both the OR alternatives and the two top-level decls survive.
    let text = now.render();
    assert!(
        text.contains("scope Author (author: User = $ctx.user)"),
        "\n{text}"
    );
    assert!(
        text.contains("scope Page (page: Page = $ctx.page)"),
        "\n{text}"
    );
    assert!(text.contains("scope=(Author) scope=(Page)"), "\n{text}");
    assert_eq!(Snapshot::parse(&text).expect("round-trip"), now, "\n{text}");

    // From a prior with only the `Page` scope + a `Post` scoped by page alone: adding
    // the `Author` scope decl and Post joining it both surface (no DDL, but stepped).
    let mut prev_post = table("post", vec![col("body", "text", false)]);
    prev_post.columns.push(col("page_id", "uuid", false));
    prev_post.columns.push(col("author_id", "uuid", false));
    prev_post.scope_alts = vec![vec!["Page".to_string()]];
    let prev = Snapshot {
        scopes: vec![scope_decl("Page", &[("page", "Page", "page")])],
        tables: vec![prev_post],

        renames: Vec::new(),
    };
    let steps = diff_snapshots(&prev, &now);
    assert!(
        steps
            .iter()
            .any(|s| matches!(s, Step::ScopeChange(ScopeChange::Add(d)) if d.name == "Author")),
        "{steps:?}"
    );
    assert!(
        steps.iter().any(
            |s| matches!(s, Step::ScopeChange(ScopeChange::Table { table, .. }) if table == "post")
        ),
        "{steps:?}"
    );
    // Scope changes are code-level, never destructive, and emit no SQL.
    assert!(steps.iter().all(|s| !s.destructive()));
    assert!(sql_statements(&steps, MariaDb).unwrap().is_empty());

    // Dropping a scope and retyping a term both surface too.
    let mut now2 = now.clone();
    now2.scopes[0].terms[0].ctx_field = "actor".to_string(); // Author term retyped
    now2.scopes.remove(1); // Page dropped
    now2.tables[0].scope_alts = vec![vec!["Author".to_string()]];
    let steps2 = diff_snapshots(&now, &now2);
    assert!(steps2
        .iter()
        .any(|s| matches!(s, Step::ScopeChange(ScopeChange::Alter(d)) if d.name == "Author")));
    assert!(steps2
        .iter()
        .any(|s| matches!(s, Step::ScopeChange(ScopeChange::Drop(n)) if n == "Page")));
}

/// A from-scratch `0001_init` stays create-only even with scopes: the scope contract
/// rides `schema.snap` + each `CreateTable`'s `scope_alts`, so no `ScopeChange` steps.
#[test]
fn init_diff_omits_scope_change_steps() {
    let mut t = table("post", vec![col("body", "text", false)]);
    t.scope_alts = vec![vec!["Page".to_string()]];
    let now = Snapshot {
        scopes: vec![scope_decl("Page", &[("page", "Page", "page")])],
        tables: vec![t],

        renames: Vec::new(),
    };
    let steps = diff_snapshots(&Snapshot::default(), &now);
    assert!(
        steps.iter().all(|s| matches!(s, Step::CreateTable(_))),
        "{steps:?}"
    );
}

// ---- per-dialect SQL rendering --------------------------------------

use crate::Dialect::{MariaDb, Postgres, Sqlite};

#[test]
fn create_table_renders_id_pk_and_types_per_dialect() {
    // A nullable column, a not-null column, and a unique column exercise the type
    // map, nullability, and the `(unique)` constraint across all three dialects.
    let mut email = col("email", "text", false);
    email.unique = true;
    let t = table("account", vec![email, col("age", "int", true)]);
    let steps = vec![Step::CreateTable(t)];

    let maria = render_sql(&steps, MariaDb);
    assert!(maria.contains("CREATE TABLE `account` ("), "\n{maria}");
    assert!(maria.contains("`id` UUID NOT NULL"), "\n{maria}");
    assert!(maria.contains("`email` VARCHAR(255) NOT NULL"), "\n{maria}");
    assert!(maria.contains("`age` BIGINT NULL"), "\n{maria}");
    assert!(maria.contains("PRIMARY KEY (`id`)"), "\n{maria}");
    assert!(
        maria.contains("CONSTRAINT `uq_account_email` UNIQUE (`email`)"),
        "\n{maria}"
    );

    let pg = render_sql(&steps, Postgres);
    assert!(pg.contains("CREATE TABLE \"account\" ("), "\n{pg}");
    assert!(pg.contains("\"id\" UUID NOT NULL"), "\n{pg}");
    assert!(pg.contains("\"email\" TEXT NOT NULL"), "\n{pg}");
    assert!(pg.contains("\"age\" BIGINT NULL"), "\n{pg}");

    let sqlite = render_sql(&steps, Sqlite);
    assert!(sqlite.contains("`id` TEXT NOT NULL"), "\n{sqlite}");
    assert!(sqlite.contains("`age` INTEGER NULL"), "\n{sqlite}");
}

#[test]
fn create_table_indexes_inline_on_mariadb_standalone_elsewhere() {
    let mut t = table("item", vec![col("sku", "text", false)]);
    t.indexes.push(IndexSnap {
        name: "idx_item_sku".to_string(),
        columns: vec!["sku".to_string()],
        unique: false,
    });
    let steps = vec![Step::CreateTable(t)];

    // MariaDB carries the index inline as a `KEY` clause inside the CREATE TABLE.
    let maria = render_sql(&steps, MariaDb);
    assert!(maria.contains("KEY `idx_item_sku` (`sku`)"), "\n{maria}");
    assert!(!maria.contains("CREATE INDEX"), "\n{maria}");

    // Postgres/SQLite trail it as a separate CREATE INDEX statement.
    let pg = render_sql(&steps, Postgres);
    assert!(
        pg.contains("CREATE INDEX \"idx_item_sku\" ON \"item\" (\"sku\");"),
        "\n{pg}"
    );
}

#[test]
fn add_column_and_string_default_render() {
    let mut c = col("status", "text", false);
    c.default = Some("\"pending\"".to_string());
    let steps = vec![Step::AddColumn {
        table: "order".to_string(),
        column: c,
    }];
    let maria = render_sql(&steps, MariaDb);
    assert!(
        maria.contains(
            "ALTER TABLE `order` ADD COLUMN `status` VARCHAR(255) NOT NULL DEFAULT 'pending';"
        ),
        "\n{maria}"
    );
}

#[test]
fn drop_column_and_drop_table_carry_destructive_markers() {
    let steps = vec![
        Step::DropColumn {
            table: "product".to_string(),
            column: "legacy".to_string(),
        },
        Step::DropTable("gone".to_string()),
    ];
    let out = render_sql(&steps, Postgres);
    assert!(
        out.contains("-- DESTRUCTIVE"),
        "destructive marker missing\n{out}"
    );
    assert!(
        out.contains("ALTER TABLE \"product\" DROP COLUMN \"legacy\";"),
        "\n{out}"
    );
    assert!(out.contains("DROP TABLE \"gone\";"), "\n{out}");
}

#[test]
fn alter_column_diverges_per_dialect() {
    // null -> not_null AND type text -> int on the same column.
    let after = col("v", "int", false);
    let changes = vec![
        ColumnChange::Type {
            from: "text".to_string(),
            to: "int".to_string(),
        },
        ColumnChange::SetNotNull { has_default: false },
    ];
    let steps = vec![Step::AlterColumn {
        table: "t".to_string(),
        column: "v".to_string(),
        changes,
        after,
    }];

    // Postgres: one ALTER COLUMN sub-statement per change.
    let pg = render_sql(&steps, Postgres);
    assert!(
        pg.contains("ALTER TABLE \"t\" ALTER COLUMN \"v\" TYPE BIGINT;"),
        "\n{pg}"
    );
    assert!(
        pg.contains("ALTER TABLE \"t\" ALTER COLUMN \"v\" SET NOT NULL;"),
        "\n{pg}"
    );

    // MariaDB: a single full MODIFY COLUMN restating the resulting definition.
    let maria = render_sql(&steps, MariaDb);
    assert!(
        maria.contains("ALTER TABLE `t` MODIFY COLUMN `v` BIGINT NOT NULL;"),
        "\n{maria}"
    );

    // SQLite: a loud comment (no in-place ALTER COLUMN) — never broken SQL.
    let sqlite = render_sql(&steps, Sqlite);
    assert!(
        sqlite.contains("-- SQLite cannot ALTER COLUMN t.v in place"),
        "\n{sqlite}"
    );

    // Both narrowing and a new not-null-without-default are destructive.
    assert!(steps[0].destructive());
    assert!(render_sql(&steps, Postgres).contains("-- DESTRUCTIVE"));
}

#[test]
fn mariadb_default_only_alter_avoids_modify() {
    let after = col("v", "int", false);
    let steps = vec![Step::AlterColumn {
        table: "t".to_string(),
        column: "v".to_string(),
        changes: vec![ColumnChange::SetDefault("0".to_string())],
        after,
    }];
    let maria = render_sql(&steps, MariaDb);
    assert!(
        maria.contains("ALTER TABLE `t` ALTER COLUMN `v` SET DEFAULT 0;"),
        "\n{maria}"
    );
    assert!(!maria.contains("MODIFY COLUMN"), "\n{maria}");
}

#[test]
fn index_add_and_drop_render_per_dialect() {
    let uq = IndexSnap {
        name: "uq_u_email".to_string(),
        columns: vec!["email".to_string()],
        unique: true,
    };
    let add = vec![Step::AddUnique {
        table: "u".to_string(),
        index: uq,
    }];
    let out = render_sql(&add, Postgres);
    assert!(out.contains("-- DESTRUCTIVE"), "\n{out}"); // unique over existing data
    assert!(
        out.contains("CREATE UNIQUE INDEX \"uq_u_email\" ON \"u\" (\"email\");"),
        "\n{out}"
    );

    let drop = vec![Step::DropIndex {
        table: "u".to_string(),
        name: "idx_u_name".to_string(),
    }];
    // MySQL/MariaDB need the `ON <table>` qualifier; Postgres/SQLite drop by name.
    assert!(render_sql(&drop, MariaDb).contains("DROP INDEX `idx_u_name` ON `u`;"));
    assert!(render_sql(&drop, Postgres).contains("DROP INDEX \"idx_u_name\";"));
}

// ---- executable statements + content hash ---------------------------

#[test]
fn sql_statements_are_bare_and_one_per_statement() {
    // A create + an add-column → bare statements (no `;`, no comments), exactly what
    // `apply` runs one at a time through `Db::execute`.
    let mut t = table("thing", vec![col("name", "text", false)]);
    t.indexes.push(IndexSnap {
        name: "idx_thing_name".to_string(),
        columns: vec!["name".to_string()],
        unique: false,
    });
    let steps = vec![
        Step::CreateTable(t),
        Step::AddColumn {
            table: "thing".to_string(),
            column: col("size", "int", true),
        },
    ];
    let stmts = sql_statements(&steps, Postgres).unwrap();
    // create table, its trailing create index, then the add column.
    assert_eq!(stmts.len(), 3, "{stmts:#?}");
    assert!(
        stmts[0].starts_with("CREATE TABLE \"thing\" ("),
        "{}",
        stmts[0]
    );
    assert!(
        stmts.iter().all(|s| !s.ends_with(';')),
        "no trailing `;`: {stmts:#?}"
    );
    assert!(
        stmts.iter().all(|s| !s.contains("--")),
        "no comments: {stmts:#?}"
    );
    assert!(
        stmts[1].contains("CREATE INDEX \"idx_thing_name\" ON \"thing\" (\"name\")"),
        "{}",
        stmts[1]
    );
    assert!(
        stmts[2].contains("ALTER TABLE \"thing\" ADD COLUMN \"size\""),
        "{}",
        stmts[2]
    );
}

#[test]
fn sql_statements_errs_on_sqlite_alter_column() {
    // SQLite can't ALTER COLUMN in place — `apply` must fail loudly, not emit broken SQL.
    let steps = vec![Step::AlterColumn {
        table: "t".to_string(),
        column: "v".to_string(),
        changes: vec![ColumnChange::SetNotNull { has_default: false }],
        after: col("v", "int", false),
    }];
    let err = sql_statements(&steps, Sqlite).unwrap_err();
    assert!(err.contains("SQLite cannot ALTER COLUMN t.v"), "{err}");
}

// ---- @was renames + raw(dialect) escape -----------------------------

/// A field `@was` (persisted as a `Rename::Column`) turns what would be a drop+add into a
/// single `rename column` step — data-preserving, non-destructive — and renders per dialect.
#[test]
fn column_rename_via_was_is_one_step_not_drop_add() {
    let prev = Snapshot {
        scopes: Vec::new(),
        tables: vec![table("product", vec![col("upc", "text", true)])],
        renames: Vec::new(),
    };
    let now = Snapshot {
        scopes: Vec::new(),
        tables: vec![table("product", vec![col("barcode", "text", true)])],
        renames: vec![Rename::Column {
            table: "product".to_string(),
            from: "upc".to_string(),
            to: "barcode".to_string(),
        }],
    };
    let steps = diff_snapshots(&prev, &now);
    assert_eq!(steps.len(), 1, "{steps:?}");
    assert!(
        matches!(&steps[0], Step::RenameColumn { from, to, .. } if from == "upc" && to == "barcode")
    );
    assert!(!steps[0].destructive());
    assert!(render_up(&steps).contains("rename column product.upc -> barcode"));
    assert!(render_sql(&steps, Postgres)
        .contains("ALTER TABLE \"product\" RENAME COLUMN \"upc\" TO \"barcode\";"));
    assert!(render_sql(&steps, MariaDb)
        .contains("ALTER TABLE `product` RENAME COLUMN `upc` TO `barcode`;"));
    assert!(render_sql(&steps, Sqlite)
        .contains("ALTER TABLE `product` RENAME COLUMN `upc` TO `barcode`;"));
}

/// A model `@was` renames the table; a spent rename (old name already gone from `prev`)
/// is inert — no step — so leaving `@was` in place after capture is harmless.
#[test]
fn table_rename_and_spent_rename_is_inert() {
    let prev = Snapshot {
        scopes: Vec::new(),
        tables: vec![table("legacy_product", vec![col("name", "text", false)])],
        renames: Vec::new(),
    };
    let now = Snapshot {
        scopes: Vec::new(),
        tables: vec![table("product", vec![col("name", "text", false)])],
        renames: vec![Rename::Table {
            from: "legacy_product".to_string(),
            to: "product".to_string(),
        }],
    };
    let steps = diff_snapshots(&prev, &now);
    assert_eq!(steps.len(), 1, "{steps:?}");
    assert!(
        matches!(&steps[0], Step::RenameTable { from, to } if from == "legacy_product" && to == "product")
    );
    assert!(render_sql(&steps, Postgres)
        .contains("ALTER TABLE \"legacy_product\" RENAME TO \"product\";"));

    // `product` already exists in prev and the `@was` still names the (gone) old table:
    // the rename is spent, so the diff produces nothing (inert `@was`).
    let spent = diff_snapshots(&now, &now);
    assert!(spent.is_empty(), "{spent:?}");
}

/// A rename hint round-trips through `schema.snap` render/parse, so `apply`/`render`
/// recover it from the stored snapshot with no database.
#[test]
fn rename_hint_round_trips_through_schema_snap() {
    let snap = Snapshot {
        scopes: Vec::new(),
        tables: vec![table("product", vec![col("barcode", "text", true)])],
        renames: vec![
            Rename::Table {
                from: "legacy".to_string(),
                to: "product".to_string(),
            },
            Rename::Column {
                table: "product".to_string(),
                from: "upc".to_string(),
                to: "barcode".to_string(),
            },
        ],
    };
    let parsed = Snapshot::parse(&snap.render()).expect("round-trip");
    assert_eq!(snap, parsed, "\n{}", snap.render());
}

/// A `raw(dialect)` escape parsed from an `up.mig` emits its SQL only for the matching
/// target and is a no-op for the others (one raw step per dialect).
#[test]
fn raw_step_renders_only_for_its_dialect() {
    let up = "add column product.note text null\n\
              raw(postgres) `UPDATE \"product\" SET note = ''`\n\
              raw(sqlite) `UPDATE \"product\" SET note = ''`\n";
    assert!(has_raw_step(up));
    let raws = parse_raw_steps(up);
    assert_eq!(raws.len(), 2);
    // Postgres target: only the postgres raw runs.
    let pg = sql_statements(&raws, Postgres).unwrap();
    assert_eq!(pg.len(), 1);
    assert!(pg[0].contains("SET note"));
    // MariaDB target: neither raw matches → nothing runs.
    assert!(sql_statements(&raws, MariaDb).unwrap().is_empty());
    // The structural residue drops the raw lines (for verify).
    assert_eq!(
        strip_raw_steps(up).trim(),
        "add column product.note text null"
    );
}

#[test]
fn content_hash_ignores_comments_and_whitespace_but_not_steps() {
    let a = "# generated header\nadd column product.barcode text null\n";
    // Same step, different comment + blank lines + indentation → identical hash.
    let b = "\n  add column product.barcode text null  \n# a different comment\n";
    assert_eq!(content_hash(a), content_hash(b));
    // A real change to the step → a different hash (the tamper guard fires).
    let c = "add column product.barcode text not_null\n";
    assert_ne!(content_hash(a), content_hash(c));
    // 16 lowercase hex digits.
    assert_eq!(content_hash(a).len(), 16);
    assert!(content_hash(a).bytes().all(|b| b.is_ascii_hexdigit()));
}
