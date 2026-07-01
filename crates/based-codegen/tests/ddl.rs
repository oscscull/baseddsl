//! DDL codegen tests: parse + check a whole-schema snippet, then assert on the
//! generated `CREATE TABLE` text. Snippets are multi-decl so relation FKs, indexes,
//! and cross-model FK types are exercised the way the CLI drives them.

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_sema::check;

fn gen(src: &str) -> String {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    sql::ddl(&schema, Dialect::MariaDb)
}

#[test]
fn implicit_id_is_uuid_primary_key() {
    let ddl = gen("Org { name: text }");
    assert!(ddl.contains("CREATE TABLE `org` ("), "\n{ddl}");
    assert!(ddl.contains("`id` UUID NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("PRIMARY KEY (`id`)"), "\n{ddl}");
    // implicit id carries no SQL default (app-generated, D1)
    assert!(!ddl.contains("`id` UUID NOT NULL DEFAULT"), "\n{ddl}");
}

#[test]
fn type_mapping_and_nullability() {
    let ddl = gen(r#"
        Widget {
          name:   text
          note:   text?
          count:  int
          active: bool
          at:     timestamp
          day:    date
          blob:   json
          tags:   text[]
        }
        "#);
    assert!(ddl.contains("`name` VARCHAR(255) NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`note` VARCHAR(255) NULL"), "\n{ddl}");
    assert!(ddl.contains("`count` BIGINT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`active` BOOLEAN NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`at` DATETIME NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`day` DATE NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`blob` JSON NOT NULL"), "\n{ddl}");
    // to-many scalar has no columnar form -> JSON array
    assert!(ddl.contains("`tags` JSON NOT NULL"), "\n{ddl}");
}

#[test]
fn defaults_render_as_sql() {
    let ddl = gen(r#"
        Order {
          status: text (default "pending")
          total:  int (default 0)
          live:   bool (default true)
          at:     timestamp (default now())
        }
        "#);
    assert!(
        ddl.contains("`status` VARCHAR(255) NOT NULL DEFAULT 'pending'"),
        "\n{ddl}"
    );
    assert!(ddl.contains("`total` BIGINT NOT NULL DEFAULT 0"), "\n{ddl}");
    assert!(
        ddl.contains("`live` BOOLEAN NOT NULL DEFAULT TRUE"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("`at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP"),
        "\n{ddl}"
    );
}

#[test]
fn unique_modifier_becomes_constraint() {
    let ddl = gen("Org { slug: text (unique) }");
    assert!(
        ddl.contains("CONSTRAINT `uq_org_slug` UNIQUE (`slug`)"),
        "\n{ddl}"
    );
}

#[test]
fn forward_relation_emits_fk_column_no_constraint() {
    let ddl = gen(r#"
        Org { name: text }
        Order { org: Org, buyer: Org? }
        "#);
    // FK column named `<field>_id`, typed as the target's PK (uuid); optional -> NULL
    assert!(ddl.contains("`org_id` UUID NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`buyer_id` UUID NULL"), "\n{ddl}");
    // relations.md: no FK constraints by default
    assert!(!ddl.contains("FOREIGN KEY"), "\n{ddl}");
    assert!(!ddl.contains("REFERENCES"), "\n{ddl}");
}

#[test]
fn inverse_edge_stores_no_column() {
    let ddl = gen(r#"
        Org { name: text, orders: Order[] }
        Order { org: Org }
        "#);
    // the to-many `orders` edge lives on `order.org_id`, never a column on `org`
    let org = ddl.split("CREATE TABLE `order`").next().unwrap();
    assert!(!org.contains("orders"), "inverse leaked a column:\n{org}");
}

#[test]
fn index_columns_resolve_relations_to_fk() {
    let ddl = gen(r#"
        Org { name: text }
        User { name: text }
        Membership {
          org:  Org
          user: User
          role: text
          @index(org, user) unique
          @index role
        }
        "#);
    // composite unique index over the two FK columns (not the field names)
    assert!(
        ddl.contains("UNIQUE KEY `uq_membership_org_user` (`org_id`, `user_id`)"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("KEY `idx_membership_role` (`role`)"),
        "\n{ddl}"
    );
}
