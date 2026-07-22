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
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    schema
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
