//! DDL codegen tests: parse + check a whole-schema snippet, then assert on the
//! generated `CREATE TABLE` text. Snippets are multi-decl so relation FKs, indexes,
//! and cross-model FK types are exercised the way the CLI drives them.

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_sema::check;

fn gen_for(src: &str, dialect: Dialect) -> String {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    sql::ddl(&schema, dialect)
}

fn gen(src: &str) -> String {
    gen_for(src, Dialect::MariaDb)
}

fn gen_sqlite(src: &str) -> String {
    gen_for(src, Dialect::Sqlite)
}

fn gen_pg(src: &str) -> String {
    gen_for(src, Dialect::Postgres)
}

#[test]
fn implicit_id_is_uuid_primary_key() {
    let ddl = gen("Org { name: text }");
    assert!(ddl.contains("CREATE TABLE `org` ("), "\n{ddl}");
    assert!(ddl.contains("`id` UUID NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("PRIMARY KEY (`id`)"), "\n{ddl}");
    // implicit id carries no SQL default (app-generated)
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
    // no FK constraints by default
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

#[test]
fn inferred_join_key_emitted_predicate_leading() {
    // The shape traverses `items` (an inverse edge), so the child table gets an
    // engine-inferred index on the join FK, led by the soft-delete column
    // (predicate-leading — MariaDB has no partial indexes).
    let ddl = gen(r#"
        @sort(placed_at desc)
        Order { placed_at: timestamp, items: OrderItem[], @index placed_at }
        @soft_delete(deleted_at)
        OrderItem { deleted_at: timestamp?, order: Order, qty: int }
        shape O from Order { first_qty = items.qty }
        query orders() -> O[];
        "#);
    assert!(
        ddl.contains("KEY `inf_order_item_deleted_at_order` (`deleted_at`, `order_id`)"),
        "\n{ddl}"
    );
}

#[test]
fn inferred_join_key_deduped_when_declared() {
    // The user declared the join-key index; nothing engine-owned is emitted.
    let ddl = gen(r#"
        @sort(placed_at desc)
        Order { placed_at: timestamp, items: OrderItem[], @index placed_at }
        OrderItem { order: Order, qty: int, @index order }
        shape O from Order { first_qty = items.qty }
        query orders() -> O[];
        "#);
    assert!(
        ddl.contains("KEY `idx_order_item_order` (`order_id`)"),
        "\n{ddl}"
    );
    assert!(!ddl.contains("`inf_"), "\n{ddl}");
}

// ---------------------------------------------------------------------------
// SQLite dialect . Same resolved schema, SQLite-shaped physical output:
// a small type set, no inline index clauses (indexes trail as `CREATE INDEX`),
// bool defaults as `0`/`1`. The DML/mutation SQL is unchanged (dialect-portable).
// ---------------------------------------------------------------------------

#[test]
fn sqlite_header_names_the_dialect() {
    let ddl = gen_sqlite("Org { name: text }");
    assert!(ddl.contains("(dialect: sqlite)"), "\n{ddl}");
}

#[test]
fn sqlite_type_mapping_and_nullability() {
    let ddl = gen_sqlite(
        r#"
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
        "#,
    );
    // text/uuid/timestamp/date/json all ride as TEXT; int/bool as INTEGER
    assert!(ddl.contains("`name` TEXT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`note` TEXT NULL"), "\n{ddl}");
    assert!(ddl.contains("`count` INTEGER NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`active` INTEGER NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`at` TEXT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`day` TEXT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`blob` TEXT NOT NULL"), "\n{ddl}");
    // to-many scalar -> TEXT (SQLite has no JSON type; stored as a JSON string)
    assert!(ddl.contains("`tags` TEXT NOT NULL"), "\n{ddl}");
    // no MariaDB types leak through
    assert!(!ddl.contains("VARCHAR"), "\n{ddl}");
    assert!(!ddl.contains("BIGINT"), "\n{ddl}");
    assert!(!ddl.contains("BOOLEAN"), "\n{ddl}");
    assert!(!ddl.contains("DATETIME"), "\n{ddl}");
}

#[test]
fn sqlite_id_is_text_primary_key() {
    let ddl = gen_sqlite("Org { name: text }");
    assert!(ddl.contains("`id` TEXT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("PRIMARY KEY (`id`)"), "\n{ddl}");
    // app-generated id carries no SQL default
    assert!(!ddl.contains("`id` TEXT NOT NULL DEFAULT"), "\n{ddl}");
}

#[test]
fn sqlite_fk_column_is_text() {
    let ddl = gen_sqlite(
        r#"
        Org { name: text }
        Order { org: Org, buyer: Org? }
        "#,
    );
    assert!(ddl.contains("`org_id` TEXT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`buyer_id` TEXT NULL"), "\n{ddl}");
    assert!(!ddl.contains("FOREIGN KEY"), "\n{ddl}");
}

#[test]
fn sqlite_bool_default_renders_as_integer() {
    let ddl = gen_sqlite(
        r#"
        Order {
          status: text (default "pending")
          total:  int (default 0)
          live:   bool (default true)
          off:    bool (default false)
          at:     timestamp (default now())
        }
        "#,
    );
    assert!(
        ddl.contains("`status` TEXT NOT NULL DEFAULT 'pending'"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("`total` INTEGER NOT NULL DEFAULT 0"),
        "\n{ddl}"
    );
    // bool default is an integer literal on SQLite (no TRUE/FALSE keyword reliance)
    assert!(ddl.contains("`live` INTEGER NOT NULL DEFAULT 1"), "\n{ddl}");
    assert!(ddl.contains("`off` INTEGER NOT NULL DEFAULT 0"), "\n{ddl}");
    assert!(!ddl.contains("DEFAULT TRUE"), "\n{ddl}");
    // now() still lowers to CURRENT_TIMESTAMP (SQLite supports it)
    assert!(
        ddl.contains("`at` TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP"),
        "\n{ddl}"
    );
}

#[test]
fn sqlite_unique_constraint_stays_inline() {
    // A column-level `(unique)` is an inline table constraint on both dialects.
    let ddl = gen_sqlite("Org { slug: text (unique) }");
    assert!(
        ddl.contains("CONSTRAINT `uq_org_slug` UNIQUE (`slug`)"),
        "\n{ddl}"
    );
}

#[test]
fn sqlite_indexes_are_separate_create_index_statements() {
    let ddl = gen_sqlite(
        r#"
        Org { name: text }
        User { name: text }
        Membership {
          org:  Org
          user: User
          role: text
          @index(org, user) unique
          @index role
        }
        "#,
    );
    // No inline KEY / UNIQUE KEY index clause on SQLite (it has no such table syntax).
    // (`PRIMARY KEY` is the column constraint, not an index clause — allowed.)
    assert!(!ddl.contains("UNIQUE KEY"), "\n{ddl}");
    assert!(!ddl.contains("  KEY `"), "\n{ddl}");
    // Instead each index trails as its own statement, columns resolved to FK cols.
    assert!(
        ddl.contains(
            "CREATE UNIQUE INDEX `uq_membership_org_user` ON `membership` (`org_id`, `user_id`);"
        ),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("CREATE INDEX `idx_membership_role` ON `membership` (`role`);"),
        "\n{ddl}"
    );
    // The CREATE INDEX statements come *after* the CREATE TABLE they index.
    let table_end = ddl.find("CREATE TABLE `membership`").unwrap();
    let idx_at = ddl.find("CREATE INDEX `idx_membership_role`").unwrap();
    assert!(idx_at > table_end, "index emitted before its table:\n{ddl}");
}

#[test]
fn sqlite_inferred_join_key_is_a_create_index() {
    // The inferred join-key baseline  becomes a trailing CREATE INDEX on SQLite,
    // still predicate-leading (soft-delete column first).
    let ddl = gen_sqlite(
        r#"
        @sort(placed_at desc)
        Order { placed_at: timestamp, items: OrderItem[], @index placed_at }
        @soft_delete(deleted_at)
        OrderItem { deleted_at: timestamp?, order: Order, qty: int }
        shape O from Order { first_qty = items.qty }
        query orders() -> O[];
        "#,
    );
    assert!(
        ddl.contains(
            "CREATE INDEX `inf_order_item_deleted_at_order` ON `order_item` (`deleted_at`, `order_id`);"
        ),
        "\n{ddl}"
    );
}

// ---------- Postgres  -------------------------------------------------

#[test]
fn pg_uses_double_quoted_identifiers_and_native_types() {
    // Postgres double-quotes identifiers (`"order"`, a reserved word — the reason
    // quoting matters) and has native BIGINT / BOOLEAN / TIMESTAMPTZ / UUID types.
    let ddl = gen_pg(
        r#"
        @created(created_at)
        Order {
          created_at: timestamp
          status:     text
          total:      int
          live:       bool
          tags:       text[]
          meta:       json
        }
        "#,
    );
    assert!(ddl.contains("CREATE TABLE \"order\" ("), "\n{ddl}");
    assert!(ddl.contains("\"id\" UUID NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("PRIMARY KEY (\"id\")"), "\n{ddl}");
    assert!(ddl.contains("\"status\" TEXT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("\"total\" BIGINT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("\"live\" BOOLEAN NOT NULL"), "\n{ddl}");
    assert!(
        ddl.contains("\"created_at\" TIMESTAMPTZ NOT NULL"),
        "\n{ddl}"
    );
    // json -> JSONB (the indexable/`@>`-queryable form); a to-many scalar -> JSONB too.
    assert!(ddl.contains("\"meta\" JSONB NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("\"tags\" JSONB NOT NULL"), "\n{ddl}");
    // no backtick-quoted identifiers in a Postgres artifact (the `based gen sql`
    // header comment has backticks in its prose, so check the statement body only).
    let body = &ddl[ddl.find("CREATE TABLE").unwrap()..];
    assert!(!body.contains('`'), "\n{ddl}");
}

#[test]
fn pg_fk_column_is_uuid() {
    let ddl = gen_pg(
        r#"
        Org { name: text }
        Order { org: Org, buyer: Org? }
        "#,
    );
    assert!(ddl.contains("\"org_id\" UUID NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("\"buyer_id\" UUID NULL"), "\n{ddl}");
    assert!(!ddl.contains("FOREIGN KEY"), "\n{ddl}");
}

#[test]
fn pg_bool_default_uses_keyword() {
    // Unlike SQLite, Postgres has TRUE/FALSE keywords (like MariaDB).
    let ddl = gen_pg(
        r#"
        Order {
          status: text (default "pending")
          live:   bool (default true)
          off:    bool (default false)
          at:     timestamp (default now())
        }
        "#,
    );
    assert!(
        ddl.contains("\"status\" TEXT NOT NULL DEFAULT 'pending'"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("\"live\" BOOLEAN NOT NULL DEFAULT TRUE"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("\"off\" BOOLEAN NOT NULL DEFAULT FALSE"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("\"at\" TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP"),
        "\n{ddl}"
    );
}

#[test]
fn pg_indexes_are_separate_create_index_statements() {
    // Like SQLite, Postgres has no inline KEY / UNIQUE KEY clause: indexes trail the
    // table as CREATE [UNIQUE] INDEX statements. `(unique)` stays an inline constraint.
    let ddl = gen_pg(
        r#"
        Org { slug: text (unique) }
        User { name: text }
        Membership {
          org:  Org
          user: User
          role: text
          @index(org, user) unique
          @index role
        }
        "#,
    );
    assert!(!ddl.contains("UNIQUE KEY"), "\n{ddl}");
    assert!(!ddl.contains("  KEY \""), "\n{ddl}");
    assert!(
        ddl.contains("CONSTRAINT \"uq_org_slug\" UNIQUE (\"slug\")"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains(
            "CREATE UNIQUE INDEX \"uq_membership_org_user\" ON \"membership\" (\"org_id\", \"user_id\");"
        ),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("CREATE INDEX \"idx_membership_role\" ON \"membership\" (\"role\");"),
        "\n{ddl}"
    );
}
