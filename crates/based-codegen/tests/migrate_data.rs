//! Typed migration data steps: placement among the structural steps, the offline check
//! against the schema state at each line, and per-dialect lowering. The scenarios are the
//! edhbuilder backfills (a JSON field copy, reference rows, the paper backfill).

use based_codegen::migrate::{plan_up, up_mig_matches_snapshot, Snapshot, UpPlan, UpUnit};
use based_codegen::Dialect;

const BEFORE: &str = "snapshot v1 dialect=neutral

table card
  column digital bool not_null default=false
  column name text not_null
  column paper bool not_null default=false

table card_printing
  column card_id uuid not_null fk=Card
  column data json not_null
  column digital bool not_null
  index idx_card_printing_card_id (card_id)

table format
  column key text not_null unique
  column legal bool not_null default=true
  column name text not_null
";

/// `BEFORE` plus `card_printing.set_code`.
fn after() -> String {
    BEFORE.replace(
        "  column digital bool not_null\n  index",
        "  column digital bool not_null\n  column set_code text null\n  index",
    )
}

fn snap(text: &str) -> Snapshot {
    Snapshot::parse(text).unwrap_or_else(|e| panic!("{e}"))
}

fn plan(prev: &str, now: &str, up: &str, dialect: Dialect) -> UpPlan {
    plan_up(&snap(prev), &snap(now), up, dialect).unwrap_or_else(|p| panic!("{p:?}"))
}

fn problems(prev: &str, now: &str, up: &str) -> String {
    match plan_up(&snap(prev), &snap(now), up, Dialect::Sqlite) {
        Ok(_) => panic!("expected a problem"),
        Err(ps) => ps
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn sql(p: &UpPlan) -> Vec<&str> {
    p.units
        .iter()
        .map(|u| match u {
            UpUnit::Sql(s) => s.as_str(),
            UpUnit::Chunked(_) => "<chunked>",
        })
        .collect()
}

const JSON_COPY: &str = "format 2
add column card_printing.set_code text null
update all card_printing { set_code = data.set }
";

#[test]
fn json_field_copy_runs_after_the_column_it_fills() {
    let now = after();
    assert!(up_mig_matches_snapshot(
        JSON_COPY,
        &based_codegen::migrate::diff_snapshots(&snap(BEFORE), &snap(&now))
    ));
    let lite = plan(BEFORE, &now, JSON_COPY, Dialect::Sqlite);
    let units = sql(&lite);
    assert_eq!(units.len(), 2, "{units:?}");
    assert!(
        units[0].starts_with("ALTER TABLE `card_printing` ADD COLUMN"),
        "{units:?}"
    );
    assert_eq!(
        units[1],
        "UPDATE `card_printing` SET `set_code` = json_extract(`card_printing`.`data`, '$.set')"
    );
    let maria = plan(BEFORE, &now, JSON_COPY, Dialect::MariaDb);
    assert_eq!(
        sql(&maria)[1],
        "UPDATE `card_printing` SET `set_code` = JSON_UNQUOTE(JSON_EXTRACT(`card_printing`.`data`, '$.set'))"
    );
    let pg = plan(BEFORE, &now, JSON_COPY, Dialect::Postgres);
    assert_eq!(
        sql(&pg)[1],
        r#"UPDATE "card_printing" SET "set_code" = ("card_printing"."data" #>> '{set}')"#
    );
}

#[test]
fn json_path_casts_to_the_target_column() {
    let up = "format 2\nupdate all format { legal = data.legal.commander }\n";
    let with_data = BEFORE.replace(
        "  column key text not_null unique",
        "  column data json not_null\n  column key text not_null unique",
    );
    let pg = plan(&with_data, &with_data, up, Dialect::Postgres);
    assert_eq!(
        sql(&pg)[0],
        r#"UPDATE "format" SET "legal" = CAST(("format"."data" #>> '{legal,commander}') AS BOOLEAN)"#
    );
    let maria = plan(&with_data, &with_data, up, Dialect::MariaDb);
    assert_eq!(
        sql(&maria)[0],
        "UPDATE `format` SET `legal` = (JSON_EXTRACT(`format`.`data`, '$.legal.commander') = CAST('true' AS JSON))"
    );
}

const REFERENCE_ROWS: &str = r#"format 2
create format[] from [
  { key = "commander", name = "Commander" },
  { key = "brawl", name = "Brawl" },
] on conflict (key) update { ...incoming }
"#;

#[test]
fn reference_rows_upsert_per_dialect() {
    let pg = plan(BEFORE, BEFORE, REFERENCE_ROWS, Dialect::Postgres);
    assert_eq!(
        sql(&pg)[0],
        r#"INSERT INTO "format" ("key", "name", "id") VALUES ('commander', 'Commander', gen_random_uuid()), ('brawl', 'Brawl', gen_random_uuid()) ON CONFLICT ("key") DO UPDATE SET "name" = excluded."name""#
    );
    let maria = plan(BEFORE, BEFORE, REFERENCE_ROWS, Dialect::MariaDb);
    assert!(
        sql(&maria)[0].ends_with("ON DUPLICATE KEY UPDATE `name` = VALUES(`name`)"),
        "{:?}",
        sql(&maria)
    );
    let lite = plan(BEFORE, BEFORE, REFERENCE_ROWS, Dialect::Sqlite);
    assert!(sql(&lite)[0].contains("ON CONFLICT (`key`) DO UPDATE SET `name` = excluded.`name`"));
    // An upsert can't be undone mechanically.
    assert_eq!(pg.auto_down, None);
}

#[test]
fn literal_rows_reverse_automatically() {
    let up = "format 2\ncreate format { key = \"pauper\", name = \"Pauper\" }\n";
    let p = plan(BEFORE, BEFORE, up, Dialect::Sqlite);
    assert_eq!(
        p.auto_down,
        Some(vec![
            "DELETE FROM `format` WHERE `key` IN ('pauper')".to_string()
        ])
    );
    let upd = "format 2\nupdate format where (key = \"pauper\") { name = \"P\" }\n";
    assert_eq!(plan(BEFORE, BEFORE, upd, Dialect::Sqlite).auto_down, None);
}

const PAPER_BACKFILL: &str =
    "format 2\nupdate card where (card_printing.any(digital = false)) { paper = true }\n";

#[test]
fn paper_backfill_uses_relation_any() {
    let lite = plan(BEFORE, BEFORE, PAPER_BACKFILL, Dialect::Sqlite);
    assert_eq!(
        sql(&lite)[0],
        "UPDATE `card` SET `paper` = 1 WHERE EXISTS (SELECT 1 FROM `card_printing` AS `s1_card_printing` WHERE `s1_card_printing`.`card_id` = `card`.`id` AND `s1_card_printing`.`digital` = 0)"
    );
    let pg = plan(BEFORE, BEFORE, PAPER_BACKFILL, Dialect::Postgres);
    assert_eq!(
        sql(&pg)[0],
        r#"UPDATE "card" SET "paper" = TRUE WHERE EXISTS (SELECT 1 FROM "card_printing" AS "s1_card_printing" WHERE "s1_card_printing"."card_id" = "card"."id" AND "s1_card_printing"."digital" = FALSE)"#
    );
}

#[test]
fn raw_lines_run_where_they_are_written() {
    let now = after();
    let up = "format 2\nraw(sqlite) `SELECT 1`\nadd column card_printing.set_code text null\n";
    let units = plan(BEFORE, &now, up, Dialect::Sqlite);
    assert_eq!(sql(&units)[0], "SELECT 1");
    // Without the marker, raw keeps running last.
    let legacy = "raw(sqlite) `SELECT 1`\nadd column card_printing.set_code text null\n";
    let units = plan(BEFORE, &now, legacy, Dialect::Sqlite);
    assert_eq!(sql(&units).last().copied(), Some("SELECT 1"));
}

#[test]
fn chunked_update_walks_the_primary_key() {
    let up = "format 2\nupdate all card { paper = true } chunk 5000 pause 200ms\n";
    let p = plan(BEFORE, BEFORE, up, Dialect::Sqlite);
    let UpUnit::Chunked(c) = &p.units[0] else {
        panic!("{:?}", p.units)
    };
    assert_eq!(c.key_columns, vec!["id"]);
    assert_eq!(c.pause_ms, Some(200));
    assert_eq!(
        c.select_sql(None),
        "SELECT `card`.`id` FROM `card` ORDER BY `card`.`id` LIMIT 5000"
    );
    assert_eq!(
        c.apply_sql(Some(&["'a'".to_string()]), &["'b'".to_string()]),
        "UPDATE `card` SET `paper` = 1 WHERE `card`.`id` > 'a' AND `card`.`id` <= 'b'"
    );
}

#[test]
fn chunked_delete_by_an_indexed_column_keeps_its_filter() {
    let up = "format 2\ndelete card_printing where (digital = true) chunk 100 by card_id\n";
    let p = plan(BEFORE, BEFORE, up, Dialect::Postgres);
    let UpUnit::Chunked(c) = &p.units[0] else {
        panic!("{:?}", p.units)
    };
    assert_eq!(
        c.select_sql(Some(&["'x'".to_string()])),
        r#"SELECT "card_printing"."card_id" FROM "card_printing" WHERE "card_printing"."digital" = TRUE AND "card_printing"."card_id" > 'x' ORDER BY "card_printing"."card_id" LIMIT 100"#
    );
}

#[test]
fn check_errors_name_the_problem_and_line() {
    let cases = [
        ("update all card { colour = \"red\" }", "has no column `colour`"),
        ("update all nope { a = 1 }", "unknown table `nope`"),
        ("update all card { paper = \"yes\" }", "cannot write a string to `paper`"),
        ("create format { key = \"x\" }", "missing required column(s) `name`"),
        (
            "create format[] from [ { key = \"x\", name = \"X\" } ] on conflict (name) update { ...incoming }",
            "is not a unique key",
        ),
        ("update all card { name = data.x }", "has no column `data`"),
        ("update all card { paper = name.x }", "not a json column"),
        (
            "create format { key = \"x\", name = \"X\" } chunk 10",
            "`chunk` applies to `update` / `delete`",
        ),
        ("update all card { paper = true } chunk 10 by name", "unique or indexed column"),
        ("update all card { id = \"x\" } chunk 10", "can't assign `id`"),
        ("update card where (format.any()) { paper = true }", "has no foreign key to `card`"),
        ("update all card { paper = $flag }", "no parameters"),
    ];
    for (step, want) in cases {
        let up = format!("format 2\n\n{step}\n");
        let got = problems(BEFORE, BEFORE, &up);
        assert!(got.contains(want), "{step}\n  want: {want}\n  got: {got}");
        assert!(got.contains("up.mig line 3"), "{step}: {got}");
    }
}

#[test]
fn a_data_step_sees_the_schema_as_of_its_line() {
    let now = after();
    let early = "format 2\nupdate all card_printing { set_code = \"x\" }\nadd column card_printing.set_code text null\n";
    assert!(problems(BEFORE, &now, early).contains("has no column `set_code`"));
}

#[test]
fn data_steps_need_the_in_order_format() {
    let up = "update all card { paper = true }\n";
    assert!(problems(BEFORE, BEFORE, up).contains("format 2"));
}

#[test]
fn mysql_rejects_a_subquery_on_the_table_being_written() {
    let tree = "snapshot v1 dialect=neutral

table node
  column name text not_null
  column parent_id uuid null fk=Node
";
    let up = "format 2\ndelete node where (not node.any())\n";
    assert!(plan_up(&snap(tree), &snap(tree), up, Dialect::Sqlite).is_ok());
    let err = plan_up(&snap(tree), &snap(tree), up, Dialect::MariaDb).unwrap_err();
    assert!(err[0].message.contains("can't read `node`"), "{err:?}");
}
