//! Migration snapshot + diff goldens. The commerce `schema.snap` is pinned to a
//! blessed golden file (`tests/migrate/commerce.snap`) so a schema change surfaces as a
//! reviewable diff; re-bless with `BLESS=1 cargo test -p based-codegen --test migrate`.
//! The diff scenarios drive two schema versions through `diff_snapshots` to pin the
//! neutral step list and its destructive marking.

use based_ast::FileId;
use based_codegen::migrate::{self, Snapshot};
use based_codegen::sql;
use based_codegen::Dialect;
use based_parser::parse_file;
use based_sema::{check, CheckedSchema};
use std::path::{Path, PathBuf};

/// Parse + check a multi-decl snippet into a `CheckedSchema`, asserting it is clean.
fn checked(src: &str) -> CheckedSchema {
    checked_with_decls(src).1
}

/// Like [`checked`], but also returns the parsed declarations — needed by the `@was`
/// self-consume helpers, which locate the directive in source via the AST spans.
fn checked_with_decls(src: &str) -> (Vec<based_ast::Decl>, CheckedSchema) {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    (sf.decls, schema)
}

/// The single-file `sources` vector (path, text) the self-consume helper indexes by FileId.
fn sources(src: &str) -> Vec<(PathBuf, String)> {
    vec![(PathBuf::from("schema.bsl"), src.to_string())]
}

/// Load + check the whole commerce example the way the CLI does.
fn commerce() -> CheckedSchema {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
    let mut decls = Vec::new();
    let mut files: Vec<PathBuf> = walk_bsl(&dir);
    files.sort();
    for (i, path) in files.iter().enumerate() {
        let src = std::fs::read_to_string(path).expect("read bsl");
        let sf = parse_file(&src, FileId(i as u32))
            .unwrap_or_else(|d| panic!("parse {}: {d:#?}", path.display()));
        decls.extend(sf.decls);
    }
    let (schema, diags) = check(&decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .collect();
    assert!(errs.is_empty(), "commerce has sema errors: {errs:#?}");
    schema
}

fn walk_bsl(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            out.extend(walk_bsl(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("bsl") {
            out.push(path);
        }
    }
    out
}

#[test]
fn commerce_snapshot_is_stable() {
    let snap = migrate::snapshot(&commerce());
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrate/commerce.snap");

    if std::env::var_os("BLESS").is_some() {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, &snap).expect("write golden");
        return;
    }

    let want = std::fs::read_to_string(&golden).unwrap_or_default();
    assert_eq!(
        snap.trim_end(),
        want.trim_end(),
        "commerce snapshot drifted (re-bless with BLESS=1):\n{snap}"
    );
}

#[test]
fn commerce_snapshot_round_trips() {
    // The generated snapshot parses back to an identical neutral model (the property the
    // diff baseline relies on).
    let snap = Snapshot::from_schema(&commerce());
    let text = snap.render();
    let parsed = Snapshot::parse(&text).expect("round-trip parse");
    assert_eq!(snap, parsed, "\n{text}");
}

#[test]
fn init_diff_equals_the_from_scratch_create_set() {
    // `0001_init` (no prior snapshot) diffs against the empty schema, so every table in
    // the schema is a `create table` step — matching what `based gen sql` builds from
    // scratch (0001_init's up == the from-scratch snapshot).
    let schema = commerce();
    let steps = migrate::diff(&Snapshot::default(), &schema);
    let creates: Vec<_> = steps
        .iter()
        .filter(|s| matches!(s, migrate::Step::CreateTable(_)))
        .collect();
    assert_eq!(
        creates.len(),
        schema.models.len(),
        "one create per model, and nothing else"
    );
    assert_eq!(creates.len(), steps.len(), "0001_init is create-only");
    // A pure create set from scratch has no destructive steps.
    assert!(steps.iter().all(|s| !s.destructive()));

    // Sanity: the DDL codegen from the same schema mentions the same tables.
    let ddl = sql::ddl(&schema, Dialect::MariaDb);
    for m in &schema.models {
        assert!(ddl.contains(&m.table), "DDL missing table {}", m.table);
    }
}

#[test]
fn add_nullable_column_plus_index_is_the_worked_example() {
    // The worked example: Product gains a nullable `barcode` + an index.
    let base = "
        Org { id: Id  name: text }
        Product { id: Id  org: Org  name: text }
    ";
    let evolved = "
        Org { id: Id  name: text }
        Product { id: Id  org: Org  name: text  barcode: text?  @index barcode }
    ";
    let prev = Snapshot::from_schema(&checked(base));
    let steps = migrate::diff(&prev, &checked(evolved));

    let up = migrate::render_up(&steps);
    assert!(
        up.contains("add column product.barcode text null"),
        "\n{up}"
    );
    assert!(
        up.contains("add index idx_product_barcode (barcode)"),
        "\n{up}"
    );
    // A nullable add and a plain index are both safe — nothing destructive.
    assert!(steps.iter().all(|s| !s.destructive()), "\n{up}");
}

#[test]
fn enum_kinds_encode_distinctly_in_the_snapshot_and_round_trip() {
    // A string enum captures its wire values; an int enum a distinct `enum:int(...)`
    // encoding — so a variant add/remove OR a string↔int kind change is a diffable change.
    let schema = checked(
        r#"
        enum Status { pending, paid = "PAID" }
        enum Priority { low = 0, high = 1 }
        Order { id: Id, status: Status, priority: Priority, total: int }
        "#,
    );
    let text = migrate::snapshot(&schema);
    assert!(text.contains("enum(pending,PAID)"), "\n{text}");
    assert!(text.contains("enum:int(0,1)"), "\n{text}");
    // The snapshot parses back identically (the diff baseline property).
    let snap = Snapshot::from_schema(&schema);
    let parsed = Snapshot::parse(&snap.render()).expect("round-trip parse");
    assert_eq!(snap, parsed, "\n{text}");
}

#[test]
fn int_enum_from_scratch_renders_integer_column_and_int_check() {
    let schema = checked(
        r#"
        enum Priority { low = 0, medium = 1, high = 2 }
        Ticket { id: Id, priority: Priority (default low), title: text }
        "#,
    );
    let steps = migrate::diff(&Snapshot::default(), &schema);
    let sql = migrate::render_sql(&steps, Dialect::Postgres);
    // The from-scratch migration matches `based gen sql`: integer column + int CHECK.
    assert!(
        sql.contains("\"priority\" BIGINT NOT NULL DEFAULT 0"),
        "\n{sql}"
    );
    assert!(sql.contains("CHECK (\"priority\" IN (0, 1, 2))"), "\n{sql}");
}

#[test]
fn dropping_a_model_is_a_marked_drop_table() {
    let base = "
        Org { id: Id  name: text }
        Legacy { id: Id  note: text }
    ";
    let evolved = "Org { id: Id  name: text }";
    let prev = Snapshot::from_schema(&checked(base));
    let steps = migrate::diff(&prev, &checked(evolved));

    assert_eq!(steps.len(), 1);
    assert!(matches!(&steps[0], migrate::Step::DropTable(n) if n == "legacy"));
    assert!(steps[0].destructive());
    let up = migrate::render_up(&steps);
    assert!(up.contains("drop table legacy  # DESTRUCTIVE"), "\n{up}");
}

#[test]
fn init_render_produces_one_create_table_per_model_per_dialect() {
    // 0001_init's create steps render to real per-dialect DDL. Cross-check each
    // dialect against `based gen sql`'s own DDL — same tables, same PKs, dialect quoting.
    let schema = commerce();
    let steps = migrate::diff(&Snapshot::default(), &schema);

    for dialect in [Dialect::MariaDb, Dialect::Sqlite, Dialect::Postgres] {
        let sql = migrate::render_sql(&steps, dialect);
        let creates = sql.matches("CREATE TABLE ").count();
        assert_eq!(
            creates,
            schema.models.len(),
            "one CREATE TABLE per model ({dialect:?})\n{sql}"
        );
        // Every model's table appears, and an `id` PK is (re)synthesized for each .
        assert_eq!(
            sql.matches("PRIMARY KEY (").count(),
            schema.models.len(),
            "one PK per table ({dialect:?})\n{sql}"
        );
        for m in &schema.models {
            assert!(
                sql.contains(&dialect.quote(&m.table)),
                "render missing table {} ({dialect:?})",
                m.table
            );
        }
        // The init render is create-only — no stray ALTER/DROP and no SQLite alter-comment.
        assert!(!sql.contains("ALTER TABLE"), "init is create-only\n{sql}");
        assert!(!sql.contains("DROP "), "init is create-only\n{sql}");
        assert!(!sql.contains("-- SQLite cannot"), "\n{sql}");
    }

    // The MariaDB render carries the same column types `based gen sql` emits (one type
    // map): e.g. `int` -> BIGINT, `text` -> VARCHAR(255).
    let maria = migrate::render_sql(&steps, Dialect::MariaDb);
    assert!(maria.contains("BIGINT"), "\n{maria}");
    assert!(maria.contains("VARCHAR(255)"), "\n{maria}");
    // Postgres uses its native spellings.
    let pg = migrate::render_sql(&steps, Dialect::Postgres);
    assert!(pg.contains("TIMESTAMPTZ") || pg.contains("JSONB"), "\n{pg}");
}

#[test]
fn renamed_column_is_a_drop_add_pair_not_a_rename() {
    // Renames are never auto-guessed (the @was rename step is the exception): a changed name reads
    // as a drop of the old column + an add of the new one.
    let base = "Widget { id: Id  label: text }";
    let evolved = "Widget { id: Id  title: text }";
    let prev = Snapshot::from_schema(&checked(base));
    let steps = migrate::diff(&prev, &checked(evolved));

    assert!(steps
        .iter()
        .any(|s| matches!(s, migrate::Step::AddColumn { column, .. } if column.name == "title")));
    assert!(steps
        .iter()
        .any(|s| matches!(s, migrate::Step::DropColumn { column, .. } if column == "label")));
    assert!(!steps
        .iter()
        .any(|s| matches!(s, migrate::Step::AlterColumn { .. })));
}

// ---------- `@was` lifecycle: self-consume + teach-at-checkpoint (D105) ------

#[test]
fn gen_self_consumes_a_field_was_it_captured() {
    // A `@was` field rename: gen emits the rename step AND retires the now-spent directive
    // from source (the rename lives durably in the migration ledger).
    let base = "Widget { id: Id  upc: text }";
    let evolved = "Widget { id: Id  barcode: text @was(\"upc\") (unique) }";
    let prev = Snapshot::from_schema(&checked(base));
    let (decls, schema) = checked_with_decls(evolved);
    let steps = migrate::diff(&prev, &schema);

    // The migration captures the rename (data-preserving, not drop+add).
    assert!(steps
        .iter()
        .any(|s| matches!(s, migrate::Step::RenameColumn { from, to, .. }
            if from == "upc" && to == "barcode")));

    let edits = migrate::spent_was_edits(&steps, &schema, &decls, &sources(evolved));
    assert_eq!(edits.len(), 1, "one spent @was consumed");
    assert!(edits[0].label.contains("Widget.barcode"), "{:?}", edits[0]);

    let rewritten = migrate::apply_spent_was(evolved, &edits);
    assert!(
        !rewritten.contains("@was"),
        "directive removed: {rewritten}"
    );
    // Surgical: the rest of the field (and its other modifier) is byte-clean.
    assert_eq!(
        rewritten, "Widget { id: Id  barcode: text (unique) }",
        "{rewritten}"
    );
    // The rewritten source still parses + checks cleanly.
    checked(&rewritten);
}

#[test]
fn gen_self_consumes_a_model_was_on_its_own_line() {
    let base = "Legacy { id: Id  name: text }";
    let evolved = "@was(\"legacy\")\nWidget { id: Id  name: text }";
    let prev = Snapshot::from_schema(&checked(base));
    let (decls, schema) = checked_with_decls(evolved);
    let steps = migrate::diff(&prev, &schema);

    assert!(steps
        .iter()
        .any(|s| matches!(s, migrate::Step::RenameTable { from, to }
            if from == "legacy" && to == "widget")));

    let edits = migrate::spent_was_edits(&steps, &schema, &decls, &sources(evolved));
    assert_eq!(edits.len(), 1);
    let rewritten = migrate::apply_spent_was(evolved, &edits);
    // The whole decorator line (incl its newline) is removed, leaving the model intact.
    assert_eq!(rewritten, "Widget { id: Id  name: text }", "{rewritten}");
    checked(&rewritten);
}

#[test]
fn a_spent_was_is_not_consumed() {
    // A `@was` whose rename is already captured (the new name is already in the prior
    // snapshot) produces no rename step, so gen must NOT strip it — that is W0107's job,
    // and stripping it here would silently edit source on an unrelated `gen`.
    let already = "Widget { id: Id  barcode: text @was(\"upc\") }";
    let prev = Snapshot::from_schema(&checked("Widget { id: Id  barcode: text }"));
    let (decls, schema) = checked_with_decls(already);
    // A real, unrelated change rides along so the diff is non-empty.
    let evolved = "Widget { id: Id  barcode: text @was(\"upc\")  note: text? }";
    let (decls2, schema2) = checked_with_decls(evolved);
    let steps = migrate::diff(&prev, &schema2);
    assert!(steps
        .iter()
        .any(|s| matches!(s, migrate::Step::AddColumn { column, .. } if column.name == "note")));
    assert!(!steps
        .iter()
        .any(|s| matches!(s, migrate::Step::RenameColumn { .. })));

    // No rename step for the spent @was ⇒ nothing consumed, in either schema state.
    assert!(migrate::spent_was_edits(
        &migrate::diff(&prev, &schema),
        &schema,
        &decls,
        &sources(already)
    )
    .is_empty());
    assert!(migrate::spent_was_edits(&steps, &schema2, &decls2, &sources(evolved)).is_empty());
}

#[test]
fn rename_hints_fire_on_a_drop_add_same_family() {
    // A rename spelled without `@was` reads as drop+add; the teach hint points at `@was`.
    let base = "Widget { id: Id  label: text }";
    let evolved = "Widget { id: Id  title: text }";
    let prev = Snapshot::from_schema(&checked(base));
    let now = Snapshot::from_schema(&checked(evolved));
    let hints = migrate::rename_hints(&prev, &now);
    assert_eq!(hints.len(), 1, "{hints:?}");
    let msg = hints[0].message();
    assert!(
        msg.contains("label") && msg.contains("title") && msg.contains("@was(\"label\")"),
        "{msg}"
    );
}

#[test]
fn rename_hint_is_silent_when_the_rename_is_declared() {
    // With `@was`, the diff is a rename step, not a drop+add — no ambiguity, no hint.
    let base = "Widget { id: Id  label: text }";
    let evolved = "Widget { id: Id  title: text @was(\"label\") }";
    let prev = Snapshot::from_schema(&checked(base));
    let now = Snapshot::from_schema(&checked(evolved));
    assert!(migrate::rename_hints(&prev, &now).is_empty());
}

#[test]
fn rename_hint_is_silent_across_different_type_families() {
    // Dropping a text column and adding an int one is unlikely to be a rename — no hint,
    // so the signal stays low-false-positive.
    let base = "Widget { id: Id  label: text }";
    let evolved = "Widget { id: Id  count: int }";
    let prev = Snapshot::from_schema(&checked(base));
    let now = Snapshot::from_schema(&checked(evolved));
    assert!(migrate::rename_hints(&prev, &now).is_empty());
}

// ---------- opaque columns + exotic indexes in the neutral snapshot ---------

const OPAQUE: &str = r#"
    Place {
      id:       Id
      name:     text
      location: raw("geometry(Point,4326)")?
      search:   raw({ sqlite: "text", postgres: "tsvector", mariadb: "text" })?
      @index name using brin
      @index raw("(lower(name))")
    }
    "#;

#[test]
fn opaque_types_and_exotic_indexes_round_trip_the_snapshot() {
    let snap = Snapshot::from_schema(&checked(OPAQUE));
    let text = snap.render();
    assert!(
        text.contains(r#"  column location raw("geometry(Point,4326)") null"#),
        "\n{text}"
    );
    // The map is canonicalized dialect-sorted, so the diff never churns on order.
    assert!(
        text.contains(
            r#"  column search raw({ mariadb: "text", postgres: "tsvector", sqlite: "text" }) null"#
        ),
        "\n{text}"
    );
    assert!(text.contains("using brin"), "\n{text}");
    assert!(text.contains(r#"raw("(lower(name))")"#), "\n{text}");
    assert_eq!(Snapshot::parse(&text).expect("re-parse"), snap);
}

#[test]
fn changing_an_opaque_type_is_an_ordinary_column_diff() {
    let prev = Snapshot::from_schema(&checked(OPAQUE));
    let now = Snapshot::from_schema(&checked(
        &OPAQUE.replace("geometry(Point,4326)", "geometry(Polygon,4326)"),
    ));
    let steps = migrate::diff_snapshots(&prev, &now);
    let up = migrate::render_up(&steps);
    assert!(
        up.contains(r#"alter column place.location type raw("geometry(Polygon,4326)")"#),
        "\n{up}"
    );
    let sql = migrate::render_sql(&steps, Dialect::Postgres);
    assert!(
        sql.contains(r#"ALTER TABLE "place" ALTER COLUMN "location" TYPE geometry(Polygon,4326);"#),
        "\n{sql}"
    );
}

#[test]
fn a_migration_recreates_opaque_columns_and_indexes_verbatim() {
    let steps = migrate::diff_snapshots(
        &Snapshot::default(),
        &Snapshot::from_schema(&checked(OPAQUE)),
    );
    let pg = migrate::render_sql(&steps, Dialect::Postgres);
    assert!(pg.contains(r#""search" tsvector NULL"#), "\n{pg}");
    assert!(pg.contains(r#"USING brin ("name")"#), "\n{pg}");
    assert!(pg.contains("ON \"place\" (lower(name));"), "\n{pg}");
    // The from-scratch migration matches `based gen sql` byte-for-byte on the
    // opaque column type — one type map, no second one to drift.
    let ddl = sql::ddl(&checked(OPAQUE), Dialect::MariaDb);
    let maria = migrate::render_sql(&steps, Dialect::MariaDb);
    assert!(ddl.contains("`search` text NULL") && maria.contains("`search` text NULL"));
    assert!(
        maria.contains("KEY `idx_place_name` (`name`) USING BRIN"),
        "\n{maria}"
    );
}

// ---------- foreign-key constraints in the snapshot + diff ------------------

const FK_ORG: &str = "Org { id: Id  name: text }\n";

#[test]
fn fk_line_round_trips_through_the_snapshot() {
    let schema = checked(&format!(
        "{FK_ORG}Order {{ id: Id  org: Org @fk(on_delete: cascade, on_update: restrict) }}"
    ));
    let text = migrate::snapshot(&schema);
    assert!(
        text.contains("fk org_id -> org.id on_delete=cascade on_update=restrict"),
        "\n{text}"
    );
    let parsed = Snapshot::parse(&text).expect("parse");
    assert_eq!(parsed, Snapshot::from_schema(&schema));
}

#[test]
fn adding_an_fk_diffs_to_add_foreign_key() {
    let prev = Snapshot::from_schema(&checked(&format!("{FK_ORG}Order {{ id: Id  org: Org }}")));
    let now = Snapshot::from_schema(&checked(&format!(
        "{FK_ORG}Order {{ id: Id  org: Org @fk(on_delete: cascade) }}"
    )));
    let steps = migrate::diff_snapshots(&prev, &now);
    assert!(
        steps
            .iter()
            .any(|s| matches!(s, migrate::Step::AddForeignKey { .. })),
        "{steps:#?}"
    );
    // Renders to a real ALTER on Postgres, an honest rebuild marker on SQLite.
    let pg = migrate::render_sql(&steps, Dialect::Postgres);
    assert!(
        pg.contains("ADD CONSTRAINT") && pg.contains("ON DELETE CASCADE"),
        "\n{pg}"
    );
    let sqlite = migrate::render_sql(&steps, Dialect::Sqlite);
    assert!(sqlite.contains("raw(sqlite) table-rebuild"), "\n{sqlite}");
}

#[test]
fn dropping_an_fk_diffs_to_drop_foreign_key() {
    let prev = Snapshot::from_schema(&checked(&format!(
        "{FK_ORG}Order {{ id: Id  org: Org @fk(on_delete: cascade) }}"
    )));
    let now = Snapshot::from_schema(&checked(&format!("{FK_ORG}Order {{ id: Id  org: Org }}")));
    let steps = migrate::diff_snapshots(&prev, &now);
    assert!(
        steps
            .iter()
            .any(|s| matches!(s, migrate::Step::DropForeignKey { .. })),
        "{steps:#?}"
    );
    let maria = migrate::render_sql(&steps, Dialect::MariaDb);
    assert!(
        maria.contains("DROP FOREIGN KEY `fk_order_org_id`"),
        "\n{maria}"
    );
    let pg = migrate::render_sql(&steps, Dialect::Postgres);
    assert!(
        pg.contains(r#"DROP CONSTRAINT "fk_order_org_id""#),
        "\n{pg}"
    );
}

#[test]
fn changing_an_fk_action_diffs_to_drop_then_add() {
    let prev = Snapshot::from_schema(&checked(&format!(
        "{FK_ORG}Order {{ id: Id  org: Org @fk(on_delete: cascade) }}"
    )));
    let now = Snapshot::from_schema(&checked(&format!(
        "{FK_ORG}Order {{ id: Id  org: Org @fk(on_delete: restrict) }}"
    )));
    let steps = migrate::diff_snapshots(&prev, &now);
    let drops = steps
        .iter()
        .filter(|s| matches!(s, migrate::Step::DropForeignKey { .. }))
        .count();
    let adds = steps
        .iter()
        .filter(|s| matches!(s, migrate::Step::AddForeignKey { .. }))
        .count();
    assert_eq!((drops, adds), (1, 1), "{steps:#?}");
}

#[test]
fn from_scratch_migration_carries_the_fk_inline_on_sqlite() {
    // 0001_init CreateTable must build the FK inline on every target including SQLite.
    let now = Snapshot::from_schema(&checked(&format!(
        "{FK_ORG}Order {{ id: Id  org: Org @fk(on_delete: cascade) }}"
    )));
    let steps = migrate::diff_snapshots(&Snapshot::default(), &now);
    let sqlite = migrate::render_sql(&steps, Dialect::Sqlite);
    assert!(
        sqlite.contains("FOREIGN KEY (`org_id`) REFERENCES `org` (`id`) ON DELETE CASCADE"),
        "\n{sqlite}"
    );
    // The create-table renderer matches `based gen sql` (same clause).
    let ddl = sql::ddl(&now_schema_fk(), Dialect::Sqlite);
    assert!(
        ddl.contains("FOREIGN KEY (`org_id`) REFERENCES `org` (`id`)"),
        "\n{ddl}"
    );
}

fn now_schema_fk() -> CheckedSchema {
    checked(&format!(
        "{FK_ORG}Order {{ id: Id  org: Org @fk(on_delete: cascade) }}"
    ))
}
